// Allocation-free native DSP effects shared by realtime and offline renderers.
use super::types::{db_to_gain, EffectSpec, EqFrequencyResponse};
use super::{MAX_BLOCK_SIZE, MAX_CHANNELS};
use std::f32::consts::PI;

pub const DISTORTION_SPECTRUM_BINS: usize = 48;

pub struct AudioBuffer {
    pub channels: [Vec<f32>; MAX_CHANNELS],
}
impl AudioBuffer {
    pub fn new() -> Self {
        Self {
            channels: [vec![0.0; MAX_BLOCK_SIZE], vec![0.0; MAX_BLOCK_SIZE]],
        }
    }
    pub fn clear(&mut self, frames: usize) {
        for channel in &mut self.channels {
            channel[..frames].fill(0.0);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Smoother {
    current: f32,
    target: f32,
    coeff: f32,
}
impl Smoother {
    pub fn new(value: f32, sample_rate: f32, time_sec: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: (-1.0 / (time_sec * sample_rate)).exp(),
        }
    }
    pub fn set_target(&mut self, value: f32) {
        self.target = value;
    }
    #[inline(always)]
    pub fn next(&mut self) -> f32 {
        self.current += (self.target - self.current) * (1.0 - self.coeff);
        self.current
    }
    pub fn advance(&mut self, samples: usize) -> f32 {
        self.current = self.target
            + (self.current - self.target) * self.coeff.powi(samples.min(i32::MAX as usize) as i32);
        self.current
    }
}

pub trait DspEffect: Send {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize);
    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        let _ = sidechain;
        self.process(buffer, frames)
    }
    fn set_param(&mut self, id: &str, value: f32);
    fn set_bypassed(&mut self, bypassed: bool);
    fn reset(&mut self);
    fn tail_samples(&self) -> usize {
        0
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn response(&self, _points: usize) -> Option<EqFrequencyResponse> {
        None
    }
    fn multiband_levels(&self) -> Option<[[f32; MAX_CHANNELS]; 3]> {
        None
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        None
    }
}

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
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
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
        self.sr = s
    }
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
        self.process_with_sidechain(b, None, n)
    }
    fn process_with_sidechain(
        &mut self,
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
        self.gain_reduction_db = 0.0
    }
}

/// A fourth-order Linkwitz-Riley crossover made from two cascaded Butterworth
/// biquads per branch. Its low and high outputs have matching phase and sum
/// without a level bump at the crossover frequency.
struct Crossover {
    low: [Biquad; 2],
    high: [Biquad; 2],
    frequency: f32,
    sample_rate: f32,
}
impl Crossover {
    fn new(frequency: f32) -> Self {
        let mut crossover = Self {
            low: [Biquad::new(), Biquad::new()],
            high: [Biquad::new(), Biquad::new()],
            frequency,
            sample_rate: 48_000.0,
        };
        crossover.configure(frequency, 48_000.0);
        crossover
    }
    fn configure(&mut self, frequency: f32, sample_rate: f32) {
        self.frequency = frequency;
        self.sample_rate = sample_rate;
        for section in &mut self.low {
            section.configure(
                FilterKind::LowPass,
                frequency,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                sample_rate,
            )
        }
        for section in &mut self.high {
            section.configure(
                FilterKind::HighPass,
                frequency,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                sample_rate,
            )
        }
    }
    fn process(&mut self, channel: usize, input: f32) -> (f32, f32) {
        let mut low = input;
        let mut high = input;
        for section in &mut self.low {
            low = section.process(channel, low)
        }
        for section in &mut self.high {
            high = section.process(channel, high)
        }
        (low, high)
    }
    fn reset(&mut self) {
        for section in self.low.iter_mut().chain(self.high.iter_mut()) {
            section.reset()
        }
    }
}

