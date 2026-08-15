pub struct Compressor {
    sr: f32,
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    knee: f32,
    makeup: f32,
    envelope: [f32; MAX_CHANNELS],
    rms: [f32; MAX_CHANNELS],
    detection_rms: bool,
    stereo_link: bool,
    auto_makeup: bool,
    gain_reduction_db: f32,
    bypassed: bool,
}
impl Compressor {
    pub fn new() -> Self {
        Self {
            sr: 48000.0,
            threshold: -18.0,
            ratio: 3.0,
            attack: 10.0,
            release: 200.0,
            knee: 12.0,
            makeup: 0.0,
            envelope: [1.0; MAX_CHANNELS],
            rms: [0.0; MAX_CHANNELS],
            detection_rms: true,
            stereo_link: true,
            auto_makeup: false,
            gain_reduction_db: 0.0,
            bypassed: false,
        }
    }
    fn gain_db(&self, input: f32) -> f32 {
        let x = input - self.threshold;
        let compressed = if self.knee > 0.0 && x.abs() < self.knee / 2.0 {
            (1.0 / self.ratio - 1.0) * (x + self.knee / 2.0).powi(2) / (2.0 * self.knee)
        } else if x > 0.0 {
            (1.0 / self.ratio - 1.0) * x
        } else {
            0.0
        };
        let automatic = if self.auto_makeup {
            -self.threshold * (1.0 - 1.0 / self.ratio) * 0.5
        } else {
            0.0
        };
        compressed + self.makeup + automatic
    }
}
impl DspEffect for Compressor {
    fn prepare(&mut self, s: f32, _: usize, _: usize) {
        self.sr = s
    }
    fn process(&mut self, events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        self.process_with_sidechain(events, b, None, n)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
        b: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        n: usize,
    ) {
        if self.bypassed {
            return;
        }
        let attack_coefficient = (-1.0 / (self.attack.max(0.01) * 0.001 * self.sr)).exp();
        let release_coefficient = (-1.0 / (self.release.max(0.01) * 0.001 * self.sr)).exp();
        for i in 0..n {
            let mut detector = [0.0; MAX_CHANNELS];
            for ch in 0..MAX_CHANNELS {
                let peak = sidechain
                    .map_or(b.channels[ch][i], |detector| detector.channels[ch][i])
                    .abs();
                if self.detection_rms {
                    self.rms[ch] = 0.99 * self.rms[ch] + 0.01 * peak * peak;
                    detector[ch] = self.rms[ch].sqrt()
                } else {
                    detector[ch] = peak
                }
            }
            if self.stereo_link {
                let linked = detector[0].max(detector[1]);
                let db = 20.0 * linked.max(1e-10).log10();
                let target = db_to_gain(self.gain_db(db));
                let coefficient = if target < self.envelope[0] {
                    attack_coefficient
                } else {
                    release_coefficient
                };
                let envelope = target + (self.envelope[0] - target) * coefficient;
                self.envelope = [envelope; MAX_CHANNELS];
                b.channels[0][i] *= envelope;
                b.channels[1][i] *= envelope
            } else {
                for ch in 0..MAX_CHANNELS {
                    let db = 20.0 * detector[ch].max(1e-10).log10();
                    let target = db_to_gain(self.gain_db(db));
                    let coefficient = if target < self.envelope[ch] {
                        attack_coefficient
                    } else {
                        release_coefficient
                    };
                    self.envelope[ch] = target + (self.envelope[ch] - target) * coefficient;
                    b.channels[ch][i] *= self.envelope[ch]
                }
            }
        }
        self.gain_reduction_db = -20.0 * self.envelope[0].max(self.envelope[1]).max(1e-10).log10()
    }
    fn set_param(&mut self, id: &str, v: f32) {
        match id {
            "threshold" | "thresholdDb" => self.threshold = v,
            "ratio" => self.ratio = v.max(1.0),
            "attack" => self.attack = v * 1000.0,
            "attackMs" => self.attack = v,
            "release" => self.release = v * 1000.0,
            "releaseMs" => self.release = v,
            "knee" | "kneeDb" => self.knee = v,
            "makeupDb" => self.makeup = v,
            "autoMakeup" | "auto_makeup" => self.auto_makeup = v >= 0.5,
            "detection" => self.detection_rms = v >= 0.5,
            "stereoLink" | "stereo_link" => self.stereo_link = v >= 0.5,
            _ => {}
        }
    }
    fn set_bypassed(&mut self, v: bool) {
        self.bypassed = v
    }
    fn reset(&mut self) {
        self.envelope = [1.0; MAX_CHANNELS];
        self.rms = [0.0; MAX_CHANNELS];
        self.gain_reduction_db = 0.0
    }
}
