pub struct Clipper {
    sample_rate: f32,
    input: Smoother,
    output: Smoother,
    knee: f32,
    previous: [f32; MAX_CHANNELS],
    fir: [[f32; 15]; MAX_CHANNELS],
    fir_position: [usize; MAX_CHANNELS],
    flow: [f32; DISTORTION_SPECTRUM_BINS],
    flow_peak: f32,
    flow_count: usize,
    flow_interval: usize,
    bypassed: bool,
}
impl Clipper {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            input: Smoother::new(db_to_gain(6.0), 48_000.0, 0.01),
            output: Smoother::new(db_to_gain(-1.0), 48_000.0, 0.01),
            knee: 0.25,
            previous: [0.0; MAX_CHANNELS],
            fir: [[0.0; 15]; MAX_CHANNELS],
            fir_position: [0; MAX_CHANNELS],
            flow: [0.0; DISTORTION_SPECTRUM_BINS],
            flow_peak: 0.0,
            flow_count: 0,
            flow_interval: 800,
            bypassed: false,
        }
    }
}
impl DspEffect for Clipper {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.flow_interval = (sample_rate / 60.0).round().max(1.0) as usize;
        self.input = Smoother::new(self.input.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        self.reset();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        for frame in 0..frames {
            let input_gain = self.input.next();
            let output_gain = self.output.next();
            let mut frame_peak = 0.0_f32;
            for channel in 0..MAX_CHANNELS {
                let dry = buffer.channels[channel][frame] * input_gain;
                frame_peak = frame_peak.max(dry.abs());
                let mut wet = 0.0;
                for phase in 0..4 {
                    let fraction = (phase + 1) as f32 * 0.25;
                    let up = self.previous[channel] + (dry - self.previous[channel]) * fraction;
                    let clipped = clipper_curve(up, self.knee);
                    let position = self.fir_position[channel];
                    self.fir[channel][position] = clipped;
                    wet = fir_read(&self.fir[channel], position);
                    self.fir_position[channel] = increment_wrap(position, 15);
                }
                self.previous[channel] = dry;
                buffer.channels[channel][frame] = denormal(wet * output_gain);
            }
            self.flow_peak = self.flow_peak.max(frame_peak);
            self.flow_count += 1;
            if self.flow_count >= self.flow_interval {
                self.flow.copy_within(1.., 0);
                self.flow[DISTORTION_SPECTRUM_BINS - 1] = self.flow_peak.min(2.5);
                self.flow_peak = 0.0;
                self.flow_count = 0;
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "inputDb" | "inputGainDb" => {
                self.input.set_target(db_to_gain(value.clamp(-24.0, 36.0)))
            }
            "outputDb" | "outputGainDb" => {
                self.output.set_target(db_to_gain(value.clamp(-24.0, 0.0)))
            }
            "knee" => self.knee = value.clamp(0.0, 1.0),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.previous = [0.0; MAX_CHANNELS];
        self.fir = [[0.0; 15]; MAX_CHANNELS];
        self.fir_position = [0; MAX_CHANNELS];
        self.flow = [0.0; DISTORTION_SPECTRUM_BINS];
        self.flow_peak = 0.0;
        self.flow_count = 0;
    }
    fn latency_samples(&self) -> usize {
        2
    }
    fn runtime_capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::finite_tail(15)
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.flow)
    }
}