/// Three-band dynamics processor. The extra high crossover on the low branch
/// is a phase-compensation all-pass (LP + HP), keeping all three bands aligned
/// when they are recombined.
pub struct MultibandCompressor {
    sample_rate: f32,
    split_low: f32,
    split_high: f32,
    low_split: Crossover,
    high_split: Crossover,
    low_phase: Crossover,
    compressors: [Compressor; 3],
    dry: AudioBuffer,
    bands: [AudioBuffer; 3],
    output: Smoother,
    mix: Smoother,
    levels: [[f32; MAX_CHANNELS]; 3],
    bypassed: bool,
}
impl MultibandCompressor {
    pub fn new() -> Self {
        let mut compressors = std::array::from_fn(|_| Compressor::new());
        let defaults = [
            (-24.0, 3.0, 30.0, 250.0),
            (-20.0, 2.5, 15.0, 180.0),
            (-18.0, 2.0, 6.0, 120.0),
        ];
        for (compressor, (threshold, ratio, attack, release)) in
            compressors.iter_mut().zip(defaults)
        {
            compressor.threshold = threshold;
            compressor.ratio = ratio;
            compressor.attack = attack;
            compressor.release = release;
            compressor.knee = 8.0;
        }
        Self {
            sample_rate: 48_000.0,
            split_low: 150.0,
            split_high: 2_500.0,
            low_split: Crossover::new(150.0),
            high_split: Crossover::new(2_500.0),
            low_phase: Crossover::new(2_500.0),
            compressors,
            dry: AudioBuffer::new(),
            bands: std::array::from_fn(|_| AudioBuffer::new()),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            levels: [[0.0; MAX_CHANNELS]; 3],
            bypassed: false,
        }
    }
    fn update_crossovers(&mut self) {
        self.low_split.configure(self.split_low, self.sample_rate);
        self.high_split.configure(self.split_high, self.sample_rate);
        self.low_phase.configure(self.split_high, self.sample_rate);
    }
    fn set_band_param(&mut self, band: usize, param: &str, value: f32) {
        if let Some(compressor) = self.compressors.get_mut(band) {
            compressor.set_param(param, value)
        }
    }
}
impl DspEffect for MultibandCompressor {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate = sample_rate;
        self.update_crossovers();
        for compressor in &mut self.compressors {
            compressor.prepare(sample_rate, max_block, channels)
        }
    }
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            self.levels = [[0.0; MAX_CHANNELS]; 3];
            return;
        }
        for channel in 0..MAX_CHANNELS {
            for frame in 0..frames {
                let input = buffer.channels[channel][frame];
                self.dry.channels[channel][frame] = input;
                let (low, upper) = self.low_split.process(channel, input);
                let (mid, high) = self.high_split.process(channel, upper);
                let (low_late, low_early) = self.low_phase.process(channel, low);
                self.bands[0].channels[channel][frame] = low_late + low_early;
                self.bands[1].channels[channel][frame] = mid;
                self.bands[2].channels[channel][frame] = high;
            }
        }
        for (compressor, band) in self.compressors.iter_mut().zip(self.bands.iter_mut()) {
            compressor.process(band, frames)
        }
        let mut peaks = [[0.0_f32; MAX_CHANNELS]; 3];
        for frame in 0..frames {
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                let low = self.bands[0].channels[channel][frame];
                let mid = self.bands[1].channels[channel][frame];
                let high = self.bands[2].channels[channel][frame];
                peaks[0][channel] = peaks[0][channel].max(low.abs());
                peaks[1][channel] = peaks[1][channel].max(mid.abs());
                peaks[2][channel] = peaks[2][channel].max(high.abs());
                let wet = low + mid + high;
                buffer.channels[channel][frame] =
                    (self.dry.channels[channel][frame] * (1.0 - mix) + wet * mix) * output
            }
        }
        let decay = 10.0_f32.powf(-12.0 * frames as f32 / self.sample_rate / 20.0);
        for (levels, peaks) in self.levels.iter_mut().zip(peaks) {
            for channel in 0..MAX_CHANNELS {
                levels[channel] = peaks[channel].max(levels[channel] * decay)
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            // Keep the requested low band within 20-150 Hz. The second split
            // defaults to 2.5 kHz while retaining a useful editable range.
            "splitLow" => {
                self.split_low = value.clamp(20.0, 150.0);
                self.update_crossovers()
            }
            "splitHigh" => {
                self.split_high = value.clamp(300.0, 16_000.0);
                self.update_crossovers()
            }
            "knee" | "kneeDb" => {
                for compressor in &mut self.compressors {
                    compressor.set_param("knee", value)
                }
            }
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            _ => {
                for (prefix, band) in [("low", 0), ("mid", 1), ("high", 2)] {
                    if let Some(param) = id.strip_prefix(prefix) {
                        let normalized = match param {
                            "Threshold" => "threshold",
                            "Ratio" => "ratio",
                            "Attack" => "attack",
                            "Release" => "release",
                            "MakeupDb" => "makeupDb",
                            _ => return,
                        };
                        self.set_band_param(band, normalized, value);
                        return;
                    }
                }
            }
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.low_split.reset();
        self.high_split.reset();
        self.low_phase.reset();
        for compressor in &mut self.compressors {
            compressor.reset()
        }
        self.levels = [[0.0; MAX_CHANNELS]; 3]
    }
    fn multiband_levels(&self) -> Option<[[f32; MAX_CHANNELS]; 3]> {
        Some(self.levels)
    }
}

