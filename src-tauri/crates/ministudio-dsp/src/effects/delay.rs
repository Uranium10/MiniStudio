pub struct Delay {
    sr: f32,
    lines: [Vec<f32>; MAX_CHANNELS],
    write: usize,
    time: Smoother,
    feedback: Smoother,
    mix: Smoother,
    damping_coeff: f32,
    stereo_offset: Smoother,
    lp: [f32; MAX_CHANNELS],
    ping_pong: bool,
    bypassed: bool,
}
impl Delay {
    pub fn new() -> Self {
        Self {
            sr: 48000.0,
            lines: [Vec::new(), Vec::new()],
            write: 0,
            time: Smoother::new(12000.0, 48000.0, 0.02),
            feedback: Smoother::new(0.3, 48000.0, 0.02),
            mix: Smoother::new(0.25, 48000.0, 0.02),
            damping_coeff: 0.35,
            stereo_offset: Smoother::new(0.0, 48000.0, 0.02),
            lp: [0.0; 2],
            ping_pong: false,
            bypassed: false,
        }
    }
}
impl DspEffect for Delay {
    fn prepare(&mut self, s: f32, _: usize, _: usize) {
        self.sr = s;
        let len = (s * 2.1) as usize;
        self.lines = [vec![0.0; len], vec![0.0; len]]
    }
    fn process(&mut self, _events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        if self.bypassed || self.lines[0].is_empty() {
            return;
        }
        let len = self.lines[0].len();
        for i in 0..n {
            let d = self.time.next().clamp(1.0, (len - 4) as f32);
            let offset = self.stereo_offset.next();
            let mut wet = [0.0; 2];
            for ch in 0..2 {
                let channel_delay = (d + if ch == 0 { -offset * 0.5 } else { offset * 0.5 })
                    .clamp(1.0, (len - 4) as f32);
                wet[ch] = cubic_delay_read(&self.lines[ch], self.write, channel_delay)
            }
            let fb = self.feedback.next();
            let mix = self.mix.next();
            for ch in 0..2 {
                let source = if self.ping_pong { wet[1 - ch] } else { wet[ch] };
                self.lp[ch] += self.damping_coeff * (source - self.lp[ch]);
                let input = b.channels[ch][i];
                self.lines[ch][self.write] = denormal(input + self.lp[ch] * fb);
                b.channels[ch][i] = input * (1.0 - mix) + wet[ch] * mix
            }
            self.write = increment_wrap(self.write, len)
        }
    }
    fn set_param(&mut self, id: &str, v: f32) {
        match id {
            "time" => self.time.set_target(v * self.sr),
            "timeMs" => self.time.set_target(v * 0.001 * self.sr),
            "feedback" => self.feedback.set_target(v.min(0.98)),
            "mix" => self.mix.set_target(v),
            "damping" => self.damping_coeff = v.clamp(0.01, 1.0),
            "dampingHz" | "damping_hz" => {
                let cutoff = v.clamp(1000.0, 20000.0);
                self.damping_coeff = 1.0 - (-2.0 * PI * cutoff / self.sr).exp()
            }
            "stereoOffsetMs" | "stereo_offset_ms" => {
                self.stereo_offset.set_target(v * 0.001 * self.sr)
            }
            "pingPong" => self.ping_pong = v >= 0.5,
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
        self.lp = [0.0; 2];
        self.write = 0
    }
    fn tail_samples(&self) -> usize {
        (self.sr * 2.0) as usize
    }
}
