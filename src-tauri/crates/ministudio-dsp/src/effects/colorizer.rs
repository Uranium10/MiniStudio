const COLORIZER_FFT_SIZE: usize = 4096;
const COLORIZER_HOP_SIZE: usize = COLORIZER_FFT_SIZE / 4;
const COLORIZER_BINS: usize = COLORIZER_FFT_SIZE / 2 + 1;
const COLORIZER_MASK_SMOOTH_FRAMES: usize = 4;
const MAX_ACTIVE_PITCHES: usize = 12;
const COLORIZER_METER_BINS: usize = DISTORTION_SPECTRUM_BINS / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum ChordSource {
    AutoDetect,
    Manual,
    MidiInput,
}

/// Fixed-capacity control-rate pitch snapshot. It is deliberately independent
/// from both UI presets and MIDI routing so spectral DSP only sees pitch data.
#[derive(Clone, Copy, Debug)]
struct ActivePitches {
    pitches: [u8; MAX_ACTIVE_PITCHES],
    count: usize,
    confidence: f32,
    changed: bool,
}
impl ActivePitches {
    fn from_mask(mask: u16, previous: u16) -> Self {
        let mut pitches = [0; MAX_ACTIVE_PITCHES];
        let mut count = 0;
        for pitch in 0..12 {
            if mask & (1 << pitch) != 0 {
                pitches[count] = pitch as u8;
                count += 1;
            }
        }
        Self {
            pitches,
            count,
            confidence: 1.0,
            changed: mask != previous,
        }
    }
    fn mask(&self) -> u16 {
        self.pitches[..self.count]
            .iter()
            .fold(0_u16, |mask, pitch| mask | (1 << pitch))
    }
}

struct ColorizerChannel {
    input_ring: Vec<f32>,
    dry_delay: Vec<f32>,
    ola_ring: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    held: Vec<f32>,
    phase: Vec<f32>,
    previous_magnitude: Vec<f32>,
}
impl ColorizerChannel {
    fn new(scratch_len: usize) -> Self {
        Self {
            input_ring: vec![0.0; COLORIZER_FFT_SIZE],
            dry_delay: vec![0.0; COLORIZER_FFT_SIZE],
            ola_ring: vec![0.0; COLORIZER_FFT_SIZE],
            spectrum: vec![Complex32::new(0.0, 0.0); COLORIZER_FFT_SIZE],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            held: vec![0.0; COLORIZER_BINS],
            phase: vec![0.0; COLORIZER_BINS],
            previous_magnitude: vec![0.0; COLORIZER_BINS],
        }
    }
    fn clear(&mut self) {
        self.input_ring.fill(0.0);
        self.dry_delay.fill(0.0);
        self.ola_ring.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
        self.scratch.fill(Complex32::new(0.0, 0.0));
        self.held.fill(0.0);
        self.phase.fill(0.0);
        self.previous_magnitude.fill(0.0);
    }
}