pub struct Utility {
    sample_rate: f32,
    input_mode: u8,
    invert_left: bool,
    invert_right: bool,
    width: Smoother,
    gain: Smoother,
    balance: Smoother,
    mono: bool,
    bass_mono: bool,
    bass_frequency: f32,
    bass_split: Crossover,
    mute: bool,
    dc_block: bool,
    dc_x: [f32; MAX_CHANNELS],
    dc_y: [f32; MAX_CHANNELS],
    bypassed: bool,
}
impl Utility {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            input_mode: 0,
            invert_left: false,
            invert_right: false,
            width: Smoother::new(1.0, 48_000.0, 0.01),
            gain: Smoother::new(1.0, 48_000.0, 0.01),
            balance: Smoother::new(0.0, 48_000.0, 0.01),
            mono: false,
            bass_mono: false,
            bass_frequency: 120.0,
            bass_split: Crossover::new(120.0),
            mute: false,
            dc_block: false,
            dc_x: [0.0; MAX_CHANNELS],
            dc_y: [0.0; MAX_CHANNELS],
            bypassed: false,
        }
    }
}
impl DspEffect for Utility {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.bass_split
            .configure(self.bass_frequency, self.sample_rate)
    }
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        for frame in 0..frames {
            let input = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let (mut left, mut right) = match self.input_mode {
                1 => (input[0], input[0]),
                2 => (input[1], input[1]),
                3 => (input[1], input[0]),
                _ => (input[0], input[1]),
            };
            if self.invert_left {
                left = -left
            }
            if self.invert_right {
                right = -right
            }
            if self.bass_mono {
                let (low_left, high_left) = self.bass_split.process(0, left);
                let (low_right, high_right) = self.bass_split.process(1, right);
                let low_mono = (low_left + low_right) * 0.5;
                left = low_mono + high_left;
                right = low_mono + high_right
            }
            let mid = (left + right) * 0.5;
            let width = self.width.next().clamp(0.0, 2.0);
            let side = if self.mono {
                0.0
            } else {
                (left - right) * 0.5 * width
            };
            left = mid + side;
            right = mid - side;
            let balance = self.balance.next().clamp(-1.0, 1.0);
            let smoothed_gain = self.gain.next();
            let gain = if self.mute { 0.0 } else { smoothed_gain };
            left *= gain * if balance > 0.0 { 1.0 - balance } else { 1.0 };
            right *= gain * if balance < 0.0 { 1.0 + balance } else { 1.0 };
            if self.dc_block {
                let filtered_left = left - self.dc_x[0] + 0.995 * self.dc_y[0];
                let filtered_right = right - self.dc_x[1] + 0.995 * self.dc_y[1];
                self.dc_x = [left, right];
                self.dc_y = [filtered_left, filtered_right];
                left = denormal(filtered_left);
                right = denormal(filtered_right)
            }
            buffer.channels[0][frame] = left;
            buffer.channels[1][frame] = right
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "inputMode" => self.input_mode = value.round().clamp(0.0, 3.0) as u8,
            "invertLeft" => self.invert_left = value >= 0.5,
            "invertRight" => self.invert_right = value >= 0.5,
            "width" => self.width.set_target(value.clamp(0.0, 2.0)),
            "gainDb" => self.gain.set_target(db_to_gain(value.clamp(-60.0, 24.0))),
            "balance" => self.balance.set_target(value.clamp(-1.0, 1.0)),
            "mono" => self.mono = value >= 0.5,
            "bassMono" => {
                self.bass_mono = value >= 0.5;
                self.bass_split.reset()
            }
            "bassFreq" => {
                self.bass_frequency = value.clamp(40.0, 500.0);
                self.bass_split
                    .configure(self.bass_frequency, self.sample_rate)
            }
            "mute" => self.mute = value >= 0.5,
            "dcBlock" => self.dc_block = value >= 0.5,
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.bass_split.reset();
        self.dc_x = [0.0; MAX_CHANNELS];
        self.dc_y = [0.0; MAX_CHANNELS]
    }
}

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
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
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
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
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
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
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

#[derive(Clone, Copy, PartialEq)]
enum DistortionMode {
    Off,
    Tube,
    Tape,
    Saturation,
    Exciter,
}

struct DistortionBand {
    mode: DistortionMode,
    gain: Smoother,
    drive: Smoother,
    mix: Smoother,
    tape_memory: [f32; MAX_CHANNELS],
    dc_x: [f32; MAX_CHANNELS],
    dc_y: [f32; MAX_CHANNELS],
    fir: [[f32; 15]; MAX_CHANNELS],
    fir_pos: [usize; MAX_CHANNELS],
    previous: [f32; MAX_CHANNELS],
}
impl DistortionBand {
    fn new(mode: DistortionMode, drive_db: f32, mix: f32) -> Self {
        Self {
            mode,
            gain: Smoother::new(1.0, 48_000.0, 0.01),
            drive: Smoother::new(db_to_gain(drive_db), 48_000.0, 0.01),
            mix: Smoother::new(mix, 48_000.0, 0.01),
            tape_memory: [0.0; MAX_CHANNELS],
            dc_x: [0.0; MAX_CHANNELS],
            dc_y: [0.0; MAX_CHANNELS],
            fir: [[0.0; 15]; MAX_CHANNELS],
            fir_pos: [0; MAX_CHANNELS],
            previous: [0.0; MAX_CHANNELS],
        }
    }
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        if self.mode == DistortionMode::Off {
            self.drive.advance(frames);
            self.mix.advance(frames);
            for frame in 0..frames {
                let gain = self.gain.next();
                buffer.channels[0][frame] *= gain;
                buffer.channels[1][frame] *= gain
            }
            return;
        }
        for frame in 0..frames {
            let drive = self.drive.next().clamp(1.0, 63.095_734);
            let gain = self.gain.next();
            let mix = self.mix.next().clamp(0.0, 1.0);
            let compensation = drive.sqrt().recip().clamp(0.12, 1.0);
            for channel in 0..MAX_CHANNELS {
                let dry = buffer.channels[channel][frame];
                let mut wet = 0.0;
                for phase in 0..4 {
                    let t = (phase + 1) as f32 * 0.25;
                    let up = self.previous[channel] + (dry - self.previous[channel]) * t;
                    let shaped =
                        distortion_shape(self.mode, up * drive, &mut self.tape_memory[channel]);
                    let position = self.fir_pos[channel];
                    self.fir[channel][position] = shaped;
                    wet = fir_read(&self.fir[channel], position);
                    self.fir_pos[channel] = increment_wrap(position, 15);
                }
                self.previous[channel] = dry;
                wet *= compensation;
                // The tube and exciter curves are intentionally asymmetric.
                // Remove their DC term after shaping, not before it.
                let dc = wet - self.dc_x[channel] + 0.99935 * self.dc_y[channel];
                self.dc_x[channel] = wet;
                self.dc_y[channel] = dc;
                buffer.channels[channel][frame] = denormal((dry * (1.0 - mix) + dc * mix) * gain);
            }
        }
    }
    fn reset(&mut self) {
        self.tape_memory = [0.0; MAX_CHANNELS];
        self.dc_x = [0.0; MAX_CHANNELS];
        self.dc_y = [0.0; MAX_CHANNELS];
        self.fir = [[0.0; 15]; MAX_CHANNELS];
        self.fir_pos = [0; MAX_CHANNELS];
        self.previous = [0.0; MAX_CHANNELS];
    }
}

