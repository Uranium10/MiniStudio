pub struct UpwardCompressor {
    sample_rate: f32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    range_db: f32,
    stereo_link: f32,
    mix: Smoother,
    output: Smoother,
    detector: [f32; MAX_CHANNELS],
    gain: [f32; MAX_CHANNELS],
    bypassed: bool,
}
impl UpwardCompressor {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            threshold_db: -32.0,
            ratio: 3.0,
            attack_ms: 35.0,
            release_ms: 240.0,
            range_db: 12.0,
            stereo_link: 1.0,
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            detector: [0.0; MAX_CHANNELS],
            gain: [1.0; MAX_CHANNELS],
            bypassed: false,
        }
    }
    fn requested_gain(&self, level: f32) -> f32 {
        let level_db = amplitude_to_db(level);
        if level_db >= self.threshold_db {
            return 1.0;
        }
        let gain_db =
            ((self.threshold_db - level_db) * (1.0 - self.ratio.recip())).clamp(0.0, self.range_db);
        db_to_gain(gain_db)
    }
}
impl DspEffect for UpwardCompressor {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.mix = Smoother::new(self.mix.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        let detector_coefficient = (-1.0 / (0.01 * self.sample_rate)).exp();
        let attack = (-1.0 / (self.attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let release = (-1.0 / (self.release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();
        for frame in 0..frames {
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let mut target = [1.0; MAX_CHANNELS];
            for channel in 0..MAX_CHANNELS {
                self.detector[channel] = dry[channel] * dry[channel]
                    + (self.detector[channel] - dry[channel] * dry[channel]) * detector_coefficient;
                target[channel] = self.requested_gain(self.detector[channel].sqrt());
            }
            let linked = target[0].min(target[1]);
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                let requested = target[channel] + (linked - target[channel]) * self.stereo_link;
                let coefficient = if requested > self.gain[channel] {
                    attack
                } else {
                    release
                };
                self.gain[channel] = requested + (self.gain[channel] - requested) * coefficient;
                let wet = dry[channel] * self.gain[channel];
                buffer.channels[channel][frame] =
                    denormal((dry[channel] * (1.0 - mix) + wet * mix) * output);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "threshold" | "thresholdDb" => self.threshold_db = value.clamp(-72.0, -6.0),
            "ratio" => self.ratio = value.clamp(1.0, 20.0),
            "attackMs" => self.attack_ms = value.clamp(0.1, 500.0),
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "rangeDb" | "range" => self.range_db = value.clamp(0.0, 36.0),
            "stereoLink" => self.stereo_link = value.clamp(0.0, 1.0),
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.detector = [0.0; MAX_CHANNELS];
        self.gain = [1.0; MAX_CHANNELS]
    }
}
