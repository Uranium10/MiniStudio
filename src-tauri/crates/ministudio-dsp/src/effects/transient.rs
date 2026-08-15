/// Zero-latency, allocation-free stereo transient shaper.
///
/// A linked pair of peak envelopes separates fast edge energy from the
/// slower body of the sound.  Gain is calculated in the log domain so the
/// response remains stable across input levels and both channels receive the
/// exact same gain, preserving the stereo image.
pub struct TransientShaper {
    sample_rate: f32,
    attack: f32,
    sustain: f32,
    threshold_db: f32,
    speed: f32,
    clip: bool,
    fast_envelope: f32,
    slow_envelope: f32,
    gate_envelope: f32,
    gain_envelope: f32,
    target_gain: f32,
    bypassed: bool,
}

impl TransientShaper {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            attack: 0.0,
            sustain: 0.0,
            threshold_db: -36.0,
            speed: 0.5,
            clip: false,
            fast_envelope: 0.0,
            slow_envelope: 0.0,
            gate_envelope: 0.0,
            gain_envelope: 1.0,
            target_gain: 1.0,
            bypassed: false,
        }
    }

    #[inline(always)]
    fn coefficient(&self, milliseconds: f32) -> f32 {
        (-1.0 / (milliseconds.max(0.01) * 0.001 * self.sample_rate)).exp()
    }

    #[inline(always)]
    fn follow(current: f32, target: f32, attack: f32, release: f32) -> f32 {
        let coefficient = if target > current { attack } else { release };
        target + (current - target) * coefficient
    }

    #[inline(always)]
    fn soft_clip(sample: f32) -> f32 {
        // Unity below -0.9 dBFS, then a smooth rational knee.  This keeps the
        // clip switch transparent until a boost actually approaches the rail.
        const KNEE: f32 = 0.9;
        let magnitude = sample.abs();
        if magnitude <= KNEE {
            sample
        } else {
            sample.signum() * (KNEE + (magnitude - KNEE) / (1.0 + (magnitude - KNEE) / (1.0 - KNEE)))
        }
    }
}

impl DspEffect for TransientShaper {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
    }

    fn process(&mut self, _: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed || (self.attack.abs() < 1e-6 && self.sustain.abs() < 1e-6 && !self.clip) {
            return;
        }

        let speed = self.speed.clamp(0.0, 1.0);
        // Slow speed is forgiving and musical; fast speed catches sharp drum
        // edges.  Defaults deliberately sit in the broad "balanced" region.
        let fast_attack = self.coefficient(0.25 + (1.0 - speed) * 3.75);
        let fast_release = self.coefficient(22.0 + (1.0 - speed) * 88.0);
        let slow_attack = self.coefficient(9.0 + (1.0 - speed) * 28.0);
        let slow_release = self.coefficient(120.0 + (1.0 - speed) * 430.0);
        let gate_attack = self.coefficient(2.0);
        let gate_release = self.coefficient(90.0 + (1.0 - speed) * 180.0);
        let gain_attack = self.coefficient(0.35 + (1.0 - speed) * 1.5);
        let gain_release = self.coefficient(18.0 + (1.0 - speed) * 90.0);
        let threshold = db_to_gain(self.threshold_db);

        for frame in 0..frames {
            let detector = buffer.channels[0][frame]
                .abs()
                .max(buffer.channels[1][frame].abs())
                .max(1e-12);
            self.fast_envelope = Self::follow(self.fast_envelope, detector, fast_attack, fast_release);
            self.slow_envelope = Self::follow(self.slow_envelope, detector, slow_attack, slow_release);

            // Detector ratios avoid log10 on the realtime path. A ratio of
            // four is 12 dB, matching the soft gate and contrast ranges.
            let gate_target = ((self.slow_envelope / threshold.max(1e-12) - 1.0) / 3.0).clamp(0.0, 1.0);
            self.gate_envelope = Self::follow(self.gate_envelope, gate_target, gate_attack, gate_release);

            let contrast = self.fast_envelope / self.slow_envelope.max(1e-12);
            let transient = ((contrast - 1.0) / 3.0).clamp(0.0, 1.0) * self.gate_envelope;
            let body = (1.0 - transient).powi(2) * self.gate_envelope;
            // Control-rate conversion saves 75% of the remaining powf calls;
            // the per-sample gain follower reconstructs a smooth trajectory.
            if frame & 3 == 0 {
                let target_db = self.attack * transient * 12.0 + self.sustain * body * 8.0;
                self.target_gain = db_to_gain(target_db.clamp(-24.0, 18.0));
            }
            self.gain_envelope = Self::follow(self.gain_envelope, self.target_gain, gain_attack, gain_release);

            for channel in 0..MAX_CHANNELS {
                let sample = buffer.channels[channel][frame] * self.gain_envelope;
                buffer.channels[channel][frame] = if self.clip { Self::soft_clip(sample) } else { sample };
            }
        }
    }

    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "attack" | "attackAmount" => self.attack = value.clamp(-1.0, 1.0),
            "sustain" | "sustainAmount" => self.sustain = value.clamp(-1.0, 1.0),
            "threshold" | "thresholdDb" => self.threshold_db = value.clamp(-72.0, 0.0),
            "speed" | "timing" => self.speed = value.clamp(0.0, 1.0),
            "clip" => self.clip = value >= 0.5,
            _ => {}
        }
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }

    fn reset(&mut self) {
        self.fast_envelope = 0.0;
        self.slow_envelope = 0.0;
        self.gate_envelope = 0.0;
        self.gain_envelope = 1.0;
        self.target_gain = 1.0;
    }
}
