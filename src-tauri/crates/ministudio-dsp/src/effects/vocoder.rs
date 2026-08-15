struct VocoderBand {
    analysis: Biquad,
    carrier: Biquad,
    envelope: f32,
}
impl VocoderBand {
    fn new() -> Self {
        Self {
            analysis: Biquad::new(),
            carrier: Biquad::new(),
            envelope: 0.0,
        }
    }
}

/// Constant-Q channel vocoder. Sidechain mode analyses the external modulator
/// and filters the main input; oscillator/noise modes analyse the main input
/// and use the selected internal excitation source.
pub struct Vocoder {
    sample_rate: f32,
    source: u8,
    band_count: usize,
    pitch_hz: f32,
    attack_ms: f32,
    release_ms: f32,
    formant_shift: f32,
    bandwidth: f32,
    mix: Smoother,
    output: Smoother,
    phase: f32,
    noise: u32,
    bands: Vec<VocoderBand>,
    bypassed: bool,
}
impl Vocoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            source: 3,
            band_count: 16,
            pitch_hz: 110.0,
            attack_ms: 5.0,
            release_ms: 90.0,
            formant_shift: 0.0,
            bandwidth: 1.0,
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            phase: 0.0,
            noise: 0x9e37_79b9,
            bands: (0..24).map(|_| VocoderBand::new()).collect(),
            bypassed: false,
        }
    }
    fn configure_bands(&mut self) {
        let ratio = 2.0_f32.powf(self.formant_shift / 12.0);
        let q = (self.band_count as f32 / 4.0 * self.bandwidth.recip()).clamp(1.0, 12.0);
        for (index, band) in self.bands.iter_mut().enumerate() {
            let t = index as f32 / (self.band_count.saturating_sub(1)).max(1) as f32;
            let carrier_frequency = 80.0 * (12_000.0_f32 / 80.0).powf(t);
            let analysis_frequency =
                (carrier_frequency / ratio).clamp(40.0, self.sample_rate * 0.45);
            band.analysis.configure(
                FilterKind::BandPass,
                analysis_frequency,
                0.0,
                q,
                self.sample_rate,
            );
            band.carrier.configure(
                FilterKind::BandPass,
                carrier_frequency,
                0.0,
                q,
                self.sample_rate,
            );
        }
    }
    #[inline(always)]
    fn oscillator(&mut self) -> f32 {
        self.phase += self.pitch_hz / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0
        }
        match self.source {
            1 => (2.0 * PI * self.phase).sin(),
            2 => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => self.phase * 2.0 - 1.0,
            4 => {
                self.noise ^= self.noise << 13;
                self.noise ^= self.noise >> 17;
                self.noise ^= self.noise << 5;
                self.noise as f32 / u32::MAX as f32 * 2.0 - 1.0
            }
            _ => 0.0,
        }
    }
}
impl DspEffect for Vocoder {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.mix = Smoother::new(self.mix.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        self.configure_bands();
        self.reset();
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(events, buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        if self.bypassed {
            return;
        }
        let attack = (-1.0 / (self.attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let release = (-1.0 / (self.release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();
        let normalization = 3.5 / (self.band_count as f32).sqrt();
        for frame in 0..frames {
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let internal = self.oscillator();
            let (modulator, carrier) = if self.source == 0 {
                if let Some(external) = sidechain {
                    (
                        (external.channels[0][frame] + external.channels[1][frame]) * 0.5,
                        dry,
                    )
                } else {
                    ((dry[0] + dry[1]) * 0.5, [internal; MAX_CHANNELS])
                }
            } else {
                ((dry[0] + dry[1]) * 0.5, [internal; MAX_CHANNELS])
            };
            let mut wet = [0.0; MAX_CHANNELS];
            for band in self.bands.iter_mut().take(self.band_count) {
                let detected = band.analysis.process(0, modulator).abs();
                let coefficient = if detected > band.envelope {
                    attack
                } else {
                    release
                };
                band.envelope = detected + (band.envelope - detected) * coefficient;
                for channel in 0..MAX_CHANNELS {
                    wet[channel] += band.carrier.process(channel, carrier[channel]) * band.envelope;
                }
            }
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                buffer.channels[channel][frame] = denormal(
                    (dry[channel] * (1.0 - mix) + wet[channel] * normalization * mix) * output,
                )
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "source" | "modulator" => self.source = value.round().clamp(0.0, 4.0) as u8,
            "bands" => {
                self.band_count = match value.round() as usize {
                    0..=10 => 8,
                    11..=14 => 12,
                    15..=20 => 16,
                    _ => 24,
                };
                self.configure_bands()
            }
            "pitchHz" | "frequency" => self.pitch_hz = value.clamp(20.0, 2_000.0),
            "attackMs" => self.attack_ms = value.clamp(0.1, 200.0),
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "formantShift" => {
                self.formant_shift = value.clamp(-24.0, 24.0);
                self.configure_bands()
            }
            "bandwidth" => {
                self.bandwidth = value.clamp(0.5, 2.0);
                self.configure_bands()
            }
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.analysis.reset();
            band.carrier.reset();
            band.envelope = 0.0
        }
        self.phase = 0.0;
    }
}
