pub struct Reverb {
    sr: f32,
    lines: Vec<Vec<f32>>,
    lengths: [usize; 8],
    indices: [usize; 8],
    damp: [f32; 8],
    damping: f32,
    size: f32,
    decay: f32,
    mix: Smoother,
    width: f32,
    pre_delay: Vec<f32>,
    pre_index: usize,
    pre_samples: usize,
    allpass: Vec<Vec<f32>>,
    allpass_indices: [usize; 4],
    diffusion: f32,
    feedback: [f32; 8],
    lfo_sin: [f32; 8],
    lfo_cos: [f32; 8],
    lfo_step_sin: [f32; 8],
    lfo_step_cos: [f32; 8],
    bypassed: bool,
}
impl Reverb {
    pub fn new() -> Self {
        Self {
            sr: 48000.0,
            lines: Vec::new(),
            lengths: [16; 8],
            indices: [0; 8],
            damp: [0.0; 8],
            damping: 0.45,
            size: 0.5,
            decay: 2.4,
            mix: Smoother::new(0.25, 48000.0, 0.02),
            width: 0.8,
            pre_delay: Vec::new(),
            pre_index: 0,
            pre_samples: 0,
            allpass: Vec::new(),
            allpass_indices: [0; 4],
            diffusion: 0.65,
            feedback: [0.0; 8],
            lfo_sin: [0.0; 8],
            lfo_cos: [1.0; 8],
            lfo_step_sin: [0.0; 8],
            lfo_step_cos: [1.0; 8],
            bypassed: false,
        }
    }
    fn update_lengths(&mut self) {
        let base = [1447, 1637, 1777, 1949, 2137, 2287, 2411, 2557];
        let scale = (0.5 + self.size * 1.5) * self.sr / 48000.0;
        for (index, nominal) in base.iter().enumerate() {
            self.lengths[index] =
                ((*nominal as f32 * scale) as usize).clamp(16, self.lines[index].len());
            self.indices[index] %= self.lengths[index]
        }
        self.update_feedback()
    }
    fn update_feedback(&mut self) {
        for (index, target) in self.feedback.iter_mut().enumerate() {
            *target =
                10.0_f32.powf(-3.0 * self.lengths[index] as f32 / (self.decay.max(0.1) * self.sr))
        }
    }
    fn update_lfo_rates(&mut self) {
        for index in 0..8 {
            let rate = 0.071 + index as f32 * 0.013;
            let step = 2.0 * PI * rate / self.sr;
            (self.lfo_step_sin[index], self.lfo_step_cos[index]) = step.sin_cos()
        }
    }
}
impl DspEffect for Reverb {
    fn prepare(&mut self, s: f32, _: usize, _: usize) {
        self.sr = s;
        let base = [1447, 1637, 1777, 1949, 2137, 2287, 2411, 2557];
        self.lines = base
            .iter()
            .map(|n| vec![0.0; ((*n as f32 * s / 48000.0 * 2.0) as usize).max(16)])
            .collect();
        self.pre_delay = vec![0.0; (s * 0.2) as usize + 1];
        self.allpass = [149, 211, 263, 293]
            .iter()
            .map(|n| vec![0.0; ((*n as f32 * s / 48000.0) as usize).max(8)])
            .collect();
        self.update_lfo_rates();
        self.update_lengths()
    }
    fn process(&mut self, _events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        if self.bypassed || self.lines.len() != 8 {
            return;
        }
        let damping_coefficient = 0.04 + (1.0 - self.damping) * 0.7;
        let modulation_depth = 0.5 + self.size * 1.5;
        let same_side = 0.5 + 0.5 * self.width;
        let cross_side = 0.5 - 0.5 * self.width;
        for i in 0..n {
            let dry_l = b.channels[0][i];
            let dry_r = b.channels[1][i];
            let mono = (dry_l + dry_r) * 0.353553;
            let mut input = if self.pre_samples == 0 {
                mono
            } else {
                let len = self.pre_delay.len();
                let read = (self.pre_index + len - self.pre_samples.min(len - 1)) % len;
                let delayed = self.pre_delay[read];
                self.pre_delay[self.pre_index] = mono;
                self.pre_index = increment_wrap(self.pre_index, len);
                delayed
            };
            for stage in 0..4 {
                let index = self.allpass_indices[stage];
                let delayed = self.allpass[stage][index];
                let output = delayed - self.diffusion * input;
                self.allpass[stage][index] = denormal(input + self.diffusion * output);
                self.allpass_indices[stage] = increment_wrap(index, self.allpass[stage].len());
                input = output
            }
            let mut v = [0.0; 8];
            for j in 0..8 {
                let modulation = self.lfo_sin[j] * modulation_depth;
                let next_sin =
                    self.lfo_sin[j] * self.lfo_step_cos[j] + self.lfo_cos[j] * self.lfo_step_sin[j];
                self.lfo_cos[j] =
                    self.lfo_cos[j] * self.lfo_step_cos[j] - self.lfo_sin[j] * self.lfo_step_sin[j];
                self.lfo_sin[j] = next_sin;
                v[j] = fractional_read(
                    &self.lines[j][..self.lengths[j]],
                    self.indices[j] as f32 - modulation,
                );
                self.damp[j] += damping_coefficient * (v[j] - self.damp[j]);
                // Feed the damped signal into the orthonormal matrix. The old
                // parallel damped + matrix paths had a loop gain above unity.
                v[j] = self.damp[j]
            }
            hadamard(&mut v);
            let mix = self.mix.next();
            let left = (v[0] + v[2] + v[4] + v[6]) * 0.25;
            let right = (v[1] + v[3] + v[5] + v[7]) * 0.25;
            for j in 0..8 {
                let len = self.lengths[j];
                self.lines[j][self.indices[j]] = denormal(input + v[j] * self.feedback[j]);
                self.indices[j] = increment_wrap(self.indices[j], len)
            }
            b.channels[0][i] = dry_l * (1.0 - mix) + (left * same_side + right * cross_side) * mix;
            b.channels[1][i] = dry_r * (1.0 - mix) + (right * same_side + left * cross_side) * mix
        }
        // The recursive oscillator avoids eight sin() calls per sample. A
        // block-rate normalization prevents long-session round-off drift.
        for index in 0..8 {
            let norm = (self.lfo_sin[index] * self.lfo_sin[index]
                + self.lfo_cos[index] * self.lfo_cos[index])
                .sqrt()
                .max(1e-12);
            self.lfo_sin[index] /= norm;
            self.lfo_cos[index] /= norm
        }
    }
    fn set_param(&mut self, id: &str, v: f32) {
        match id {
            "preDelayMs" | "pre_delay_ms" => {
                self.pre_samples = (v.clamp(0.0, 200.0) * 0.001 * self.sr) as usize
            }
            "size" => {
                self.size = v.clamp(0.0, 1.0);
                if self.lines.len() == 8 {
                    self.update_lengths()
                }
            }
            "decay" | "decaySec" => {
                self.decay = v.clamp(0.1, 20.0);
                if self.lines.len() == 8 {
                    self.update_feedback()
                }
            }
            "damping" => self.damping = v.clamp(0.0, 1.0),
            "diffusion" => self.diffusion = v.clamp(0.0, 0.92),
            "mix" => self.mix.set_target(v),
            "width" => self.width = v,
            _ => {}
        }
    }
    fn set_bypassed(&mut self, v: bool) {
        self.bypassed = v
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0)
        }
        self.indices = [0; 8];
        self.damp = [0.0; 8];
        self.lfo_sin = [0.0; 8];
        self.lfo_cos = [1.0; 8];
        self.pre_delay.fill(0.0);
        self.pre_index = 0;
        for line in &mut self.allpass {
            line.fill(0.0)
        }
        self.allpass_indices = [0; 4]
    }
    fn tail_samples(&self) -> usize {
        (self.decay * self.sr) as usize
    }
}
