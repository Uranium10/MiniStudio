/// A fourth-order Linkwitz-Riley crossover made from two cascaded Butterworth
/// biquads per branch. Its low and high outputs have matching phase and sum
/// without a level bump at the crossover frequency.
struct Crossover {
    low: [Biquad; 2],
    high: [Biquad; 2],
    frequency: f32,
    sample_rate: f32,
}
impl Crossover {
    fn new(frequency: f32) -> Self {
        let mut crossover = Self {
            low: [Biquad::new(), Biquad::new()],
            high: [Biquad::new(), Biquad::new()],
            frequency,
            sample_rate: 48_000.0,
        };
        crossover.configure(frequency, 48_000.0);
        crossover
    }
    fn configure(&mut self, frequency: f32, sample_rate: f32) {
        self.frequency = frequency;
        self.sample_rate = sample_rate;
        for section in &mut self.low {
            section.configure(
                FilterKind::LowPass,
                frequency,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                sample_rate,
            )
        }
        for section in &mut self.high {
            section.configure(
                FilterKind::HighPass,
                frequency,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                sample_rate,
            )
        }
    }
    fn process(&mut self, channel: usize, input: f32) -> (f32, f32) {
        let mut low = input;
        let mut high = input;
        for section in &mut self.low {
            low = section.process(channel, low)
        }
        for section in &mut self.high {
            high = section.process(channel, high)
        }
        (low, high)
    }
    fn reset(&mut self) {
        for section in self.low.iter_mut().chain(self.high.iter_mut()) {
            section.reset()
        }
    }
}

/// Three-band dynamics processor. The extra high crossover on the low branch
/// is a phase-compensation all-pass (LP + HP), keeping all three bands aligned
/// when they are recombined.
pub struct MultibandCompressor {
    sample_rate: f32,
    split_low: f32,
    split_high: f32,
    low_split: Crossover,
    high_split: Crossover,
    low_phase: Crossover,
    compressors: [Compressor; 3],
    dry: AudioBuffer,
    bands: [AudioBuffer; 3],
    output: Smoother,
    mix: Smoother,
    levels: [[f32; MAX_CHANNELS]; 3],
    bypassed: bool,
}
impl MultibandCompressor {
    pub fn new() -> Self {
        let mut compressors = std::array::from_fn(|_| Compressor::new());
        let defaults = [
            (-24.0, 3.0, 30.0, 250.0),
            (-20.0, 2.5, 15.0, 180.0),
            (-18.0, 2.0, 6.0, 120.0),
        ];
        for (compressor, (threshold, ratio, attack, release)) in
            compressors.iter_mut().zip(defaults)
        {
            compressor.threshold = threshold;
            compressor.ratio = ratio;
            compressor.attack = attack;
            compressor.release = release;
            compressor.knee = 8.0;
        }
        Self {
            sample_rate: 48_000.0,
            split_low: 150.0,
            split_high: 2_500.0,
            low_split: Crossover::new(150.0),
            high_split: Crossover::new(2_500.0),
            low_phase: Crossover::new(2_500.0),
            compressors,
            dry: AudioBuffer::new(),
            bands: std::array::from_fn(|_| AudioBuffer::new()),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            levels: [[0.0; MAX_CHANNELS]; 3],
            bypassed: false,
        }
    }
    fn update_crossovers(&mut self) {
        self.low_split.configure(self.split_low, self.sample_rate);
        self.high_split.configure(self.split_high, self.sample_rate);
        self.low_phase.configure(self.split_high, self.sample_rate);
    }
    fn set_band_param(&mut self, band: usize, param: &str, value: f32) {
        if let Some(compressor) = self.compressors.get_mut(band) {
            compressor.set_param(param, value)
        }
    }
}
impl DspEffect for MultibandCompressor {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate = sample_rate;
        self.update_crossovers();
        for compressor in &mut self.compressors {
            compressor.prepare(sample_rate, max_block, channels)
        }
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            self.levels = [[0.0; MAX_CHANNELS]; 3];
            return;
        }
        for channel in 0..MAX_CHANNELS {
            for frame in 0..frames {
                let input = buffer.channels[channel][frame];
                self.dry.channels[channel][frame] = input;
                let (low, upper) = self.low_split.process(channel, input);
                let (mid, high) = self.high_split.process(channel, upper);
                let (low_late, low_early) = self.low_phase.process(channel, low);
                self.bands[0].channels[channel][frame] = low_late + low_early;
                self.bands[1].channels[channel][frame] = mid;
                self.bands[2].channels[channel][frame] = high;
            }
        }
        for (compressor, band) in self.compressors.iter_mut().zip(self.bands.iter_mut()) {
            compressor.process(&[], band, frames)
        }
        let mut peaks = [[0.0_f32; MAX_CHANNELS]; 3];
        for frame in 0..frames {
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                let low = self.bands[0].channels[channel][frame];
                let mid = self.bands[1].channels[channel][frame];
                let high = self.bands[2].channels[channel][frame];
                peaks[0][channel] = peaks[0][channel].max(low.abs());
                peaks[1][channel] = peaks[1][channel].max(mid.abs());
                peaks[2][channel] = peaks[2][channel].max(high.abs());
                let wet = low + mid + high;
                buffer.channels[channel][frame] =
                    (self.dry.channels[channel][frame] * (1.0 - mix) + wet * mix) * output
            }
        }
        let decay = 10.0_f32.powf(-12.0 * frames as f32 / self.sample_rate / 20.0);
        for (levels, peaks) in self.levels.iter_mut().zip(peaks) {
            for channel in 0..MAX_CHANNELS {
                levels[channel] = peaks[channel].max(levels[channel] * decay)
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            // Keep the requested low band within 20-150 Hz. The second split
            // defaults to 2.5 kHz while retaining a useful editable range.
            "splitLow" => {
                self.split_low = value.clamp(20.0, 150.0);
                self.update_crossovers()
            }
            "splitHigh" => {
                self.split_high = value.clamp(300.0, 16_000.0);
                self.update_crossovers()
            }
            "knee" | "kneeDb" => {
                for compressor in &mut self.compressors {
                    compressor.set_param("knee", value)
                }
            }
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            _ => {
                for (prefix, band) in [("low", 0), ("mid", 1), ("high", 2)] {
                    if let Some(param) = id.strip_prefix(prefix) {
                        let normalized = match param {
                            "Threshold" => "threshold",
                            "Ratio" => "ratio",
                            "Attack" => "attack",
                            "Release" => "release",
                            "MakeupDb" => "makeupDb",
                            _ => return,
                        };
                        self.set_band_param(band, normalized, value);
                        return;
                    }
                }
            }
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.low_split.reset();
        self.high_split.reset();
        self.low_phase.reset();
        for compressor in &mut self.compressors {
            compressor.reset()
        }
        self.levels = [[0.0; MAX_CHANNELS]; 3]
    }
    fn multiband_levels(&self) -> Option<[[f32; MAX_CHANNELS]; 3]> {
        Some(self.levels)
    }
}
