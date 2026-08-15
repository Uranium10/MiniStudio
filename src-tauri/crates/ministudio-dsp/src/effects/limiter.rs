#[derive(Clone, Copy)]
enum LimiterMode {
    Clean,
    Transparent,
    Punch,
    Loud,
}

/// ITU-R BS.1770 K-weighted loudness meter. The 400 ms and 3 s windows are
/// maintained as rolling sums; integrated loudness uses overlapping 400 ms
/// blocks and the absolute/relative gates from the recommendation.
struct LoudnessMeter {
    shelf: Biquad,
    high_pass: Biquad,
    powers: Vec<f32>,
    position: usize,
    count: usize,
    momentary_samples: usize,
    short_samples: usize,
    momentary_sum: f64,
    short_sum: f64,
    hop_samples: usize,
    hop_count: usize,
    blocks: Vec<f32>,
    block_count: usize,
    momentary_lufs: f32,
    short_lufs: f32,
    integrated_lufs: f32,
}
impl LoudnessMeter {
    fn new() -> Self {
        Self {
            shelf: Biquad::new(),
            high_pass: Biquad::new(),
            powers: Vec::new(),
            position: 0,
            count: 0,
            momentary_samples: 19_200,
            short_samples: 144_000,
            momentary_sum: 0.0,
            short_sum: 0.0,
            hop_samples: 4_800,
            hop_count: 0,
            // One hour of 100 ms blocks, allocated off the audio callback.
            blocks: vec![0.0; 36_000],
            block_count: 0,
            momentary_lufs: -120.0,
            short_lufs: -120.0,
            integrated_lufs: -120.0,
        }
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.momentary_samples = (sample_rate * 0.4).round().max(1.0) as usize;
        self.short_samples = (sample_rate * 3.0).round().max(1.0) as usize;
        self.hop_samples = (sample_rate * 0.1).round().max(1.0) as usize;
        self.powers.resize(self.short_samples, 0.0);
        self.shelf.configure(
            FilterKind::HighShelf,
            1_681.9745,
            3.999_843_8,
            0.707_175_25,
            sample_rate,
        );
        self.high_pass
            .configure(FilterKind::HighPass, 38.13547, 0.0, 0.500_327, sample_rate);
        self.reset();
    }
    #[inline(always)]
    fn push(&mut self, left: f32, right: f32) {
        let wl = self.high_pass.process(0, self.shelf.process(0, left));
        let wr = self.high_pass.process(1, self.shelf.process(1, right));
        let power = wl * wl + wr * wr;
        let old_short = self.powers[self.position];
        self.short_sum += f64::from(power - old_short);
        if self.count >= self.momentary_samples {
            let old_momentary = self.powers[(self.position + self.short_samples
                - self.momentary_samples)
                % self.short_samples];
            self.momentary_sum += f64::from(power - old_momentary);
        } else {
            self.momentary_sum += f64::from(power);
        }
        self.powers[self.position] = power;
        self.position = (self.position + 1) % self.short_samples;
        self.count = (self.count + 1).min(self.short_samples);
        self.hop_count += 1;
        if self.hop_count >= self.hop_samples {
            self.hop_count = 0;
            let momentary_count = self.count.min(self.momentary_samples).max(1);
            self.momentary_lufs = power_to_lufs(self.momentary_sum / momentary_count as f64);
            self.short_lufs = power_to_lufs(self.short_sum / self.count.max(1) as f64);
            if self.count >= self.momentary_samples {
                let energy = (self.momentary_sum / self.momentary_samples as f64) as f32;
                let index = self.block_count % self.blocks.len();
                self.blocks[index] = energy;
                self.block_count += 1;
                // Re-evaluate the relative gate once per second, not per sample.
                if self.block_count % 10 == 0 {
                    self.integrated_lufs =
                        integrated_lufs(&self.blocks, self.block_count.min(self.blocks.len()));
                }
            }
        }
    }
    fn reset(&mut self) {
        self.shelf.reset();
        self.high_pass.reset();
        self.powers.fill(0.0);
        self.position = 0;
        self.count = 0;
        self.momentary_sum = 0.0;
        self.short_sum = 0.0;
        self.hop_count = 0;
        self.blocks.fill(0.0);
        self.block_count = 0;
        self.momentary_lufs = -120.0;
        self.short_lufs = -120.0;
        self.integrated_lufs = -120.0;
    }
}

