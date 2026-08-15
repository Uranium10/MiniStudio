#[derive(Clone, Copy, Default)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum FilterKind {
    Bell,
    LowShelf,
    HighShelf,
    HighPass,
    LowPass,
    BandPass,
    Notch,
}

struct Biquad {
    coeff: Coefficients,
    z1: [f32; MAX_CHANNELS],
    z2: [f32; MAX_CHANNELS],
}
impl Biquad {
    fn new() -> Self {
        Self {
            coeff: Coefficients {
                b0: 1.0,
                ..Default::default()
            },
            z1: [0.0; MAX_CHANNELS],
            z2: [0.0; MAX_CHANNELS],
        }
    }
    fn configure(&mut self, kind: FilterKind, freq: f32, gain_db: f32, q: f32, sr: f32) {
        let w0 = 2.0 * PI * freq.clamp(20.0, sr * 0.49) / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q.max(0.1));
        let a = 10.0_f32.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::Bell => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    (a * ((a + 1.0) - (a - 1.0) * cos + s)),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            FilterKind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
        };
        self.coeff = Coefficients {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        };
    }
    #[inline(always)]
    fn process(&mut self, channel: usize, x: f32) -> f32 {
        let c = self.coeff;
        let y = c.b0 * x + self.z1[channel];
        self.z1[channel] = c.b1 * x - c.a1 * y + self.z2[channel];
        self.z2[channel] = c.b2 * x - c.a2 * y;
        denormal(y)
    }
    fn reset(&mut self) {
        self.z1 = [0.0; MAX_CHANNELS];
        self.z2 = [0.0; MAX_CHANNELS];
    }
    fn magnitude_db(&self, freq: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * freq / sr;
        let (s, c) = w.sin_cos();
        let c2 = (2.0 * w).cos();
        let s2 = (2.0 * w).sin();
        let k = self.coeff;
        let nr = k.b0 + k.b1 * c + k.b2 * c2;
        let ni = -k.b1 * s - k.b2 * s2;
        let dr = 1.0 + k.a1 * c + k.a2 * c2;
        let di = -k.a1 * s - k.a2 * s2;
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-20)).log10()
    }
}