struct SpectrumAnalyzer {
    samples: [f32; 256],
    window: [f32; 256],
    bin_positions: [f32; DISTORTION_SPECTRUM_BINS],
    fill: usize,
    wait: usize,
    interval: usize,
    bins: [f32; DISTORTION_SPECTRUM_BINS],
}
impl SpectrumAnalyzer {
    fn new() -> Self {
        Self {
            samples: [0.0; 256],
            window: [0.0; 256],
            bin_positions: [0.0; DISTORTION_SPECTRUM_BINS],
            fill: 0,
            wait: 0,
            interval: 1_600,
            bins: [0.0; DISTORTION_SPECTRUM_BINS],
        }
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.interval = (sample_rate / 30.0).round() as usize;
        for (index, target) in self.window.iter_mut().enumerate() {
            *target = 0.5 - 0.5 * (2.0 * PI * index as f32 / 255.0).cos()
        }
        let nyquist_bin = 127.0;
        for (index, target) in self.bin_positions.iter_mut().enumerate() {
            let frequency =
                20.0 * 1000.0_f32.powf(index as f32 / (DISTORTION_SPECTRUM_BINS - 1) as f32);
            *target = (frequency * 256.0 / sample_rate).clamp(0.0, nyquist_bin)
        }
    }
    fn push(&mut self, sample: f32) {
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        self.samples[self.fill] = sample;
        self.fill += 1;
        if self.fill == self.samples.len() {
            self.calculate_fft();
            self.fill = 0;
            self.wait = self.interval.saturating_sub(self.samples.len());
        }
    }
    fn calculate_fft(&mut self) {
        const N: usize = 256;
        let mut real = [0.0_f32; N];
        let mut imag = [0.0_f32; N];
        for (index, sample) in self.samples.iter().enumerate() {
            real[index] = *sample * self.window[index];
        }
        let mut j = 0;
        for i in 1..N {
            let mut bit = N >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                real.swap(i, j);
            }
        }
        let mut length = 2;
        while length <= N {
            let angle = -2.0 * PI / length as f32;
            let (step_sin, step_cos) = angle.sin_cos();
            for base in (0..N).step_by(length) {
                let (mut wr, mut wi) = (1.0_f32, 0.0_f32);
                for offset in 0..length / 2 {
                    let even = base + offset;
                    let odd = even + length / 2;
                    let tr = wr * real[odd] - wi * imag[odd];
                    let ti = wr * imag[odd] + wi * real[odd];
                    real[odd] = real[even] - tr;
                    imag[odd] = imag[even] - ti;
                    real[even] += tr;
                    imag[even] += ti;
                    let next_wr = wr * step_cos - wi * step_sin;
                    wi = wr * step_sin + wi * step_cos;
                    wr = next_wr;
                }
            }
            length *= 2;
        }
        let nyquist_bin = N / 2 - 1;
        for (index, target) in self.bins.iter_mut().enumerate() {
            let position = self.bin_positions[index];
            let lower = position.floor() as usize;
            let upper = (lower + 1).min(nyquist_bin);
            let magnitude = |bin: usize| {
                (real[bin] * real[bin] + imag[bin] * imag[bin]).sqrt() * 4.0 / N as f32
            };
            let value = magnitude(lower) + (magnitude(upper) - magnitude(lower)) * position.fract();
            *target = value.max(*target * 0.72);
        }
    }
    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.bins.fill(0.0);
        self.fill = 0;
        self.wait = 0;
    }
}

