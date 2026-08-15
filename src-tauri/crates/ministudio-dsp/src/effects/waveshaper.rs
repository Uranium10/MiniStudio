#[derive(Clone, Copy)]
enum Curve {
    SoftClip,
    HardClip,
    Sine,
}
pub struct Waveshaper {
    drive: Smoother,
    output: Smoother,
    mix: Smoother,
    curve: Curve,
    oversample: usize,
    auto_level: bool,
    dc: bool,
    dc_x: [f32; 2],
    dc_y: [f32; 2],
    fir: [[f32; 15]; 2],
    fir_pos: [usize; 2],
    previous: [f32; 2],
    bypassed: bool,
}
impl Waveshaper {
    pub fn new() -> Self {
        Self {
            drive: Smoother::new(1.0, 48000.0, 0.01),
            output: Smoother::new(1.0, 48000.0, 0.01),
            mix: Smoother::new(1.0, 48000.0, 0.01),
            curve: Curve::SoftClip,
            oversample: 4,
            auto_level: true,
            dc: true,
            dc_x: [0.0; 2],
            dc_y: [0.0; 2],
            fir: [[0.0; 15]; 2],
            fir_pos: [0; 2],
            previous: [0.0; 2],
            bypassed: false,
        }
    }
}
impl DspEffect for Waveshaper {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, _events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        if self.bypassed {
            return;
        }
        for i in 0..n {
            let drive = self.drive.next();
            let out = self.output.next();
            let mix = self.mix.next();
            let compensation = if self.auto_level {
                drive.sqrt().recip().clamp(0.12, 1.0)
            } else {
                1.0
            };
            for ch in 0..2 {
                let dry = b.channels[ch][i];
                let mut wet = 0.0;
                for phase in 0..self.oversample {
                    let t = (phase + 1) as f32 / self.oversample as f32;
                    let up = self.previous[ch] + (dry - self.previous[ch]) * t;
                    let shaped = shape(self.curve, up * drive);
                    let pos = self.fir_pos[ch];
                    self.fir[ch][pos] = shaped;
                    wet = fir_read(&self.fir[ch], pos);
                    self.fir_pos[ch] = increment_wrap(pos, 15)
                }
                self.previous[ch] = dry;
                wet *= out * compensation;
                if self.dc {
                    let y = wet - self.dc_x[ch] + 0.99935 * self.dc_y[ch];
                    self.dc_x[ch] = wet;
                    self.dc_y[ch] = y;
                    wet = y
                }
                b.channels[ch][i] = dry * (1.0 - mix) + wet * mix
            }
        }
    }
    fn set_param(&mut self, id: &str, v: f32) {
        match id {
            "driveDb" | "drive" => self.drive.set_target(db_to_gain(v)),
            "outputDb" => self.output.set_target(db_to_gain(v)),
            "mix" => self.mix.set_target(v),
            "oversample" => {
                self.oversample = match v as usize {
                    8 => 8,
                    4 => 4,
                    2 => 2,
                    _ => 1,
                }
            }
            "curve" => {
                self.curve = match v as usize {
                    1 => Curve::HardClip,
                    2 => Curve::Sine,
                    _ => Curve::SoftClip,
                }
            }
            "dcBlock" => self.dc = v >= 0.5,
            "autoLevel" | "auto_level" => self.auto_level = v >= 0.5,
            _ => {}
        }
    }
    fn set_bypassed(&mut self, v: bool) {
        self.bypassed = v
    }
    fn reset(&mut self) {
        self.dc_x = [0.0; 2];
        self.dc_y = [0.0; 2];
        self.fir = [[0.0; 15]; 2];
        self.fir_pos = [0; 2];
        self.previous = [0.0; 2]
    }
    fn latency_samples(&self) -> usize {
        if self.oversample > 1 {
            7_usize.div_ceil(self.oversample)
        } else {
            0
        }
    }
}
