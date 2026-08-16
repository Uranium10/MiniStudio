const DYNAMICS_HISTORY_POINTS: usize = DISTORTION_SPECTRUM_BINS / 2;

/// Fixed-capacity, control-rate I/O history shared by both dynamics processors.
/// One peak pair is committed every 10 ms; the audio callback performs O(1)
/// work per sample and never allocates, locks, or shifts an array.
struct DynamicsHistory {
    input: [f32; DYNAMICS_HISTORY_POINTS],
    output: [f32; DYNAMICS_HISTORY_POINTS],
    write: usize,
    samples_until_commit: usize,
    commit_interval: usize,
    input_peak: f32,
    output_peak: f32,
}
impl DynamicsHistory {
    fn new() -> Self {
        Self {
            input: [0.0; DYNAMICS_HISTORY_POINTS],
            output: [0.0; DYNAMICS_HISTORY_POINTS],
            write: 0,
            samples_until_commit: 480,
            commit_interval: 480,
            input_peak: 0.0,
            output_peak: 0.0,
        }
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.commit_interval = (sample_rate / 100.0).round().max(1.0) as usize;
        self.reset();
    }
    #[inline(always)]
    fn observe(&mut self, input: f32, output: f32) {
        self.input_peak = self.input_peak.max(input.abs());
        self.output_peak = self.output_peak.max(output.abs());
        self.samples_until_commit -= 1;
        if self.samples_until_commit != 0 {
            return;
        }
        self.input[self.write] = self.input_peak;
        self.output[self.write] = self.output_peak;
        self.write = increment_wrap(self.write, DYNAMICS_HISTORY_POINTS);
        self.samples_until_commit = self.commit_interval;
        self.input_peak = 0.0;
        self.output_peak = 0.0;
    }
    fn snapshot(&self) -> [f32; DISTORTION_SPECTRUM_BINS] {
        let mut output = [0.0; DISTORTION_SPECTRUM_BINS];
        for index in 0..DYNAMICS_HISTORY_POINTS {
            let source = (self.write + index) % DYNAMICS_HISTORY_POINTS;
            output[index] = self.input[source];
            output[DYNAMICS_HISTORY_POINTS + index] = self.output[source];
        }
        output
    }
    fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.write = 0;
        self.samples_until_commit = self.commit_interval;
        self.input_peak = 0.0;
        self.output_peak = 0.0;
    }
}

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
    history: DynamicsHistory,
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
            history: DynamicsHistory::new(),
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
        self.sr = s;
        self.history.prepare(s);
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
            let input_peak = b.channels[0][i].abs().max(b.channels[1][i].abs());
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
            self.history.observe(
                input_peak,
                b.channels[0][i].abs().max(b.channels[1][i].abs()),
            );
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
        self.gain_reduction_db = 0.0;
        self.history.reset()
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.history.snapshot())
    }
}
