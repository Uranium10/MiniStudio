const MAX_DISPERSER_STAGES: usize = 64;

#[derive(Clone, Copy)]
struct AllpassSection {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: [f64; MAX_CHANNELS],
    z2: [f64; MAX_CHANNELS],
}
impl AllpassSection {
    fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: [0.0; MAX_CHANNELS],
            z2: [0.0; MAX_CHANNELS],
        }
    }
    fn configure(&mut self, frequency: f32, q: f32, sample_rate: f32) {
        // RBJ second-order all-pass. Double precision avoids coefficient loss
        // when the pole pair sits very close to the unit circle at low Hz.
        let omega = 2.0_f64 * std::f64::consts::PI * f64::from(frequency) / f64::from(sample_rate);
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / (2.0 * f64::from(q.max(0.01)));
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - alpha) / a0;
        self.b1 = -2.0 * cos / a0;
        self.b2 = 1.0;
        self.a1 = self.b1;
        self.a2 = self.b0;
    }
    fn process(&mut self, channel: usize, input: f32) -> f32 {
        let x = f64::from(input);
        let y = self.b0 * x + self.z1[channel];
        self.z1[channel] = self.b1 * x - self.a1 * y + self.z2[channel];
        self.z2[channel] = self.b2 * x - self.a2 * y;
        denormal(y as f32)
    }
    fn reset(&mut self) {
        self.z1 = [0.0; MAX_CHANNELS];
        self.z2 = [0.0; MAX_CHANNELS];
    }
}

/// Frequency-dependent phase dispersion made from a cascade of identical
/// second-order all-pass sections. Magnitude stays unity; Amount increases the
/// cascade order and Pinch narrows the group-delay peak around Frequency.
pub struct Disperser {
    sample_rate: f32,
    frequency: f32,
    target_frequency: f32,
    pinch: f32,
    target_pinch: f32,
    stages: usize,
    sections: [AllpassSection; MAX_DISPERSER_STAGES],
    analyzer: SpectrumAnalyzer,
    bypassed: bool,
}
impl Disperser {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            frequency: 3_050.0,
            target_frequency: 3_050.0,
            pinch: 0.45,
            target_pinch: 0.45,
            stages: 16,
            sections: [AllpassSection::new(); MAX_DISPERSER_STAGES],
            analyzer: SpectrumAnalyzer::new(),
            bypassed: false,
        }
    }
    fn q(&self) -> f32 {
        0.12 * 100.0_f32.powf(self.pinch.clamp(0.0, 1.0))
    }
    fn configure(&mut self) {
        let frequency = self.frequency.clamp(20.0, self.sample_rate * 0.45);
        let q = self.q();
        // Every stage shares coefficients, so evaluate sin/cos only once per
        // audio block and retain each section's independent delay state.
        let mut template = AllpassSection::new();
        template.configure(frequency, q, self.sample_rate);
        for section in self.sections.iter_mut().take(self.stages) {
            section.b0 = template.b0;
            section.b1 = template.b1;
            section.b2 = template.b2;
            section.a1 = template.a1;
            section.a2 = template.a2;
        }
    }
}
impl DspEffect for Disperser {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.analyzer.prepare(sample_rate);
        self.frequency = self.target_frequency.clamp(20.0, sample_rate * 0.45);
        self.configure();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed || self.stages == 0 {
            for frame in 0..frames {
                self.analyzer
                    .push((buffer.channels[0][frame] + buffer.channels[1][frame]) * 0.5)
            }
            return;
        }
        // Twenty-millisecond block-rate smoothing prevents zipper noise while
        // avoiding trigonometric coefficient work in the inner sample loop.
        let smoothing = 1.0 - (-(frames as f32) / (self.sample_rate * 0.02)).exp();
        self.frequency += (self.target_frequency - self.frequency) * smoothing;
        self.pinch += (self.target_pinch - self.pinch) * smoothing;
        self.configure();
        // Stage-major traversal avoids walking the entire 64-section array for
        // every sample and is mathematically identical for a serial cascade.
        for section in self.sections.iter_mut().take(self.stages) {
            for channel in 0..MAX_CHANNELS {
                for sample in &mut buffer.channels[channel][..frames] {
                    *sample = section.process(channel, *sample)
                }
            }
        }
        for frame in 0..frames {
            self.analyzer
                .push((buffer.channels[0][frame] + buffer.channels[1][frame]) * 0.5)
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "frequency" | "freq" => {
                self.target_frequency = value.clamp(20.0, self.sample_rate * 0.45)
            }
            "pinch" => self.target_pinch = value.clamp(0.0, 1.0),
            "amount" => {
                let next = (value.clamp(0.0, 1.0) * MAX_DISPERSER_STAGES as f32).round() as usize;
                if next > self.stages {
                    for section in &mut self.sections[self.stages..next] {
                        section.reset()
                    }
                }
                self.stages = next;
                self.configure();
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for section in &mut self.sections {
            section.reset()
        }
        self.analyzer.reset();
    }
    fn tail_samples(&self) -> usize {
        let estimate =
            self.stages as f32 * self.q() * self.sample_rate / self.frequency.max(20.0) * 2.0;
        estimate.min(self.sample_rate * 5.0) as usize
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.analyzer.bins)
    }
}