/// Three-band nonlinear processor. LR4 crossovers keep the clean bands phase
/// aligned; each nonlinear branch is independently oversampled four times.
pub struct Distortion {
    sample_rate: f32,
    split_low: f32,
    split_high: f32,
    low_split: Crossover,
    high_split: Crossover,
    low_phase: Crossover,
    bands: [AudioBuffer; 3],
    processors: [DistortionBand; 3],
    analyzer: SpectrumAnalyzer,
    bypassed: bool,
}
impl Distortion {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            split_low: 180.0,
            split_high: 4_500.0,
            low_split: Crossover::new(180.0),
            high_split: Crossover::new(4_500.0),
            low_phase: Crossover::new(4_500.0),
            bands: std::array::from_fn(|_| AudioBuffer::new()),
            processors: [
                DistortionBand::new(DistortionMode::Tube, 6.0, 0.75),
                DistortionBand::new(DistortionMode::Tape, 6.0, 0.75),
                DistortionBand::new(DistortionMode::Exciter, 6.0, 0.6),
            ],
            analyzer: SpectrumAnalyzer::new(),
            bypassed: false,
        }
    }
    fn update_crossovers(&mut self) {
        self.low_split.configure(self.split_low, self.sample_rate);
        self.high_split.configure(self.split_high, self.sample_rate);
        self.low_phase.configure(self.split_high, self.sample_rate);
    }
    fn set_band_param(&mut self, index: usize, parameter: &str, value: f32) {
        let Some(band) = self.processors.get_mut(index) else {
            return;
        };
        match parameter {
            "Mode" => band.mode = distortion_mode(value),
            "GainDb" => band.gain.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            "DriveDb" => band.drive.set_target(db_to_gain(value.clamp(0.0, 36.0))),
            "Mix" => band.mix.set_target(value.clamp(0.0, 1.0)),
            _ => {}
        }
    }
}
impl DspEffect for Distortion {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.update_crossovers();
        self.analyzer.prepare(sample_rate);
    }
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            for frame in 0..frames {
                self.analyzer
                    .push((buffer.channels[0][frame] + buffer.channels[1][frame]) * 0.5)
            }
            return;
        }
        let low_enabled = self.split_low > 20.5;
        let high_enabled = self.split_high < 19_950.0;
        for channel in 0..MAX_CHANNELS {
            for frame in 0..frames {
                let input = buffer.channels[channel][frame];
                let (low, upper) = if low_enabled {
                    self.low_split.process(channel, input)
                } else {
                    (0.0, input)
                };
                let (mid, high) = if high_enabled {
                    self.high_split.process(channel, upper)
                } else {
                    (upper, 0.0)
                };
                let low = if low_enabled && high_enabled {
                    let (late, early) = self.low_phase.process(channel, low);
                    late + early
                } else {
                    low
                };
                self.bands[0].channels[channel][frame] = low;
                self.bands[1].channels[channel][frame] = mid;
                self.bands[2].channels[channel][frame] = high;
            }
        }
        for (processor, band) in self.processors.iter_mut().zip(self.bands.iter_mut()) {
            processor.process(band, frames);
        }
        for frame in 0..frames {
            for channel in 0..MAX_CHANNELS {
                buffer.channels[channel][frame] = self.bands[0].channels[channel][frame]
                    + self.bands[1].channels[channel][frame]
                    + self.bands[2].channels[channel][frame];
            }
            self.analyzer
                .push((buffer.channels[0][frame] + buffer.channels[1][frame]) * 0.5)
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "splitLow" => {
                self.split_low = value.clamp(20.0, self.split_high / 1.05);
                self.update_crossovers()
            }
            "splitHigh" => {
                self.split_high = value.clamp(self.split_low * 1.05, 20_000.0);
                self.update_crossovers()
            }
            _ => {
                for (prefix, index) in [("low", 0), ("mid", 1), ("high", 2)] {
                    if let Some(parameter) = id.strip_prefix(prefix) {
                        self.set_band_param(index, parameter, value);
                        break;
                    }
                }
            }
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.low_split.reset();
        self.high_split.reset();
        self.low_phase.reset();
        for processor in &mut self.processors {
            processor.reset()
        }
        self.analyzer.reset();
    }
    fn latency_samples(&self) -> usize {
        2
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.analyzer.bins)
    }
}

fn distortion_mode(value: f32) -> DistortionMode {
    match value.round() as usize {
        1 => DistortionMode::Tube,
        2 => DistortionMode::Tape,
        3 => DistortionMode::Saturation,
        4 => DistortionMode::Exciter,
        _ => DistortionMode::Off,
    }
}

fn distortion_shape(mode: DistortionMode, input: f32, memory: &mut f32) -> f32 {
    match mode {
        DistortionMode::Off => input,
        DistortionMode::Tube => ((input + 0.17).tanh() - 0.168_381_05) / 0.971_647_8,
        DistortionMode::Tape => {
            // A compact hysteretic magnetic-memory approximation: the slowly
            // moving state changes the curve on rising and falling passages.
            *memory = denormal(*memory + (input - *memory) * 0.018);
            let magnetized = (input * 0.9 + *memory * 0.38).tanh();
            *memory = denormal(*memory + (magnetized - *memory) * 0.004);
            magnetized
        }
        DistortionMode::Saturation => input.tanh(),
        DistortionMode::Exciter => {
            let asymmetric = (input + 0.21).tanh() - 0.206_966_5;
            input + (asymmetric - input) * 1.65
        }
    }
}

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
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
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