/// Harmonic STFT resonator shown to users as `Colorizer`.
///
/// FFT plans, scratch, rings, masks, and phase state are all prepared before
/// processing. `process` performs no heap allocation or locking.
struct Colorizer {
    sample_rate: f32,
    bypassed: bool,
    source: ChordSource,
    manual_mask: u16,
    midi_mask: u16,
    applied_mask: u16,
    midi_notes: [u8; 128],
    resonance: f32,
    decay: f32,
    depth: f32,
    mix: Smoother,
    mask_current: Vec<f32>,
    mask_start: Vec<f32>,
    mask_target: Vec<f32>,
    mask_dirty: bool,
    mask_smooth_remaining: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    channels: [ColorizerChannel; MAX_CHANNELS],
    ring_position: usize,
    samples_seen: usize,
    samples_until_frame: usize,
    meter: [f32; DISTORTION_SPECTRUM_BINS],
}
impl Colorizer {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(COLORIZER_FFT_SIZE);
        let inverse = planner.plan_fft_inverse(COLORIZER_FFT_SIZE);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Self {
            sample_rate: 48_000.0,
            bypassed: false,
            source: ChordSource::Manual,
            manual_mask: 0b1010_1101_0101,
            midi_mask: 0,
            applied_mask: 0,
            midi_notes: [0; 128],
            resonance: 0.62,
            decay: 0.45,
            depth: 0.82,
            mix: Smoother::new(0.72, 48_000.0, 0.015),
            mask_current: vec![1.0; COLORIZER_BINS],
            mask_start: vec![1.0; COLORIZER_BINS],
            mask_target: vec![1.0; COLORIZER_BINS],
            mask_dirty: true,
            mask_smooth_remaining: 0,
            window: vec![0.0; COLORIZER_FFT_SIZE],
            forward,
            inverse,
            channels: [
                ColorizerChannel::new(scratch_len),
                ColorizerChannel::new(scratch_len),
            ],
            ring_position: 0,
            samples_seen: 0,
            samples_until_frame: COLORIZER_FFT_SIZE,
            meter: [0.0; DISTORTION_SPECTRUM_BINS],
        }
    }
    fn active_mask(&self) -> u16 {
        match self.source {
            ChordSource::MidiInput => self.midi_mask,
            ChordSource::Manual | ChordSource::AutoDetect => self.manual_mask,
        }
    }
    fn update_active_pitches(&mut self) {
        let pitches = ActivePitches::from_mask(self.active_mask(), self.applied_mask);
        let _confidence = pitches.confidence;
        if pitches.changed {
            self.applied_mask = pitches.mask();
            self.mask_dirty = true;
        }
    }
    fn rebuild_mask(&mut self) {
        if !self.mask_dirty {
            return;
        }
        self.mask_start.copy_from_slice(&self.mask_current);
        let pitch_mask = self.active_mask();
        let midi_closed = self.source == ChordSource::MidiInput && pitch_mask == 0;
        let sigma = 1.55 * (1.0 - self.resonance).powi(2) + 0.075;
        let bin_width = self.sample_rate / COLORIZER_FFT_SIZE as f32;
        for (bin, gain) in self.mask_target.iter_mut().enumerate() {
            let frequency = bin as f32 * self.sample_rate / COLORIZER_FFT_SIZE as f32;
            if midi_closed {
                *gain = 0.0;
                continue;
            }
            let harmonic_mask = if (40.0..=12_000.0).contains(&frequency) && pitch_mask != 0 {
                let midi_pitch = 69.0 + 12.0 * (frequency / 440.0).log2();
                let pitch_class = midi_pitch.rem_euclid(12.0);
                // A bell narrower than one FFT bin can miss a low note
                // completely. Widen only as much as the local bin resolution
                // requires, preserving the requested logarithmic Q elsewhere.
                let resolution_sigma = if frequency > bin_width * 0.55 {
                    12.0 * ((frequency + bin_width * 0.5) / (frequency - bin_width * 0.5)).log2()
                        * 0.55
                } else {
                    3.0
                };
                let effective_sigma = sigma.max(resolution_sigma);
                let mut sum = 0.0_f32;
                for pitch in 0..12 {
                    if pitch_mask & (1 << pitch) == 0 {
                        continue;
                    }
                    let direct = (pitch_class - pitch as f32).abs();
                    let distance = direct.min(12.0 - direct);
                    sum += (-0.5 * (distance / effective_sigma).powi(2)).exp();
                }
                sum.min(1.0)
            } else {
                0.0
            };
            *gain = harmonic_mask + (1.0 - harmonic_mask) * (1.0 - self.depth);
        }
        self.mask_smooth_remaining = COLORIZER_MASK_SMOOTH_FRAMES;
        self.mask_dirty = false;
    }
    fn advance_mask(&mut self) {
        self.rebuild_mask();
        if self.mask_smooth_remaining == 0 {
            return;
        }
        let completed = COLORIZER_MASK_SMOOTH_FRAMES - self.mask_smooth_remaining + 1;
        let amount = completed as f32 / COLORIZER_MASK_SMOOTH_FRAMES as f32;
        for bin in 0..COLORIZER_BINS {
            self.mask_current[bin] =
                self.mask_start[bin] + (self.mask_target[bin] - self.mask_start[bin]) * amount;
        }
        self.mask_smooth_remaining -= 1;
    }
    fn settle_mask_before_audio(&mut self) {
        if self.samples_seen != 0 {
            return;
        }
        self.rebuild_mask();
        self.mask_current.copy_from_slice(&self.mask_target);
        self.mask_smooth_remaining = 0;
    }
    fn decay_coefficient(&self) -> f32 {
        if self.decay <= 1e-5 {
            return 0.0;
        }
        let seconds = 0.035 * (240.0_f32).powf(self.decay);
        (-(COLORIZER_HOP_SIZE as f32) / (seconds * self.sample_rate))
            .exp()
            .min(0.999_95)
    }
    fn handle_event(&mut self, event: NoteEventKind) {
        match event {
            NoteEventKind::NoteOn {
                pitch, velocity, ..
            } if velocity > 0.0 => {
                self.midi_notes[pitch as usize] = self.midi_notes[pitch as usize].saturating_add(1)
            }
            NoteEventKind::NoteOn { pitch, .. } | NoteEventKind::NoteOff { pitch, .. } => {
                self.midi_notes[pitch as usize] = self.midi_notes[pitch as usize].saturating_sub(1)
            }
            NoteEventKind::AllNotesOff => self.midi_notes.fill(0),
            _ => return,
        }
        let mut mask = 0_u16;
        for (pitch, count) in self.midi_notes.iter().enumerate() {
            if *count > 0 {
                mask |= 1 << (pitch % 12)
            }
        }
        if mask != self.midi_mask {
            self.midi_mask = mask;
            self.update_active_pitches();
        }
    }
    fn render_frame(&mut self) {
        self.advance_mask();
        let decay = self.decay_coefficient();
        self.meter[..COLORIZER_METER_BINS].fill(0.0);
        for channel in &mut self.channels {
            process_colorizer_frame(
                channel,
                &self.forward,
                &self.inverse,
                &self.window,
                &self.mask_current,
                self.sample_rate,
                self.ring_position,
                decay,
                &mut self.meter[..COLORIZER_METER_BINS],
            );
        }
        for value in &mut self.meter[..COLORIZER_METER_BINS] {
            *value *= 0.5;
        }
        for index in 0..COLORIZER_METER_BINS {
            let frequency = colorizer_meter_frequency(index);
            let bin = ((frequency * COLORIZER_FFT_SIZE as f32 / self.sample_rate).round() as usize)
                .min(COLORIZER_BINS - 1);
            self.meter[COLORIZER_METER_BINS + index] = self.mask_current[bin];
        }
    }
}
impl DspEffect for Colorizer {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let mut planner = FftPlanner::<f32>::new();
        self.forward = planner.plan_fft_forward(COLORIZER_FFT_SIZE);
        self.inverse = planner.plan_fft_inverse(COLORIZER_FFT_SIZE);
        let scratch_len = self
            .forward
            .get_inplace_scratch_len()
            .max(self.inverse.get_inplace_scratch_len());
        self.channels = [
            ColorizerChannel::new(scratch_len),
            ColorizerChannel::new(scratch_len),
        ];
        for (index, value) in self.window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * PI * index as f32 / COLORIZER_FFT_SIZE as f32).cos();
        }
        self.mix = Smoother::new(self.mix.target, self.sample_rate, 0.015);
        self.reset();
        self.mask_dirty = true;
        self.rebuild_mask();
        self.mask_current.copy_from_slice(&self.mask_target);
        self.mask_smooth_remaining = 0;
        self.applied_mask = self.active_mask();
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        debug_assert!(events
            .windows(2)
            .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
        let mut event_index = 0;
        for frame in 0..frames.min(MAX_BLOCK_SIZE) {
            if self.source == ChordSource::MidiInput {
                while event_index < events.len()
                    && events[event_index].sample_offset as usize == frame
                {
                    self.handle_event(events[event_index].kind);
                    event_index += 1;
                }
            }
            let mix = self.mix.next();
            for channel in 0..MAX_CHANNELS {
                let input = buffer.channels[channel][frame];
                let state = &mut self.channels[channel];
                state.input_ring[self.ring_position] = input;
                let dry = state.dry_delay[self.ring_position];
                state.dry_delay[self.ring_position] = input;
                let wet = state.ola_ring[self.ring_position];
                state.ola_ring[self.ring_position] = 0.0;
                buffer.channels[channel][frame] = denormal(dry + (wet - dry) * mix);
            }
            self.ring_position = increment_wrap(self.ring_position, COLORIZER_FFT_SIZE);
            self.samples_seen = self.samples_seen.saturating_add(1);
            self.samples_until_frame -= 1;
            if self.samples_until_frame == 0 {
                self.render_frame();
                self.samples_until_frame = COLORIZER_HOP_SIZE;
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "midi" => {
                let source = if value >= 0.5 {
                    ChordSource::MidiInput
                } else {
                    ChordSource::Manual
                };
                if source != self.source {
                    self.source = source;
                    self.applied_mask = self.active_mask();
                    self.mask_dirty = true;
                    self.settle_mask_before_audio();
                }
            }
            "resonance" => {
                self.resonance = value.clamp(0.0, 1.0);
                self.mask_dirty = true;
                self.settle_mask_before_audio();
            }
            "decay" => self.decay = value.clamp(0.0, 1.0),
            "depth" => {
                self.depth = value.clamp(0.0, 1.0);
                self.mask_dirty = true;
                self.settle_mask_before_audio();
            }
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            _ if id.starts_with("pitch") => {
                if let Ok(pitch) = id[5..].parse::<usize>() {
                    if pitch < 12 {
                        if value >= 0.5 {
                            self.manual_mask |= 1 << pitch
                        } else {
                            self.manual_mask &= !(1 << pitch)
                        }
                        self.update_active_pitches();
                        self.settle_mask_before_audio();
                    }
                }
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.clear();
        }
        self.midi_notes.fill(0);
        self.midi_mask = 0;
        self.applied_mask = self.active_mask();
        self.mask_dirty = true;
        self.ring_position = 0;
        self.samples_seen = 0;
        self.samples_until_frame = COLORIZER_FFT_SIZE;
        self.meter.fill(0.0);
    }
    fn tail_samples(&self) -> usize {
        if self.decay <= 1e-5 {
            COLORIZER_FFT_SIZE
        } else {
            let seconds = 0.035 * (240.0_f32).powf(self.decay);
            COLORIZER_FFT_SIZE + (seconds * 9.21 * self.sample_rate) as usize
        }
    }
    fn latency_samples(&self) -> usize {
        COLORIZER_FFT_SIZE
    }
    fn wants_midi(&self) -> bool {
        self.source == ChordSource::MidiInput
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.meter)
    }
}

