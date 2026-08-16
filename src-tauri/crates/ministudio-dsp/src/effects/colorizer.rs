const COLORIZER_MAX_MODES: usize = 96;
const COLORIZER_LOWEST_MIDI: usize = 36;
const COLORIZER_HIGHEST_MIDI: usize = 119;
const MAX_ACTIVE_PITCHES: usize = 12;

/// `QUALITY = Fast`: lower latency, appropriate for upper-band/synth material
/// where responsiveness matters more than dense low-end separation.
const COLORIZER_MAP_FAST_FFT_SIZE: usize = 512;
const COLORIZER_MAP_FAST_HOP_SIZE: usize = 128;
/// `QUALITY = Clean`: the default. Matches the previous fixed 1024-point path.
const COLORIZER_MAP_CLEAN_FFT_SIZE: usize = 1024;
const COLORIZER_MAP_CLEAN_HOP_SIZE: usize = 256;

/// Both `ColorizerMap` engines run during this window on a `QUALITY` switch.
/// Their outputs are linearly crossfaded so the FFT-size change (and the
/// resulting jump in algorithmic latency) is inaudible instead of a click.
const MAP_QUALITY_CROSSFADE_MS: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorizerQuality {
    /// Immediate modal colour with no algorithmic latency.
    Live,
    /// Polyphonic spectral pitch-class mapping with transient preservation.
    Map,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MapQuality {
    Fast,
    Clean,
}
impl MapQuality {
    fn fft_size(self) -> usize {
        match self {
            Self::Fast => COLORIZER_MAP_FAST_FFT_SIZE,
            Self::Clean => COLORIZER_MAP_CLEAN_FFT_SIZE,
        }
    }
}

struct ColorizerMap {
    engine: spectral::stft::StftEngine,
    processor: spectral::pitch_map::PitchMapProcessor,
    dry_delay: [Vec<f32>; MAX_CHANNELS],
    dry_position: usize,
    fft_size: usize,
    meter: [f32; DISTORTION_SPECTRUM_BINS],
}

impl ColorizerMap {
    fn new(sample_rate: f32, fft_size: usize, hop_size: usize) -> Self {
        let plan =
            spectral::stft::StftPlan::new(spectral::stft::StftConfig::new(fft_size, hop_size))
                .expect("Colorizer uses a compile-time validated STFT configuration");
        Self {
            engine: spectral::stft::StftEngine::new(plan),
            processor: spectral::pitch_map::PitchMapProcessor::new(sample_rate, fft_size, hop_size),
            dry_delay: std::array::from_fn(|_| vec![0.0; fft_size]),
            dry_position: 0,
            fft_size,
            meter: [0.0; DISTORTION_SPECTRUM_BINS],
        }
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.processor.reset();
        for channel in &mut self.dry_delay {
            channel.fill(0.0);
        }
        self.dry_position = 0;
        self.meter.fill(0.0);
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn process_sample(
        &mut self,
        input: [f32; MAX_CHANNELS],
        target_mask: u16,
        amount: f32,
        transient_preserve: f32,
        color: f32,
        gate: f32,
    ) -> ([f32; MAX_CHANNELS], [f32; MAX_CHANNELS]) {
        let mut delayed = [0.0; MAX_CHANNELS];
        for channel in 0..MAX_CHANNELS {
            delayed[channel] = self.dry_delay[channel][self.dry_position];
            self.dry_delay[channel][self.dry_position] = input[channel];
        }
        self.dry_position += 1;
        if self.dry_position == self.fft_size {
            self.dry_position = 0;
        }

        let processor = &mut self.processor;
        let meter = &mut self.meter;
        let mapped = self.engine.process_sample(input, |frame| {
            processor.process(frame, target_mask, amount, transient_preserve, color, gate);
            meter.fill(0.0);
            meter[..spectral::analysis::HPCP_BINS]
                .copy_from_slice(processor.analyzer().hpcp());
            meter[spectral::analysis::HPCP_BINS] = processor.analyzer().transient().strength;
        });
        (delayed, mapped)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum ChordSource {
    AutoDetect,
    Manual,
    MidiInput,
}

/// Fixed-capacity control snapshot. MIDI and manual scale selection converge
/// here before the realtime resonator coefficients are rebuilt.
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

/// One complex modal resonator. Coefficients are prepared only when the scale,
/// sample rate, resonance or decay changes; processing is a pair of fused
/// multiply/add recurrences per channel with no allocation, FFT or OLA delay.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct ColorizerMode {
    frequency: f32,
    pitch_class: u8,
    cosine: f32,
    sine: f32,
    radius: f32,
    injection: f32,
    weight: f32,
    real: [f32; MAX_CHANNELS],
    imaginary: [f32; MAX_CHANNELS],
}
impl ColorizerMode {
    const EMPTY: Self = Self {
        frequency: 0.0,
        pitch_class: 0,
        cosine: 1.0,
        sine: 0.0,
        radius: 0.0,
        injection: 0.0,
        weight: 0.0,
        real: [0.0; MAX_CHANNELS],
        imaginary: [0.0; MAX_CHANNELS],
    };
    fn clear(&mut self) {
        self.real = [0.0; MAX_CHANNELS];
        self.imaginary = [0.0; MAX_CHANNELS];
    }
}

/// Dual-backend chromatic processor shown to users as `Colorizer`.
///
/// `Live` is the zero-latency fixed modal bank: selected pitch classes ring
/// across octaves without executing an FFT. `Map` is an explicit higher-cost
/// path that moves linked-stereo harmonic families onto the selected pitch grid
/// while preserving transients and reporting its fixed STFT latency to PDC.
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
    transient_preserve: f32,
    /// Spectral gate amount for `Map` mode; 0 fully skips the gate stage.
    gate: f32,
    mix: Smoother,
    quality: ColorizerQuality,
    map_quality: MapQuality,
    map_fast: ColorizerMap,
    map_clean: ColorizerMap,
    /// Nonzero while a `QUALITY` switch is being crossfaded in; both engines
    /// run during this window. See `MAP_QUALITY_CROSSFADE_MS`.
    map_crossfade_remaining: u32,
    map_crossfade_total: u32,
    /// The engine fading *out* of a crossfade (the one active before the
    /// switch). `map_quality` already holds the incoming target.
    map_crossfade_from: MapQuality,
    modes: [ColorizerMode; COLORIZER_MAX_MODES],
    active_modes: usize,
}
impl Colorizer {
    fn new() -> Self {
        let mut effect = Self {
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
            transient_preserve: 0.72,
            gate: 0.0,
            mix: Smoother::new(0.72, 48_000.0, 0.015),
            quality: ColorizerQuality::Live,
            map_quality: MapQuality::Clean,
            map_fast: ColorizerMap::new(
                48_000.0,
                COLORIZER_MAP_FAST_FFT_SIZE,
                COLORIZER_MAP_FAST_HOP_SIZE,
            ),
            map_clean: ColorizerMap::new(
                48_000.0,
                COLORIZER_MAP_CLEAN_FFT_SIZE,
                COLORIZER_MAP_CLEAN_HOP_SIZE,
            ),
            map_crossfade_remaining: 0,
            map_crossfade_total: 1,
            map_crossfade_from: MapQuality::Clean,
            modes: [ColorizerMode::EMPTY; COLORIZER_MAX_MODES],
            active_modes: 0,
        };
        effect.rebuild_modes();
        effect
    }
    /// Runs whichever `ColorizerMap` engine(s) are currently live: just the
    /// active one at rest, or both (linearly crossfaded) while `QUALITY` is
    /// switching so the FFT-size/latency change never clicks.
    fn process_map_sample(
        &mut self,
        dry: [f32; MAX_CHANNELS],
    ) -> ([f32; MAX_CHANNELS], [f32; MAX_CHANNELS]) {
        let target_mask = self.active_mask();
        let (depth, transient, color, gate) =
            (self.depth, self.transient_preserve, self.resonance, self.gate);
        if self.map_crossfade_remaining == 0 {
            let active = match self.map_quality {
                MapQuality::Fast => &mut self.map_fast,
                MapQuality::Clean => &mut self.map_clean,
            };
            return active.process_sample(dry, target_mask, depth, transient, color, gate);
        }
        let (from, to) = match self.map_crossfade_from {
            MapQuality::Fast => (&mut self.map_fast, &mut self.map_clean),
            MapQuality::Clean => (&mut self.map_clean, &mut self.map_fast),
        };
        let (delayed_from, wet_from) =
            from.process_sample(dry, target_mask, depth, transient, color, gate);
        let (delayed_to, wet_to) = to.process_sample(dry, target_mask, depth, transient, color, gate);
        let progress =
            1.0 - self.map_crossfade_remaining as f32 / self.map_crossfade_total.max(1) as f32;
        self.map_crossfade_remaining -= 1;
        let delayed = std::array::from_fn(|c| {
            delayed_from[c] + (delayed_to[c] - delayed_from[c]) * progress
        });
        let wet = std::array::from_fn(|c| wet_from[c] + (wet_to[c] - wet_from[c]) * progress);
        (delayed, wet)
    }
    fn active_map(&self) -> &ColorizerMap {
        match self.map_quality {
            MapQuality::Fast => &self.map_fast,
            MapQuality::Clean => &self.map_clean,
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
            self.rebuild_modes();
        }
    }
    fn decay_seconds(&self) -> f32 {
        let base = 0.018 + self.decay * self.decay * 1.25;
        base * (0.4 + self.resonance * 1.6)
    }
    fn decay_coefficient(&self) -> f32 {
        (-1.0 / (self.decay_seconds() * self.sample_rate.max(8_000.0))).exp()
    }
    fn rebuild_modes(&mut self) {
        let mask = self.active_mask();
        let pitch_count = mask.count_ones().max(1) as f32;
        let radius = self.decay_coefficient().clamp(0.0, 0.999_995);
        // The positive-frequency pole sees half of a real sinusoid. 2*(1-r)
        // therefore approaches unity gain on a matching steady tone.
        let injection = (2.0 * (1.0 - radius)).max(1e-6);
        let normalization = 1.1 / pitch_count.powf(0.2);
        let nyquist_guard = self.sample_rate * 0.45;
        let mut count = 0;
        for midi in COLORIZER_LOWEST_MIDI..=COLORIZER_HIGHEST_MIDI {
            let pitch_class = (midi % 12) as u8;
            if mask & (1 << pitch_class) == 0 {
                continue;
            }
            let frequency = 440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0);
            if frequency >= nyquist_guard || count == COLORIZER_MAX_MODES {
                continue;
            }
            let phase = 2.0 * PI * frequency / self.sample_rate;
            // Slight high-frequency damping keeps dense scales smooth without
            // masking the bright selected partials that provide the color.
            let spectral_weight = (1.0 + (frequency / 7_500.0).powi(2)).powf(-0.18);
            self.modes[count] = ColorizerMode {
                frequency,
                pitch_class,
                cosine: phase.cos(),
                sine: phase.sin(),
                radius,
                injection,
                weight: normalization * spectral_weight,
                real: [0.0; MAX_CHANNELS],
                imaginary: [0.0; MAX_CHANNELS],
            };
            count += 1;
        }
        for mode in &mut self.modes[count..] {
            *mode = ColorizerMode::EMPTY;
        }
        self.active_modes = count;
        self.applied_mask = mask;
    }
    fn handle_event(&mut self, event: NoteEventKind) -> bool {
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
            _ => return false,
        }
        let mut mask = 0_u16;
        for (pitch, count) in self.midi_notes.iter().enumerate() {
            if *count > 0 {
                mask |= 1 << (pitch % 12)
            }
        }
        if mask != self.midi_mask {
            self.midi_mask = mask;
            true
        } else {
            false
        }
    }

    #[inline]
    fn process_live_sample(&mut self, dry: [f32; MAX_CHANNELS]) -> [f32; MAX_CHANNELS] {
        let mut resonant = [0.0_f32; MAX_CHANNELS];
        for mode in &mut self.modes[..self.active_modes] {
            for channel in 0..MAX_CHANNELS {
                let real = mode.real[channel];
                let imaginary = mode.imaginary[channel];
                let next_real = mode.radius * (real * mode.cosine - imaginary * mode.sine)
                    + dry[channel] * mode.injection;
                let next_imaginary =
                    mode.radius * (real * mode.sine + imaginary * mode.cosine);
                mode.real[channel] = denormal(next_real);
                mode.imaginary[channel] = denormal(next_imaginary);
                resonant[channel] += next_real * mode.weight;
            }
        }
        let depth = self.depth.clamp(0.0, 1.0);
        std::array::from_fn(|channel| {
            let colored = dry[channel] + resonant[channel] * depth;
            if colored.abs() > 3.0 {
                let excess = colored.abs() - 3.0;
                colored.signum() * (3.0 + excess / (1.0 + excess))
            } else {
                colored
            }
        })
    }
}
impl DspEffect for Colorizer {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.mix = Smoother::new(self.mix.target, self.sample_rate, 0.015);
        // Both QUALITY engines are prepared up front so switching at runtime
        // is a pointer flip plus a crossfade, never an audio-thread allocation.
        self.map_fast = ColorizerMap::new(
            self.sample_rate,
            COLORIZER_MAP_FAST_FFT_SIZE,
            COLORIZER_MAP_FAST_HOP_SIZE,
        );
        self.map_clean = ColorizerMap::new(
            self.sample_rate,
            COLORIZER_MAP_CLEAN_FFT_SIZE,
            COLORIZER_MAP_CLEAN_HOP_SIZE,
        );
        self.map_crossfade_remaining = 0;
        self.map_crossfade_total =
            ((self.sample_rate * MAP_QUALITY_CROSSFADE_MS / 1000.0) as u32).max(1);
        self.rebuild_modes();
        self.reset();
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed && self.quality == ColorizerQuality::Live {
            return;
        }
        debug_assert!(events
            .windows(2)
            .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
        let mut event_index = 0;
        for frame in 0..frames.min(MAX_BLOCK_SIZE) {
            if self.source == ChordSource::MidiInput {
                let mut chord_changed = false;
                while event_index < events.len()
                    && events[event_index].sample_offset as usize == frame
                {
                    chord_changed |= self.handle_event(events[event_index].kind);
                    event_index += 1;
                }
                if chord_changed {
                    // A whole chord arriving at one sample rebuilds the fixed
                    // bank once, not once per individual note event.
                    self.update_active_pitches();
                }
            }
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let mix = self.mix.next().clamp(0.0, 1.0);
            match self.quality {
                ColorizerQuality::Live => {
                    let wet = self.process_live_sample(dry);
                    for channel in 0..MAX_CHANNELS {
                        buffer.channels[channel][frame] =
                            denormal(dry[channel] + (wet[channel] - dry[channel]) * mix);
                    }
                }
                ColorizerQuality::Map => {
                    let (delayed, wet) = self.process_map_sample(dry);
                    for channel in 0..MAX_CHANNELS {
                        buffer.channels[channel][frame] = if self.bypassed {
                            delayed[channel]
                        } else {
                            denormal(delayed[channel] + (wet[channel] - delayed[channel]) * mix)
                        };
                    }
                }
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
                    self.rebuild_modes();
                }
            }
            "resonance" => {
                self.resonance = value.clamp(0.0, 1.0);
                self.rebuild_modes();
            }
            "decay" => {
                self.decay = value.clamp(0.0, 1.0);
                self.rebuild_modes();
            }
            "depth" => self.depth = value.clamp(0.0, 1.0),
            "transient" => self.transient_preserve = value.clamp(0.0, 1.0),
            "gate" => self.gate = value.clamp(0.0, 1.0),
            "quality" => {
                self.quality = if value >= 0.5 {
                    ColorizerQuality::Map
                } else {
                    ColorizerQuality::Live
                }
            }
            "mapQuality" => {
                let requested = if value >= 0.5 {
                    MapQuality::Clean
                } else {
                    MapQuality::Fast
                };
                // A switch mid-crossfade is ignored rather than reflown: the
                // in-flight fade already commits to a `from`/`to` pair, and
                // resolving a second request before it lands would need to
                // re-derive a consistent blend from three engines instead of
                // two. The next `set_param` after it settles picks it up.
                if requested != self.map_quality && self.map_crossfade_remaining == 0 {
                    self.map_crossfade_from = self.map_quality;
                    self.map_quality = requested;
                    self.map_crossfade_remaining = self.map_crossfade_total;
                }
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
        for mode in &mut self.modes {
            mode.clear();
        }
        self.midi_notes.fill(0);
        self.midi_mask = 0;
        self.map_fast.reset();
        self.map_clean.reset();
        self.map_crossfade_remaining = 0;
        self.rebuild_modes();
    }
    fn tail_samples(&self) -> usize {
        match self.quality {
            ColorizerQuality::Live => (self.decay_seconds() * 9.21 * self.sample_rate) as usize,
            ColorizerQuality::Map => self.active_map().fft_size,
        }
    }
    fn latency_samples(&self) -> usize {
        match self.quality {
            ColorizerQuality::Live => 0,
            // Reflects the settled/target engine. A `QUALITY` switch changes
            // this declared value without a matching PDC graph rebuild, the
            // same accepted trade-off Formant Shifter's Mono/Poly toggle
            // already makes (see `formant.rs::latency_samples`) — the audible
            // FFT-size transition is handled by the crossfade above, not by
            // this contract.
            ColorizerQuality::Map => self.map_quality.fft_size(),
        }
    }
    fn wants_midi(&self) -> bool {
        self.source == ChordSource::MidiInput
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        (self.quality == ColorizerQuality::Map).then_some(self.active_map().meter)
    }
}