pub struct MasteringLimiter {
    sample_rate: f32,
    input: Smoother,
    output: Smoother,
    release_ms: f32,
    stereo_link: f32,
    true_peak: bool,
    mode: LimiterMode,
    delay: [Vec<f32>; MAX_CHANNELS],
    delay_position: usize,
    lookahead: usize,
    detector_history: [[f32; 4]; MAX_CHANNELS],
    output_history: [[f32; 4]; MAX_CHANNELS],
    envelope: [f32; MAX_CHANNELS],
    hold: [usize; MAX_CHANNELS],
    input_peak: f32,
    output_peak: f32,
    true_peak_level: f32,
    gain_reduction_db: f32,
    loudness: LoudnessMeter,
    bypassed: bool,
}
impl MasteringLimiter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            input: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(db_to_gain(-1.0), 48_000.0, 0.01),
            release_ms: 120.0,
            stereo_link: 1.0,
            true_peak: true,
            mode: LimiterMode::Clean,
            delay: [Vec::new(), Vec::new()],
            delay_position: 0,
            lookahead: 240,
            detector_history: [[0.0; 4]; MAX_CHANNELS],
            output_history: [[0.0; 4]; MAX_CHANNELS],
            envelope: [1.0; MAX_CHANNELS],
            hold: [0; MAX_CHANNELS],
            input_peak: 0.0,
            output_peak: 0.0,
            true_peak_level: 0.0,
            gain_reduction_db: 0.0,
            loudness: LoudnessMeter::new(),
            bypassed: false,
        }
    }
    fn release_coefficient(&self) -> f32 {
        let scale = match self.mode {
            LimiterMode::Transparent => 1.5,
            LimiterMode::Punch => 0.55,
            LimiterMode::Loud => 0.7,
            LimiterMode::Clean => 1.0,
        };
        (-1.0 / (self.release_ms.max(5.0) * scale * 0.001 * self.sample_rate)).exp()
    }
}
impl DspEffect for MasteringLimiter {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.lookahead = (sample_rate * 0.005).round().max(1.0) as usize;
        self.delay = [vec![0.0; self.lookahead + 1], vec![0.0; self.lookahead + 1]];
        self.input = Smoother::new(self.input.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        self.loudness.prepare(sample_rate);
        self.reset();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        let release = self.release_coefficient();
        let meter_decay = 10.0_f32.powf(-18.0 / 20.0 / self.sample_rate);
        for frame in 0..frames {
            let input_gain = if self.bypassed {
                1.0
            } else {
                self.input.next()
            };
            let ceiling = if self.bypassed {
                1.0
            } else {
                self.output.next().clamp(0.000_1, 1.0)
            };
            // A small reconstruction guard keeps the interpolated waveform below
            // the requested dBTP ceiling instead of merely clipping sample peaks.
            let detector_ceiling = if self.true_peak {
                ceiling * 0.9975
            } else {
                ceiling
            };
            let mut input = [0.0; MAX_CHANNELS];
            let mut independent = [1.0; MAX_CHANNELS];
            for channel in 0..MAX_CHANNELS {
                input[channel] = buffer.channels[channel][frame] * input_gain;
                self.input_peak = self
                    .input_peak
                    .mul_add(meter_decay, 0.0)
                    .max(input[channel].abs());
                let history = &mut self.detector_history[channel];
                history.rotate_left(1);
                history[3] = input[channel];
                let peak = if self.true_peak {
                    interpolated_peak(*history, 4)
                } else {
                    input[channel].abs()
                };
                independent[channel] = (detector_ceiling / peak.max(detector_ceiling)).min(1.0);
            }
            let linked = independent[0].min(independent[1]);
            for channel in 0..MAX_CHANNELS {
                let target = if self.bypassed {
                    1.0
                } else {
                    independent[channel] + (linked - independent[channel]) * self.stereo_link
                };
                if target < self.envelope[channel] {
                    self.envelope[channel] = target;
                    self.hold[channel] = self.lookahead;
                } else if self.hold[channel] > 0 {
                    self.hold[channel] -= 1;
                } else {
                    self.envelope[channel] = target + (self.envelope[channel] - target) * release;
                }
                self.delay[channel][self.delay_position] = input[channel];
                let read = (self.delay_position + 1) % self.delay[channel].len();
                let mut sample = self.delay[channel][read] * self.envelope[channel];
                if !self.bypassed && matches!(self.mode, LimiterMode::Loud) {
                    let drive = 1.35;
                    sample = (sample * drive).tanh() / drive.tanh();
                }
                // The final guard is intentionally non-colouring in normal operation;
                // it only catches numerical/sample peaks missed by interpolation.
                if !self.bypassed {
                    sample = sample.clamp(-ceiling, ceiling)
                }
                buffer.channels[channel][frame] = sample;
                self.output_peak = self.output_peak.mul_add(meter_decay, 0.0).max(sample.abs());
                let history = &mut self.output_history[channel];
                history.rotate_left(1);
                history[3] = sample;
                self.true_peak_level = self
                    .true_peak_level
                    .mul_add(meter_decay, 0.0)
                    .max(interpolated_peak(*history, 4));
            }
            self.delay_position = (self.delay_position + 1) % self.delay[0].len();
            let minimum_gain = self.envelope[0].min(self.envelope[1]);
            self.gain_reduction_db =
                (-20.0 * minimum_gain.max(1e-9).log10()).max(self.gain_reduction_db * meter_decay);
            self.loudness
                .push(buffer.channels[0][frame], buffer.channels[1][frame]);
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "inputDb" | "inputGainDb" => {
                self.input.set_target(db_to_gain(value.clamp(-24.0, 24.0)))
            }
            "outputDb" | "outputGainDb" | "ceilingDb" => {
                self.output.set_target(db_to_gain(value.clamp(-24.0, 0.0)))
            }
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "release" => self.release_ms = (value * 1_000.0).clamp(5.0, 2_000.0),
            "stereoLink" => self.stereo_link = value.clamp(0.0, 1.0),
            "truePeak" => self.true_peak = value >= 0.5,
            "algorithm" | "mode" => {
                self.mode = match value.round() as i32 {
                    1 => LimiterMode::Transparent,
                    2 => LimiterMode::Punch,
                    3 => LimiterMode::Loud,
                    _ => LimiterMode::Clean,
                }
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for channel in &mut self.delay {
            channel.fill(0.0)
        }
        self.delay_position = 0;
        self.detector_history = [[0.0; 4]; MAX_CHANNELS];
        self.output_history = [[0.0; 4]; MAX_CHANNELS];
        self.envelope = [1.0; MAX_CHANNELS];
        self.hold = [0; MAX_CHANNELS];
        self.input_peak = 0.0;
        self.output_peak = 0.0;
        self.true_peak_level = 0.0;
        self.gain_reduction_db = 0.0;
        self.loudness.reset();
    }
    fn latency_samples(&self) -> usize {
        self.lookahead
    }
    fn limiter_metrics(&self) -> Option<[f32; LIMITER_METER_VALUES]> {
        Some([
            amplitude_to_db(self.input_peak),
            amplitude_to_db(self.output_peak),
            self.gain_reduction_db,
            amplitude_to_db(self.true_peak_level),
            self.loudness.momentary_lufs,
            self.loudness.short_lufs,
            self.loudness.integrated_lufs,
        ])
    }
}
