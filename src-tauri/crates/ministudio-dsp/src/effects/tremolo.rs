#[derive(Clone, Copy)]
enum LfoWave {
    Sine,
    Triangle,
    Square,
    Saw,
}
pub struct LfoTremolo {
    sample_rate: f32,
    rate_hz: f32,
    volume_depth: Smoother,
    pan_depth: Smoother,
    phase_offset: f32,
    waveform: LfoWave,
    phase: f32,
    bypassed: bool,
}
impl LfoTremolo {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            rate_hz: 4.0,
            volume_depth: Smoother::new(0.5, 48_000.0, 0.01),
            pan_depth: Smoother::new(0.0, 48_000.0, 0.01),
            phase_offset: 0.25,
            waveform: LfoWave::Sine,
            phase: 0.0,
            bypassed: false,
        }
    }
    #[inline(always)]
    fn wave(&self, phase: f32) -> f32 {
        let p = phase - phase.floor();
        match self.waveform {
            LfoWave::Sine => (2.0 * PI * p).sin(),
            LfoWave::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
            LfoWave::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::Saw => p * 2.0 - 1.0,
        }
    }
}
impl DspEffect for LfoTremolo {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.volume_depth = Smoother::new(self.volume_depth.target, sample_rate, 0.01);
        self.pan_depth = Smoother::new(self.pan_depth.target, sample_rate, 0.01);
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        for frame in 0..frames {
            let left_lfo = self.wave(self.phase);
            let right_lfo = self.wave(self.phase + self.phase_offset);
            let volume_depth = self.volume_depth.next().clamp(0.0, 1.0);
            let volume = 1.0 - volume_depth * (0.5 + 0.25 * (left_lfo + right_lfo));
            let pan = ((left_lfo + right_lfo) * 0.5 * self.pan_depth.next()).clamp(-1.0, 1.0);
            let left_gain = ((pan + 1.0) * PI * 0.25).cos() * std::f32::consts::SQRT_2;
            let right_gain = ((pan + 1.0) * PI * 0.25).sin() * std::f32::consts::SQRT_2;
            buffer.channels[0][frame] *= volume * left_gain;
            buffer.channels[1][frame] *= volume * right_gain;
            self.phase += self.rate_hz / self.sample_rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "rateHz" | "rate" => self.rate_hz = value.clamp(0.01, 30.0),
            "volumeDepth" | "depth" => self.volume_depth.set_target(value.clamp(0.0, 1.0)),
            "panDepth" => self.pan_depth.set_target(value.clamp(0.0, 1.0)),
            "stereoPhase" | "phase" => self.phase_offset = value.clamp(0.0, 1.0),
            "waveform" => {
                self.waveform = match value.round() as i32 {
                    1 => LfoWave::Triangle,
                    2 => LfoWave::Square,
                    3 => LfoWave::Saw,
                    _ => LfoWave::Sine,
                }
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.phase = 0.0
    }
}
