const COLORIZER_FAST_FFT_SIZE: usize = 512;
const COLORIZER_FAST_HOP_SIZE: usize = 128;
const COLORIZER_CLEAN_FFT_SIZE: usize = 1024;
const COLORIZER_CLEAN_HOP_SIZE: usize = 256;
const QUALITY_CROSSFADE_MS: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorizerQuality {
    Fast,
    Clean,
}

impl ColorizerQuality {
    fn fft_size(self) -> usize {
        match self {
            Self::Fast => COLORIZER_FAST_FFT_SIZE,
            Self::Clean => COLORIZER_CLEAN_FFT_SIZE,
        }
    }

    fn hop_size(self) -> usize {
        match self {
            Self::Fast => COLORIZER_FAST_HOP_SIZE,
            Self::Clean => COLORIZER_CLEAN_HOP_SIZE,
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
    fn new(sample_rate: f32, quality: ColorizerQuality) -> Self {
        let fft_size = quality.fft_size();
        let hop_size = quality.hop_size();
        let plan =
            spectral::stft::StftPlan::new(spectral::stft::StftConfig::new(fft_size, hop_size))
                .expect("Colorizer uses validated real-FFT configurations");
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

    fn clear_resonance(&mut self) {
        self.processor.clear_resonance();
    }

    #[inline]
    fn process_sample(
        &mut self,
        input: [f32; MAX_CHANNELS],
        target_mask: u16,
        morph: f32,
        gate: f32,
        resonance: f32,
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
            processor.process(frame, target_mask, morph, gate, resonance);
            meter.fill(0.0);
            meter[..spectral::analysis::HPCP_BINS]
                .copy_from_slice(processor.analyzer().hpcp());
            meter[spectral::analysis::HPCP_BINS] = processor.analyzer().transient().strength;
        });
        (delayed, mapped)
    }
}

/// Harmonic-family spectral pitch mapper. `color` is normalized to 0..2:
/// 0..1 is delayed dry/wet, 1..2 adds a strictly separate resonance layer.
struct Colorizer {
    sample_rate: f32,
    bypassed: bool,
    manual_mask: u16,
    midi_mask: u16,
    midi_enabled: bool,
    midi_notes: [u8; 128],
    color: Smoother,
    morph: f32,
    gate: f32,
    quality: ColorizerQuality,
    fast: ColorizerMap,
    clean: ColorizerMap,
    crossfade_from: ColorizerQuality,
    crossfade_remaining: u32,
    crossfade_total: u32,
}

impl Colorizer {
    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            bypassed: false,
            manual_mask: 0b1010_1101_0101,
            midi_mask: 0,
            midi_enabled: false,
            midi_notes: [0; 128],
            color: Smoother::new(0.72, 48_000.0, 0.015),
            morph: 0.72,
            gate: 0.0,
            quality: ColorizerQuality::Clean,
            fast: ColorizerMap::new(48_000.0, ColorizerQuality::Fast),
            clean: ColorizerMap::new(48_000.0, ColorizerQuality::Clean),
            crossfade_from: ColorizerQuality::Clean,
            crossfade_remaining: 0,
            crossfade_total: 1,
        }
    }

    fn active_mask(&self) -> u16 {
        if self.midi_enabled {
            self.midi_mask
        } else {
            self.manual_mask
        }
    }

    fn active_map(&self) -> &ColorizerMap {
        match self.quality {
            ColorizerQuality::Fast => &self.fast,
            ColorizerQuality::Clean => &self.clean,
        }
    }

    fn set_quality(&mut self, requested: ColorizerQuality) {
        if requested != self.quality && self.crossfade_remaining == 0 {
            self.crossfade_from = self.quality;
            self.quality = requested;
            self.crossfade_remaining = self.crossfade_total;
        }
    }

    fn handle_event(&mut self, event: NoteEventKind) {
        match event {
            NoteEventKind::NoteOn {
                pitch, velocity, ..
            } if velocity > 0.0 => {
                self.midi_notes[pitch as usize] =
                    self.midi_notes[pitch as usize].saturating_add(1)
            }
            NoteEventKind::NoteOn { pitch, .. } | NoteEventKind::NoteOff { pitch, .. } => {
                self.midi_notes[pitch as usize] =
                    self.midi_notes[pitch as usize].saturating_sub(1)
            }
            NoteEventKind::AllNotesOff => self.midi_notes.fill(0),
            _ => return,
        }
        self.midi_mask = self
            .midi_notes
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .fold(0_u16, |mask, (pitch, _)| mask | (1 << (pitch % 12)));
    }

    #[inline]
    fn process_maps(
        &mut self,
        dry: [f32; MAX_CHANNELS],
        color: f32,
    ) -> ([f32; MAX_CHANNELS], [f32; MAX_CHANNELS]) {
        let mask = self.active_mask();
        let resonance = (color - 1.0).max(0.0);
        if self.crossfade_remaining == 0 {
            let active = match self.quality {
                ColorizerQuality::Fast => &mut self.fast,
                ColorizerQuality::Clean => &mut self.clean,
            };
            return active.process_sample(dry, mask, self.morph, self.gate, resonance);
        }

        let (from, to) = match self.crossfade_from {
            ColorizerQuality::Fast => (&mut self.fast, &mut self.clean),
            ColorizerQuality::Clean => (&mut self.clean, &mut self.fast),
        };
        let (dry_from, wet_from) =
            from.process_sample(dry, mask, self.morph, self.gate, resonance);
        let (dry_to, wet_to) = to.process_sample(dry, mask, self.morph, self.gate, resonance);
        let progress =
            1.0 - self.crossfade_remaining as f32 / self.crossfade_total.max(1) as f32;
        self.crossfade_remaining -= 1;
        (
            std::array::from_fn(|channel| {
                dry_from[channel] + (dry_to[channel] - dry_from[channel]) * progress
            }),
            std::array::from_fn(|channel| {
                wet_from[channel] + (wet_to[channel] - wet_from[channel]) * progress
            }),
        )
    }
}