pub fn create_effect(spec: &EffectSpec, sr: f32) -> Option<Box<dyn DspEffect>> {
    if spec.kind.starts_with("vst3:") || spec.kind.starts_with("clap:") {
        return super::plugin::create_external_effect(spec, sr);
    }
    let mut fx: Box<dyn DspEffect> = match spec.kind.as_str() {
        "builtin:eq" => Box::new(ParametricEq::new()),
        "builtin:eq8" => Box::new(ParametricEq::new_eight()),
        "builtin:compressor" => Box::new(Compressor::new()),
        "builtin:multiband-compressor" => Box::new(MultibandCompressor::new()),
        "builtin:utility" => Box::new(Utility::new()),
        "builtin:delay" => Box::new(Delay::new()),
        "builtin:reverb" => Box::new(Reverb::new()),
        "builtin:waveshaper" => Box::new(Waveshaper::new()),
        "builtin:distortion" => Box::new(Distortion::new()),
        "builtin:disperser" => Box::new(Disperser::new()),
        _ => return None,
    };
    fx.prepare(sr, MAX_BLOCK_SIZE, MAX_CHANNELS);
    for (id, v) in &spec.params {
        fx.set_param(id, *v)
    }
    fx.set_bypassed(spec.bypassed);
    Some(fx)
}
fn parse_band_param(id: &str, band_count: usize) -> (usize, &str) {
    if let Some(rest) = id.strip_prefix("band") {
        let mut p = rest.split('.');
        let idx = p.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        return (idx, p.next().unwrap_or("gain"));
    }
    let idx = if id.starts_with("low") {
        0
    } else if id.starts_with("high") {
        band_count.saturating_sub(1)
    } else {
        1
    };
    let p = if id.ends_with("Freq") {
        "freq"
    } else if id.ends_with("Gain") {
        "gain"
    } else {
        "q"
    };
    (idx, p)
}
fn kind_from_value(v: f32) -> FilterKind {
    match v as usize {
        1 => FilterKind::LowShelf,
        2 => FilterKind::HighShelf,
        3 => FilterKind::HighPass,
        4 => FilterKind::LowPass,
        5 => FilterKind::Notch,
        _ => FilterKind::Bell,
    }
}
fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}
#[inline(always)]
fn cubic_delay_read(line: &[f32], write: usize, delay: f32) -> f32 {
    let length = line.len();
    let mut position = write as f32 - delay;
    if position < 0.0 {
        position += length as f32
    }
    let base = position.floor() as usize;
    let fraction = position - base as f32;
    let previous = if base == 0 { length - 1 } else { base - 1 };
    let next = if base + 1 == length { 0 } else { base + 1 };
    let next_two = if next + 1 == length { 0 } else { next + 1 };
    catmull(
        line[previous],
        line[base],
        line[next],
        line[next_two],
        fraction,
    )
}
#[inline(always)]
fn fractional_read(line: &[f32], position: f32) -> f32 {
    let length = line.len();
    let mut wrapped = position;
    if wrapped < 0.0 {
        wrapped += length as f32
    } else if wrapped >= length as f32 {
        wrapped -= length as f32
    }
    let base = wrapped.floor() as usize;
    let fraction = wrapped - base as f32;
    let previous = if base == 0 { length - 1 } else { base - 1 };
    let next = if base + 1 == length { 0 } else { base + 1 };
    let next_two = if next + 1 == length { 0 } else { next + 1 };
    catmull(
        line[previous],
        line[base],
        line[next],
        line[next_two],
        fraction,
    )
}
#[inline(always)]
fn hadamard(v: &mut [f32; 8]) {
    let mut h = 1;
    while h < 8 {
        let step = h * 2;
        let mut i = 0;
        while i < 8 {
            for j in i..i + h {
                let a = v[j];
                let b = v[j + h];
                v[j] = a + b;
                v[j + h] = a - b
            }
            i += step
        }
        h *= 2
    }
    for value in v {
        *value *= 0.35355338
    }
}
#[inline(always)]
fn denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}
#[inline(always)]
fn increment_wrap(index: usize, length: usize) -> usize {
    let next = index + 1;
    if next == length {
        0
    } else {
        next
    }
}
#[inline(always)]
fn shape(curve: Curve, x: f32) -> f32 {
    match curve {
        Curve::SoftClip => x.tanh(),
        Curve::HardClip => x.clamp(-1.0, 1.0),
        // A sine soft clip is linear around zero and reaches the rails with a
        // zero derivative. Clamping its phase avoids foldback above the rail.
        Curve::Sine => (x.clamp(-1.0, 1.0) * PI * 0.5).sin(),
    }
}
#[inline(always)]
fn fir_read(history: &[f32; 15], position: usize) -> f32 {
    const TAPS: [(usize, f32); 9] = [
        (0, -0.001682),
        (2, 0.010703),
        (4, -0.049012),
        (6, 0.289991),
        (7, 0.499999),
        (8, 0.289991),
        (10, -0.049012),
        (12, 0.010703),
        (14, -0.001682),
    ];
    let mut sum = 0.0;
    for (offset, coefficient) in TAPS {
        sum += history[(position + 15 - offset) % 15] * coefficient
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smoother_converges() {
        let mut s = Smoother::new(0.0, 48000.0, 0.01);
        s.set_target(1.0);
        for _ in 0..480 {
            s.next();
        }
        assert!((s.current - 0.632).abs() < 0.01)
    }
    #[test]
    fn waveshaper_reports_latency() {
        let mut w = Waveshaper::new();
        w.oversample = 4;
        assert_eq!(w.latency_samples(), 2)
    }
    #[test]
    fn waveshaper_modes_are_finite_bounded_and_odd() {
        for curve in [Curve::SoftClip, Curve::HardClip, Curve::Sine] {
            for x in [-12.0_f32, -2.0, -1.0, -0.25, 0.0, 0.25, 1.0, 2.0, 12.0] {
                let y = shape(curve, x);
                assert!(y.is_finite());
                assert!(y.abs() <= 1.000_001);
                assert!((y + shape(curve, -x)).abs() < 1e-6)
            }
        }
    }
    #[test]
    fn sine_shaper_reaches_rails_without_foldback() {
        assert!((shape(Curve::Sine, 1.0) - 1.0).abs() < 1e-6);
        assert!((shape(Curve::Sine, 8.0) - 1.0).abs() < 1e-6);
        assert!((shape(Curve::Sine, -8.0) + 1.0).abs() < 1e-6)
    }
    #[test]
    fn eq_response_is_finite() {
        let mut e = ParametricEq::new();
        e.prepare(48000.0, 256, 2);
        assert!(e
            .response(256)
            .unwrap()
            .combined_db
            .iter()
            .all(|v| v.is_finite()))
    }
    #[test]
    fn every_eq_starts_with_an_enabled_low_cut() {
        for effect in [ParametricEq::new(), ParametricEq::new_eight()] {
            assert!(effect.bands[0].enabled);
            assert!(matches!(effect.bands[0].kind, FilterKind::HighPass));
        }
    }
    #[test]
    fn eight_band_eq_response_is_finite() {
        let mut effect = ParametricEq::new_eight();
        effect.prepare(48_000.0, 256, 2);
        effect.set_param("band7.enabled", 1.0);
        effect.set_param("band7.type", 4.0);
        assert_eq!(effect.bands.len(), 8);
        assert!(effect
            .response(256)
            .unwrap()
            .combined_db
            .iter()
            .all(|value| value.is_finite()))
    }
    #[test]
    fn utility_mono_removes_pure_side_signal() {
        let mut effect = Utility::new();
        effect.set_param("mono", 1.0);
        let mut buffer = AudioBuffer::new();
        buffer.channels[0][0] = 1.0;
        buffer.channels[1][0] = -1.0;
        effect.process(&mut buffer, 1);
        assert!(buffer.channels[0][0].abs() < 1e-6);
        assert!(buffer.channels[1][0].abs() < 1e-6)
    }
    #[test]
    fn compressor_uses_external_sidechain_detector() {
        let mut effect = Compressor::new();
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, 2);
        effect.set_param("threshold", -30.0);
        effect.set_param("ratio", 20.0);
        effect.set_param("attackMs", 1.0);
        let mut buffer = AudioBuffer::new();
        let mut detector = AudioBuffer::new();
        buffer.channels[0][..MAX_BLOCK_SIZE].fill(0.25);
        buffer.channels[1][..MAX_BLOCK_SIZE].fill(0.25);
        detector.channels[0][..MAX_BLOCK_SIZE].fill(1.0);
        detector.channels[1][..MAX_BLOCK_SIZE].fill(1.0);
        effect.process_with_sidechain(&mut buffer, Some(&detector), MAX_BLOCK_SIZE);
        assert!(buffer.channels[0][MAX_BLOCK_SIZE - 1] < 0.1)
    }
    #[test]
    fn multiband_compressor_processes_finite_audio() {
        let mut effect = MultibandCompressor::new();
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let sample = (2.0 * PI * 440.0 * frame as f32 / 48_000.0).sin() * 0.25;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        effect.process(&mut buffer, MAX_BLOCK_SIZE);
        assert!(buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            .all(|sample| sample.is_finite()));
        let levels = effect.multiband_levels().expect("multiband meter");
        assert!(levels.iter().flatten().all(|level| level.is_finite()));
        assert!(levels.iter().flatten().any(|level| *level > 0.0))
    }
    #[test]
    fn multiband_low_split_respects_requested_range() {
        let mut effect = MultibandCompressor::new();
        effect.set_param("splitLow", 500.0);
        assert_eq!(effect.split_low, 150.0);
        effect.set_param("splitLow", 1.0);
        assert_eq!(effect.split_low, 20.0)
    }
    #[test]
    fn distortion_modes_process_finite_audio_and_publish_fft() {
        for mode in 0..=4 {
            let mut effect = Distortion::new();
            effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
            for band in ["lowMode", "midMode", "highMode"] {
                effect.set_param(band, mode as f32)
            }
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let sample = (2.0 * PI * 440.0 * frame as f32 / 48_000.0).sin() * 0.25;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            effect.process(&mut buffer, MAX_BLOCK_SIZE);
            assert!(buffer
                .channels
                .iter()
                .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
                .all(|sample| sample.is_finite()));
            let spectrum = effect.effect_spectrum().expect("distortion FFT");
            assert!(spectrum.iter().all(|value| value.is_finite()));
            assert!(spectrum.iter().any(|value| *value > 0.0));
        }
    }
    #[test]
    fn distortion_crossovers_clamp_and_disable_at_edges() {
        let mut effect = Distortion::new();
        effect.set_param("splitLow", -100.0);
        effect.set_param("splitHigh", 40_000.0);
        assert_eq!(effect.split_low, 20.0);
        assert_eq!(effect.split_high, 20_000.0);
        effect.set_param("splitLow", 10_000.0);
        assert!(effect.split_low <= effect.split_high / 1.05)
    }
    #[test]
    fn disperser_allpass_section_has_unity_magnitude() {
        let mut section = AllpassSection::new();
        section.configure(3_050.0, 1.2, 48_000.0);
        for hz in [20.0_f64, 100.0, 1_000.0, 3_050.0, 10_000.0, 20_000.0] {
            let omega = 2.0 * std::f64::consts::PI * hz / 48_000.0;
            let (sin_1, cos_1) = omega.sin_cos();
            let (sin_2, cos_2) = (2.0 * omega).sin_cos();
            let numerator_real = section.b0 + section.b1 * cos_1 + section.b2 * cos_2;
            let numerator_imag = -section.b1 * sin_1 - section.b2 * sin_2;
            let denominator_real = 1.0 + section.a1 * cos_1 + section.a2 * cos_2;
            let denominator_imag = -section.a1 * sin_1 - section.a2 * sin_2;
            let magnitude = (numerator_real * numerator_real + numerator_imag * numerator_imag)
                .sqrt()
                / (denominator_real * denominator_real + denominator_imag * denominator_imag)
                    .sqrt();
            assert!(
                (magnitude - 1.0).abs() < 1e-10,
                "{hz} Hz magnitude was {magnitude}"
            )
        }
    }
    #[test]
    fn disperser_amount_controls_order_and_output_stays_finite() {
        let mut effect = Disperser::new();
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        effect.set_param("amount", 1.0);
        effect.set_param("frequency", 80.0);
        effect.set_param("pinch", 1.0);
        assert_eq!(effect.stages, MAX_DISPERSER_STAGES);
        let mut buffer = AudioBuffer::new();
        buffer.channels[0][0] = 1.0;
        buffer.channels[1][0] = 1.0;
        effect.process(&mut buffer, MAX_BLOCK_SIZE);
        assert!(buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            .all(|sample| sample.is_finite()));

        effect.set_param("amount", 0.0);
        let mut dry = AudioBuffer::new();
        dry.channels[0][0] = 0.25;
        dry.channels[1][0] = -0.25;
        effect.process(&mut dry, 1);
        assert_eq!(dry.channels[0][0], 0.25);
        assert_eq!(dry.channels[1][0], -0.25)
    }
    #[test]
    fn eq_and_disperser_publish_finite_spectra() {
        let mut effects: [Box<dyn DspEffect>; 2] = [
            Box::new(ParametricEq::new_eight()),
            Box::new(Disperser::new()),
        ];
        for effect in &mut effects {
            effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let sample = (2.0 * PI * 880.0 * frame as f32 / 48_000.0).sin() * 0.2;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            effect.process(&mut buffer, MAX_BLOCK_SIZE);
            let spectrum = effect.effect_spectrum().expect("effect spectrum");
            assert!(spectrum.iter().all(|value| value.is_finite()));
            assert!(spectrum.iter().any(|value| *value > 0.0));
        }
    }
    #[test]
    fn optimized_delay_reader_matches_wrapped_reference() {
        let line = (0..17)
            .map(|index| (index as f32 * 0.37).sin())
            .collect::<Vec<_>>();
        for write in [0, 1, 8, 16] {
            for delay in [1.0_f32, 2.25, 8.5, 13.0] {
                let position = (write as f32 - delay).rem_euclid(line.len() as f32);
                let base = position.floor() as usize;
                let fraction = position - base as f32;
                let reference = catmull(
                    line[(base + line.len() - 1) % line.len()],
                    line[base],
                    line[(base + 1) % line.len()],
                    line[(base + 2) % line.len()],
                    fraction,
                );
                assert!((cubic_delay_read(&line, write, delay) - reference).abs() < 1e-6)
            }
        }
    }
    #[test]
    fn sparse_oversampling_fir_matches_full_reference() {
        const COEFFICIENTS: [f32; 15] = [
            -0.001682, 0.0, 0.010703, 0.0, -0.049012, 0.0, 0.289991, 0.499999, 0.289991, 0.0,
            -0.049012, 0.0, 0.010703, 0.0, -0.001682,
        ];
        let history = std::array::from_fn(|index| (index as f32 * 0.61).cos());
        for position in 0..15 {
            let reference = COEFFICIENTS
                .iter()
                .enumerate()
                .map(|(offset, coefficient)| history[(position + 15 - offset) % 15] * coefficient)
                .sum::<f32>();
            assert!((fir_read(&history, position) - reference).abs() < 1e-6)
        }
    }
}