#[allow(clippy::too_many_arguments)]
fn process_colorizer_frame(
    channel: &mut ColorizerChannel,
    forward: &Arc<dyn Fft<f32>>,
    inverse: &Arc<dyn Fft<f32>>,
    window: &[f32],
    mask: &[f32],
    sample_rate: f32,
    ring_position: usize,
    decay: f32,
    meter: &mut [f32],
) {
    for index in 0..COLORIZER_FFT_SIZE {
        let input_index = (ring_position + index) % COLORIZER_FFT_SIZE;
        channel.spectrum[index] =
            Complex32::new(channel.input_ring[input_index] * window[index], 0.0);
    }
    forward.process_with_scratch(&mut channel.spectrum, &mut channel.scratch);
    let mut flux = 0.0_f32;
    let mut energy = 1e-12_f32;
    for bin in 0..COLORIZER_BINS {
        let magnitude = channel.spectrum[bin].norm();
        flux += (magnitude - channel.previous_magnitude[bin]).max(0.0);
        energy += magnitude;
        channel.previous_magnitude[bin] = magnitude;
    }
    let frame_decay = if flux / energy > 0.18 {
        decay * 0.35
    } else {
        decay
    };
    for bin in 0..COLORIZER_BINS {
        let input = channel.spectrum[bin];
        let input_magnitude = input.norm();
        let filtered = input_magnitude * mask[bin];
        let previous = channel.held[bin];
        let held = if frame_decay <= 1e-8 {
            filtered
        } else {
            filtered.max(previous * frame_decay)
        };
        channel.held[bin] = denormal(held);
        if held <= 1e-20 {
            channel.spectrum[bin] = Complex32::new(0.0, 0.0);
            continue;
        }
        let input_phase = input.arg();
        let expected =
            2.0 * PI * bin as f32 * COLORIZER_HOP_SIZE as f32 / COLORIZER_FFT_SIZE as f32;
        let propagated = wrap_phase(channel.phase[bin] + expected);
        let fresh = (filtered / (held + 1e-20)).clamp(0.0, 1.0);
        let phase_vector = Complex32::from_polar(fresh, input_phase)
            + Complex32::from_polar(1.0 - fresh, propagated);
        let phase = if phase_vector.norm_sqr() > 1e-12 {
            phase_vector.arg()
        } else {
            propagated
        };
        channel.phase[bin] = wrap_phase(phase);
        channel.spectrum[bin] = Complex32::from_polar(held, phase);
        if bin > 0 && bin < COLORIZER_FFT_SIZE / 2 {
            channel.spectrum[COLORIZER_FFT_SIZE - bin] = channel.spectrum[bin].conj();
        }
    }
    inverse.process_with_scratch(&mut channel.spectrum, &mut channel.scratch);
    let normalization = 1.0 / (COLORIZER_FFT_SIZE as f32 * 1.5);
    for index in 0..COLORIZER_FFT_SIZE {
        let output_index = (ring_position + index) % COLORIZER_FFT_SIZE;
        channel.ola_ring[output_index] +=
            channel.spectrum[index].re * window[index] * normalization;
    }
    for (index, value) in meter.iter_mut().enumerate() {
        let frequency = colorizer_meter_frequency(index);
        let bin = ((frequency * COLORIZER_FFT_SIZE as f32 / sample_rate).round() as usize)
            .min(COLORIZER_BINS - 1);
        let amplitude = channel.held[bin] * (2.0 / COLORIZER_FFT_SIZE as f32);
        *value += amplitude.max(0.0).sqrt().min(1.0);
    }
}

#[inline(always)]
fn colorizer_meter_frequency(index: usize) -> f32 {
    40.0 * (12_000.0_f32 / 40.0).powf(index as f32 / (COLORIZER_METER_BINS - 1) as f32)
}

#[inline(always)]
fn wrap_phase(phase: f32) -> f32 {
    (phase + PI).rem_euclid(2.0 * PI) - PI
}