impl DspEffect for Colorizer {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.color = Smoother::new(self.color.target, self.sample_rate, 0.015);
        self.fast = ColorizerMap::new(self.sample_rate, ColorizerQuality::Fast);
        self.clean = ColorizerMap::new(self.sample_rate, ColorizerQuality::Clean);
        self.crossfade_total =
            ((self.sample_rate * QUALITY_CROSSFADE_MS / 1000.0) as u32).max(1);
        self.crossfade_remaining = 0;
        self.reset();
    }

    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        debug_assert!(events
            .windows(2)
            .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
        let mut event_index = 0;
        for frame in 0..frames.min(MAX_BLOCK_SIZE) {
            while self.midi_enabled
                && event_index < events.len()
                && events[event_index].sample_offset as usize == frame
            {
                self.handle_event(events[event_index].kind);
                event_index += 1;
            }
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let color = self.color.next().clamp(0.0, 2.0);
            let mix = color.min(1.0);
            let (delayed, mapped) = self.process_maps(dry, color);
            for channel in 0..MAX_CHANNELS {
                buffer.channels[channel][frame] = if self.bypassed {
                    delayed[channel]
                } else {
                    denormal(delayed[channel] + (mapped[channel] - delayed[channel]) * mix)
                };
            }
        }
    }

    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "color" => {
                self.color.set_target(value.clamp(0.0, 2.0));
                if value <= 1.0 {
                    self.fast.clear_resonance();
                    self.clean.clear_resonance();
                }
            }
            "quality" | "mapQuality" => self.set_quality(if value >= 0.5 {
                ColorizerQuality::Clean
            } else {
                ColorizerQuality::Fast
            }),
            "morph" | "transient" => self.morph = value.clamp(0.0, 1.0),
            "gate" => self.gate = value.clamp(0.0, 1.0),
            "midi" => self.midi_enabled = value >= 0.5,
            // Project compatibility: the previous Map engine stored its wet
            // amount as `mix`. It maps directly to COLOR's 0..100 range.
            "mix" => self.color.set_target(value.clamp(0.0, 1.0)),
            _ if id.starts_with("pitch") => {
                if let Ok(pitch) = id[5..].parse::<usize>() {
                    if pitch < 12 {
                        if value >= 0.5 {
                            self.manual_mask |= 1 << pitch;
                        } else {
                            self.manual_mask &= !(1 << pitch);
                        }
                    }
                }
            }
            // Retired Live-engine parameters are intentionally accepted as
            // no-ops so old sessions load without introducing a second DSP.
            "resonance" | "decay" | "depth" => {}
            _ => {}
        }
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }

    fn reset(&mut self) {
        self.midi_notes.fill(0);
        self.midi_mask = 0;
        self.fast.reset();
        self.clean.reset();
        self.crossfade_remaining = 0;
    }

    fn tail_samples(&self) -> usize {
        let base = self.quality.fft_size() + self.quality.hop_size();
        if self.color.target > 1.0 {
            base + (self.sample_rate * 2.0) as usize
        } else {
            base
        }
    }

    fn latency_samples(&self) -> usize {
        self.quality.fft_size()
    }

    fn wants_midi(&self) -> bool {
        self.midi_enabled
    }

    fn runtime_capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::finite_tail(self.tail_samples())
    }

    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.active_map().meter)
    }
}
