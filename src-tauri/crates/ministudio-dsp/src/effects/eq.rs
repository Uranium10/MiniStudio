struct EqBand {
    enabled: bool,
    kind: FilterKind,
    freq: f32,
    gain: f32,
    q: f32,
    sections: [Biquad; 4],
    section_count: usize,
}
impl EqBand {
    fn new(kind: FilterKind, freq: f32) -> Self {
        Self {
            enabled: true,
            kind,
            freq,
            gain: 0.0,
            q: 0.707,
            sections: [Biquad::new(), Biquad::new(), Biquad::new(), Biquad::new()],
            section_count: 1,
        }
    }
    fn update_scaled(&mut self, sr: f32, scale: f32, adaptive_q: bool) {
        let gain = self.gain * scale;
        let q = if adaptive_q {
            self.q * (1.0 + gain.abs() / 36.0)
        } else {
            self.q
        };
        for s in self.sections.iter_mut().take(self.section_count) {
            s.configure(self.kind, self.freq, gain, q, sr)
        }
    }
}

pub struct ParametricEq {
    bands: Vec<EqBand>,
    sample_rate: f32,
    bypassed: bool,
    scale: f32,
    adaptive_q: bool,
    output: Smoother,
    analyzer: SpectrumAnalyzer,
}
impl ParametricEq {
    pub fn new() -> Self {
        Self {
            bands: vec![
                EqBand::new(FilterKind::HighPass, 80.0),
                EqBand::new(FilterKind::Bell, 400.0),
                EqBand::new(FilterKind::Bell, 2500.0),
                EqBand::new(FilterKind::HighShelf, 10000.0),
            ],
            sample_rate: 48000.0,
            bypassed: false,
            scale: 1.0,
            adaptive_q: false,
            output: Smoother::new(1.0, 48_000.0, 0.01),
            analyzer: SpectrumAnalyzer::new(),
        }
    }
    pub fn new_eight() -> Self {
        let mut eq = Self {
            bands: vec![
                EqBand::new(FilterKind::HighPass, 30.0),
                EqBand::new(FilterKind::Bell, 100.0),
                EqBand::new(FilterKind::Bell, 300.0),
                EqBand::new(FilterKind::Bell, 800.0),
                EqBand::new(FilterKind::Bell, 2500.0),
                EqBand::new(FilterKind::Bell, 6000.0),
                EqBand::new(FilterKind::Bell, 12000.0),
                EqBand::new(FilterKind::HighShelf, 16000.0),
            ],
            sample_rate: 48000.0,
            bypassed: false,
            scale: 1.0,
            adaptive_q: true,
            output: Smoother::new(1.0, 48_000.0, 0.01),
            analyzer: SpectrumAnalyzer::new(),
        };
        eq.bands[7].enabled = false;
        eq
    }
}
impl DspEffect for ParametricEq {
    fn prepare(&mut self, sr: f32, _: usize, _: usize) {
        self.sample_rate = sr;
        self.analyzer.prepare(sr);
        for b in &mut self.bands {
            b.update_scaled(sr, self.scale, self.adaptive_q)
        }
    }
    fn process(&mut self, _events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        if self.bypassed {
            for frame in 0..n {
                self.analyzer
                    .push((b.channels[0][frame] + b.channels[1][frame]) * 0.5)
            }
            return;
        }
        for band in &mut self.bands {
            if !band.enabled {
                continue;
            }
            // Stage-major traversal keeps one recursive filter's coefficients
            // and state hot while it streams across the whole block.
            for s in band.sections.iter_mut().take(band.section_count) {
                for ch in 0..MAX_CHANNELS {
                    for x in &mut b.channels[ch][..n] {
                        *x = s.process(ch, *x)
                    }
                }
            }
        }
        for frame in 0..n {
            let gain = self.output.next();
            for channel in 0..MAX_CHANNELS {
                b.channels[channel][frame] *= gain
            }
            self.analyzer
                .push((b.channels[0][frame] + b.channels[1][frame]) * 0.5)
        }
    }
    fn set_param(&mut self, id: &str, v: f32) {
        if id == "scale" {
            self.scale = v.clamp(0.0, 2.0);
            for band in &mut self.bands {
                band.update_scaled(self.sample_rate, self.scale, self.adaptive_q)
            }
            return;
        }
        if id == "adaptiveQ" {
            self.adaptive_q = v >= 0.5;
            for band in &mut self.bands {
                band.update_scaled(self.sample_rate, self.scale, self.adaptive_q)
            }
            return;
        }
        if id == "outputDb" {
            self.output.set_target(db_to_gain(v.clamp(-24.0, 24.0)));
            return;
        }
        let (idx, param) = parse_band_param(id, self.bands.len());
        if let Some(b) = self.bands.get_mut(idx) {
            match param {
                "freq" => b.freq = v,
                "gain" | "gainDb" => b.gain = v,
                "q" => b.q = v,
                "enabled" => b.enabled = v >= 0.5,
                "slope" => b.section_count = (v as usize / 12).clamp(1, 4),
                "type" => {
                    b.kind = kind_from_value(v);
                    if !matches!(b.kind, FilterKind::HighPass | FilterKind::LowPass) {
                        b.section_count = 1
                    }
                }
                _ => {}
            }
            b.update_scaled(self.sample_rate, self.scale, self.adaptive_q)
        }
    }
    fn set_bypassed(&mut self, v: bool) {
        self.bypassed = v
    }
    fn reset(&mut self) {
        for b in &mut self.bands {
            for s in &mut b.sections {
                s.reset()
            }
        }
        self.analyzer.reset();
    }
    fn response(&self, points: usize) -> Option<EqFrequencyResponse> {
        let mut f = Vec::with_capacity(points);
        let mut combined = vec![0.0; points];
        let mut bands = Vec::new();
        for i in 0..points {
            f.push(20.0 * (1000.0_f32).powf(i as f32 / (points - 1).max(1) as f32))
        }
        for band in &self.bands {
            let mut curve = vec![0.0; points];
            if band.enabled {
                for (i, hz) in f.iter().enumerate() {
                    for s in band.sections.iter().take(band.section_count) {
                        curve[i] += s.magnitude_db(*hz, self.sample_rate)
                    }
                    combined[i] += curve[i]
                }
            }
            bands.push(curve)
        }
        Some(EqFrequencyResponse {
            frequencies: f,
            combined_db: combined,
            bands_db: bands,
        })
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.analyzer.bins)
    }
}
