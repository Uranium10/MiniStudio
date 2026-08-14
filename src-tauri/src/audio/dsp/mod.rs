// Allocation-free native DSP effects shared by realtime and offline renderers.
use super::{
    instrument::{NoteEvent, NoteEventKind},
    types::{db_to_gain, EffectSpec, EqFrequencyResponse},
};
use super::{MAX_BLOCK_SIZE, MAX_CHANNELS};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

pub const DISTORTION_SPECTRUM_BINS: usize = 48;
pub const LIMITER_METER_VALUES: usize = 7;

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
    /// `events` are sorted by ascending `sample_offset`; offsets are exact
    /// positions inside this block. Effects that do not consume MIDI receive
    /// an empty slice so the realtime path never copies irrelevant events.
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize);
    fn process_with_sidechain(
        &mut self,
        events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        let _ = sidechain;
        self.process(events, buffer, frames)
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
    fn wants_midi(&self) -> bool {
        false
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
    fn limiter_metrics(&self) -> Option<[f32; LIMITER_METER_VALUES]> {
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
    fn process(&mut self, events: &[NoteEvent], b: &mut AudioBuffer, n: usize) {
        self.process_with_sidechain(events, b, None, n)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
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
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
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
            compressor.process(&[], band, frames)
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
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
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
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
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

#[derive(Clone, Copy)]
enum LimiterMode {
    Clean,
    Transparent,
    Punch,
    Loud,
}

/// ITU-R BS.1770 K-weighted loudness meter. The 400 ms and 3 s windows are
/// maintained as rolling sums; integrated loudness uses overlapping 400 ms
/// blocks and the absolute/relative gates from the recommendation.
struct LoudnessMeter {
    shelf: Biquad,
    high_pass: Biquad,
    powers: Vec<f32>,
    position: usize,
    count: usize,
    momentary_samples: usize,
    short_samples: usize,
    momentary_sum: f64,
    short_sum: f64,
    hop_samples: usize,
    hop_count: usize,
    blocks: Vec<f32>,
    block_count: usize,
    momentary_lufs: f32,
    short_lufs: f32,
    integrated_lufs: f32,
}
impl LoudnessMeter {
    fn new() -> Self {
        Self {
            shelf: Biquad::new(),
            high_pass: Biquad::new(),
            powers: Vec::new(),
            position: 0,
            count: 0,
            momentary_samples: 19_200,
            short_samples: 144_000,
            momentary_sum: 0.0,
            short_sum: 0.0,
            hop_samples: 4_800,
            hop_count: 0,
            // One hour of 100 ms blocks, allocated off the audio callback.
            blocks: vec![0.0; 36_000],
            block_count: 0,
            momentary_lufs: -120.0,
            short_lufs: -120.0,
            integrated_lufs: -120.0,
        }
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.momentary_samples = (sample_rate * 0.4).round().max(1.0) as usize;
        self.short_samples = (sample_rate * 3.0).round().max(1.0) as usize;
        self.hop_samples = (sample_rate * 0.1).round().max(1.0) as usize;
        self.powers.resize(self.short_samples, 0.0);
        self.shelf.configure(
            FilterKind::HighShelf,
            1_681.9745,
            3.999_843_8,
            0.707_175_25,
            sample_rate,
        );
        self.high_pass
            .configure(FilterKind::HighPass, 38.13547, 0.0, 0.500_327, sample_rate);
        self.reset();
    }
    #[inline(always)]
    fn push(&mut self, left: f32, right: f32) {
        let wl = self.high_pass.process(0, self.shelf.process(0, left));
        let wr = self.high_pass.process(1, self.shelf.process(1, right));
        let power = wl * wl + wr * wr;
        let old_short = self.powers[self.position];
        self.short_sum += f64::from(power - old_short);
        if self.count >= self.momentary_samples {
            let old_momentary = self.powers[(self.position + self.short_samples
                - self.momentary_samples)
                % self.short_samples];
            self.momentary_sum += f64::from(power - old_momentary);
        } else {
            self.momentary_sum += f64::from(power);
        }
        self.powers[self.position] = power;
        self.position = (self.position + 1) % self.short_samples;
        self.count = (self.count + 1).min(self.short_samples);
        self.hop_count += 1;
        if self.hop_count >= self.hop_samples {
            self.hop_count = 0;
            let momentary_count = self.count.min(self.momentary_samples).max(1);
            self.momentary_lufs = power_to_lufs(self.momentary_sum / momentary_count as f64);
            self.short_lufs = power_to_lufs(self.short_sum / self.count.max(1) as f64);
            if self.count >= self.momentary_samples {
                let energy = (self.momentary_sum / self.momentary_samples as f64) as f32;
                let index = self.block_count % self.blocks.len();
                self.blocks[index] = energy;
                self.block_count += 1;
                // Re-evaluate the relative gate once per second, not per sample.
                if self.block_count % 10 == 0 {
                    self.integrated_lufs =
                        integrated_lufs(&self.blocks, self.block_count.min(self.blocks.len()));
                }
            }
        }
    }
    fn reset(&mut self) {
        self.shelf.reset();
        self.high_pass.reset();
        self.powers.fill(0.0);
        self.position = 0;
        self.count = 0;
        self.momentary_sum = 0.0;
        self.short_sum = 0.0;
        self.hop_count = 0;
        self.blocks.fill(0.0);
        self.block_count = 0;
        self.momentary_lufs = -120.0;
        self.short_lufs = -120.0;
        self.integrated_lufs = -120.0;
    }
}

pub struct MasteringLimiter {
    sample_rate: f32,
    input: Smoother,
    output: Smoother,
    release_ms: f32,
    stereo_link: f32,
    true_peak: bool,
    mode: LimiterMode,
    delay: [Vec<f32>; MAX_CHANNELS],
    delay_position: usize,
    lookahead: usize,
    detector_history: [[f32; 4]; MAX_CHANNELS],
    output_history: [[f32; 4]; MAX_CHANNELS],
    envelope: [f32; MAX_CHANNELS],
    hold: [usize; MAX_CHANNELS],
    input_peak: f32,
    output_peak: f32,
    true_peak_level: f32,
    gain_reduction_db: f32,
    loudness: LoudnessMeter,
    bypassed: bool,
}
impl MasteringLimiter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            input: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(db_to_gain(-1.0), 48_000.0, 0.01),
            release_ms: 120.0,
            stereo_link: 1.0,
            true_peak: true,
            mode: LimiterMode::Clean,
            delay: [Vec::new(), Vec::new()],
            delay_position: 0,
            lookahead: 240,
            detector_history: [[0.0; 4]; MAX_CHANNELS],
            output_history: [[0.0; 4]; MAX_CHANNELS],
            envelope: [1.0; MAX_CHANNELS],
            hold: [0; MAX_CHANNELS],
            input_peak: 0.0,
            output_peak: 0.0,
            true_peak_level: 0.0,
            gain_reduction_db: 0.0,
            loudness: LoudnessMeter::new(),
            bypassed: false,
        }
    }
    fn release_coefficient(&self) -> f32 {
        let scale = match self.mode {
            LimiterMode::Transparent => 1.5,
            LimiterMode::Punch => 0.55,
            LimiterMode::Loud => 0.7,
            LimiterMode::Clean => 1.0,
        };
        (-1.0 / (self.release_ms.max(5.0) * scale * 0.001 * self.sample_rate)).exp()
    }
}
impl DspEffect for MasteringLimiter {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.lookahead = (sample_rate * 0.005).round().max(1.0) as usize;
        self.delay = [vec![0.0; self.lookahead + 1], vec![0.0; self.lookahead + 1]];
        self.input = Smoother::new(self.input.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        self.loudness.prepare(sample_rate);
        self.reset();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        let release = self.release_coefficient();
        let meter_decay = 10.0_f32.powf(-18.0 / 20.0 / self.sample_rate);
        for frame in 0..frames {
            let input_gain = if self.bypassed {
                1.0
            } else {
                self.input.next()
            };
            let ceiling = if self.bypassed {
                1.0
            } else {
                self.output.next().clamp(0.000_1, 1.0)
            };
            // A small reconstruction guard keeps the interpolated waveform below
            // the requested dBTP ceiling instead of merely clipping sample peaks.
            let detector_ceiling = if self.true_peak {
                ceiling * 0.9975
            } else {
                ceiling
            };
            let mut input = [0.0; MAX_CHANNELS];
            let mut independent = [1.0; MAX_CHANNELS];
            for channel in 0..MAX_CHANNELS {
                input[channel] = buffer.channels[channel][frame] * input_gain;
                self.input_peak = self
                    .input_peak
                    .mul_add(meter_decay, 0.0)
                    .max(input[channel].abs());
                let history = &mut self.detector_history[channel];
                history.rotate_left(1);
                history[3] = input[channel];
                let peak = if self.true_peak {
                    interpolated_peak(*history, 4)
                } else {
                    input[channel].abs()
                };
                independent[channel] = (detector_ceiling / peak.max(detector_ceiling)).min(1.0);
            }
            let linked = independent[0].min(independent[1]);
            for channel in 0..MAX_CHANNELS {
                let target = if self.bypassed {
                    1.0
                } else {
                    independent[channel] + (linked - independent[channel]) * self.stereo_link
                };
                if target < self.envelope[channel] {
                    self.envelope[channel] = target;
                    self.hold[channel] = self.lookahead;
                } else if self.hold[channel] > 0 {
                    self.hold[channel] -= 1;
                } else {
                    self.envelope[channel] = target + (self.envelope[channel] - target) * release;
                }
                self.delay[channel][self.delay_position] = input[channel];
                let read = (self.delay_position + 1) % self.delay[channel].len();
                let mut sample = self.delay[channel][read] * self.envelope[channel];
                if !self.bypassed && matches!(self.mode, LimiterMode::Loud) {
                    let drive = 1.35;
                    sample = (sample * drive).tanh() / drive.tanh();
                }
                // The final guard is intentionally non-colouring in normal operation;
                // it only catches numerical/sample peaks missed by interpolation.
                if !self.bypassed {
                    sample = sample.clamp(-ceiling, ceiling)
                }
                buffer.channels[channel][frame] = sample;
                self.output_peak = self.output_peak.mul_add(meter_decay, 0.0).max(sample.abs());
                let history = &mut self.output_history[channel];
                history.rotate_left(1);
                history[3] = sample;
                self.true_peak_level = self
                    .true_peak_level
                    .mul_add(meter_decay, 0.0)
                    .max(interpolated_peak(*history, 4));
            }
            self.delay_position = (self.delay_position + 1) % self.delay[0].len();
            let minimum_gain = self.envelope[0].min(self.envelope[1]);
            self.gain_reduction_db =
                (-20.0 * minimum_gain.max(1e-9).log10()).max(self.gain_reduction_db * meter_decay);
            self.loudness
                .push(buffer.channels[0][frame], buffer.channels[1][frame]);
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "inputDb" | "inputGainDb" => {
                self.input.set_target(db_to_gain(value.clamp(-24.0, 24.0)))
            }
            "outputDb" | "outputGainDb" | "ceilingDb" => {
                self.output.set_target(db_to_gain(value.clamp(-24.0, 0.0)))
            }
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "release" => self.release_ms = (value * 1_000.0).clamp(5.0, 2_000.0),
            "stereoLink" => self.stereo_link = value.clamp(0.0, 1.0),
            "truePeak" => self.true_peak = value >= 0.5,
            "algorithm" | "mode" => {
                self.mode = match value.round() as i32 {
                    1 => LimiterMode::Transparent,
                    2 => LimiterMode::Punch,
                    3 => LimiterMode::Loud,
                    _ => LimiterMode::Clean,
                }
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for channel in &mut self.delay {
            channel.fill(0.0)
        }
        self.delay_position = 0;
        self.detector_history = [[0.0; 4]; MAX_CHANNELS];
        self.output_history = [[0.0; 4]; MAX_CHANNELS];
        self.envelope = [1.0; MAX_CHANNELS];
        self.hold = [0; MAX_CHANNELS];
        self.input_peak = 0.0;
        self.output_peak = 0.0;
        self.true_peak_level = 0.0;
        self.gain_reduction_db = 0.0;
        self.loudness.reset();
    }
    fn latency_samples(&self) -> usize {
        self.lookahead
    }
    fn limiter_metrics(&self) -> Option<[f32; LIMITER_METER_VALUES]> {
        Some([
            amplitude_to_db(self.input_peak),
            amplitude_to_db(self.output_peak),
            self.gain_reduction_db,
            amplitude_to_db(self.true_peak_level),
            self.loudness.momentary_lufs,
            self.loudness.short_lufs,
            self.loudness.integrated_lufs,
        ])
    }
}

struct VocoderBand {
    analysis: Biquad,
    carrier: Biquad,
    envelope: f32,
}
impl VocoderBand {
    fn new() -> Self {
        Self {
            analysis: Biquad::new(),
            carrier: Biquad::new(),
            envelope: 0.0,
        }
    }
}

/// Constant-Q channel vocoder. Sidechain mode analyses the external modulator
/// and filters the main input; oscillator/noise modes analyse the main input
/// and use the selected internal excitation source.
pub struct Vocoder {
    sample_rate: f32,
    source: u8,
    band_count: usize,
    pitch_hz: f32,
    attack_ms: f32,
    release_ms: f32,
    formant_shift: f32,
    bandwidth: f32,
    mix: Smoother,
    output: Smoother,
    phase: f32,
    noise: u32,
    bands: Vec<VocoderBand>,
    bypassed: bool,
}
impl Vocoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            source: 3,
            band_count: 16,
            pitch_hz: 110.0,
            attack_ms: 5.0,
            release_ms: 90.0,
            formant_shift: 0.0,
            bandwidth: 1.0,
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            phase: 0.0,
            noise: 0x9e37_79b9,
            bands: (0..24).map(|_| VocoderBand::new()).collect(),
            bypassed: false,
        }
    }
    fn configure_bands(&mut self) {
        let ratio = 2.0_f32.powf(self.formant_shift / 12.0);
        let q = (self.band_count as f32 / 4.0 * self.bandwidth.recip()).clamp(1.0, 12.0);
        for (index, band) in self.bands.iter_mut().enumerate() {
            let t = index as f32 / (self.band_count.saturating_sub(1)).max(1) as f32;
            let carrier_frequency = 80.0 * (12_000.0_f32 / 80.0).powf(t);
            let analysis_frequency =
                (carrier_frequency / ratio).clamp(40.0, self.sample_rate * 0.45);
            band.analysis.configure(
                FilterKind::BandPass,
                analysis_frequency,
                0.0,
                q,
                self.sample_rate,
            );
            band.carrier.configure(
                FilterKind::BandPass,
                carrier_frequency,
                0.0,
                q,
                self.sample_rate,
            );
        }
    }
    #[inline(always)]
    fn oscillator(&mut self) -> f32 {
        self.phase += self.pitch_hz / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0
        }
        match self.source {
            1 => (2.0 * PI * self.phase).sin(),
            2 => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => self.phase * 2.0 - 1.0,
            4 => {
                self.noise ^= self.noise << 13;
                self.noise ^= self.noise >> 17;
                self.noise ^= self.noise << 5;
                self.noise as f32 / u32::MAX as f32 * 2.0 - 1.0
            }
            _ => 0.0,
        }
    }
}
impl DspEffect for Vocoder {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.mix = Smoother::new(self.mix.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        self.configure_bands();
        self.reset();
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(events, buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        if self.bypassed {
            return;
        }
        let attack = (-1.0 / (self.attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let release = (-1.0 / (self.release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();
        let normalization = 3.5 / (self.band_count as f32).sqrt();
        for frame in 0..frames {
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let internal = self.oscillator();
            let (modulator, carrier) = if self.source == 0 {
                if let Some(external) = sidechain {
                    (
                        (external.channels[0][frame] + external.channels[1][frame]) * 0.5,
                        dry,
                    )
                } else {
                    ((dry[0] + dry[1]) * 0.5, [internal; MAX_CHANNELS])
                }
            } else {
                ((dry[0] + dry[1]) * 0.5, [internal; MAX_CHANNELS])
            };
            let mut wet = [0.0; MAX_CHANNELS];
            for band in self.bands.iter_mut().take(self.band_count) {
                let detected = band.analysis.process(0, modulator).abs();
                let coefficient = if detected > band.envelope {
                    attack
                } else {
                    release
                };
                band.envelope = detected + (band.envelope - detected) * coefficient;
                for channel in 0..MAX_CHANNELS {
                    wet[channel] += band.carrier.process(channel, carrier[channel]) * band.envelope;
                }
            }
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                buffer.channels[channel][frame] = denormal(
                    (dry[channel] * (1.0 - mix) + wet[channel] * normalization * mix) * output,
                )
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "source" | "modulator" => self.source = value.round().clamp(0.0, 4.0) as u8,
            "bands" => {
                self.band_count = match value.round() as usize {
                    0..=10 => 8,
                    11..=14 => 12,
                    15..=20 => 16,
                    _ => 24,
                };
                self.configure_bands()
            }
            "pitchHz" | "frequency" => self.pitch_hz = value.clamp(20.0, 2_000.0),
            "attackMs" => self.attack_ms = value.clamp(0.1, 200.0),
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "formantShift" => {
                self.formant_shift = value.clamp(-24.0, 24.0);
                self.configure_bands()
            }
            "bandwidth" => {
                self.bandwidth = value.clamp(0.5, 2.0);
                self.configure_bands()
            }
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.analysis.reset();
            band.carrier.reset();
            band.envelope = 0.0
        }
        self.phase = 0.0;
    }
}

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
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.flow)
    }
}

pub struct UpwardCompressor {
    sample_rate: f32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    range_db: f32,
    stereo_link: f32,
    mix: Smoother,
    output: Smoother,
    detector: [f32; MAX_CHANNELS],
    gain: [f32; MAX_CHANNELS],
    bypassed: bool,
}
impl UpwardCompressor {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            threshold_db: -32.0,
            ratio: 3.0,
            attack_ms: 35.0,
            release_ms: 240.0,
            range_db: 12.0,
            stereo_link: 1.0,
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            detector: [0.0; MAX_CHANNELS],
            gain: [1.0; MAX_CHANNELS],
            bypassed: false,
        }
    }
    fn requested_gain(&self, level: f32) -> f32 {
        let level_db = amplitude_to_db(level);
        if level_db >= self.threshold_db {
            return 1.0;
        }
        let gain_db =
            ((self.threshold_db - level_db) * (1.0 - self.ratio.recip())).clamp(0.0, self.range_db);
        db_to_gain(gain_db)
    }
}
impl DspEffect for UpwardCompressor {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.mix = Smoother::new(self.mix.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        let detector_coefficient = (-1.0 / (0.01 * self.sample_rate)).exp();
        let attack = (-1.0 / (self.attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let release = (-1.0 / (self.release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();
        for frame in 0..frames {
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let mut target = [1.0; MAX_CHANNELS];
            for channel in 0..MAX_CHANNELS {
                self.detector[channel] = dry[channel] * dry[channel]
                    + (self.detector[channel] - dry[channel] * dry[channel]) * detector_coefficient;
                target[channel] = self.requested_gain(self.detector[channel].sqrt());
            }
            let linked = target[0].min(target[1]);
            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                let requested = target[channel] + (linked - target[channel]) * self.stereo_link;
                let coefficient = if requested > self.gain[channel] {
                    attack
                } else {
                    release
                };
                self.gain[channel] = requested + (self.gain[channel] - requested) * coefficient;
                let wet = dry[channel] * self.gain[channel];
                buffer.channels[channel][frame] =
                    denormal((dry[channel] * (1.0 - mix) + wet * mix) * output);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "threshold" | "thresholdDb" => self.threshold_db = value.clamp(-72.0, -6.0),
            "ratio" => self.ratio = value.clamp(1.0, 20.0),
            "attackMs" => self.attack_ms = value.clamp(0.1, 500.0),
            "releaseMs" => self.release_ms = value.clamp(5.0, 2_000.0),
            "rangeDb" | "range" => self.range_db = value.clamp(0.0, 36.0),
            "stereoLink" => self.stereo_link = value.clamp(0.0, 1.0),
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.detector = [0.0; MAX_CHANNELS];
        self.gain = [1.0; MAX_CHANNELS]
    }
}

const ROBOTER_YIN_FRAME: usize = 1024;
const ROBOTER_YIN_MAX_LAG: usize = 320;
const ROBOTER_VOICE_COUNT: usize = 6;
const ROBOTER_HARMONY_COUNT: usize = 5;
const ROBOTER_ANALYSIS_HOP: usize = 256;

const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const MAJOR_SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR_SCALE: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
const ROBOTER_DETUNE_CENTS: [f32; ROBOTER_VOICE_COUNT] = [0.0, -4.0, 3.5, -6.5, 7.0, -3.0];
const ROBOTER_DRIFT_HZ: [f32; ROBOTER_VOICE_COUNT] = [0.0, 0.13, 0.17, 0.21, 0.25, 0.29];
const ROBOTER_EXTRA_DELAY_SEC: [f32; ROBOTER_VOICE_COUNT] =
    [0.0, 0.010, 0.006, 0.009, 0.013, 0.015];

#[derive(Clone, Copy, Debug, Default)]
struct PitchEstimate {
    frequency: f32,
    confidence: f32,
    aperiodicity: f32,
    voiced: bool,
}

#[derive(Clone, Copy)]
struct ChromaEvent {
    pitch_class: u8,
    weight: f32,
}
impl ChromaEvent {
    const EMPTY: Self = Self {
        pitch_class: u8::MAX,
        weight: 0.0,
    };
}

#[derive(Clone, Copy)]
struct RoboterVoice {
    phase: f32,
    drift_phase: f32,
    current_ratio: f32,
    target_ratio: f32,
    gain: f32,
    target_gain: f32,
    pan: f32,
    target_pan: f32,
    low_state: [f32; MAX_CHANNELS],
}
impl RoboterVoice {
    const fn new() -> Self {
        Self {
            phase: 0.0,
            drift_phase: 0.0,
            current_ratio: 1.0,
            target_ratio: 1.0,
            gain: 0.0,
            target_gain: 0.0,
            pan: 0.0,
            target_pan: 0.0,
            low_state: [0.0; MAX_CHANNELS],
        }
    }

    #[inline(always)]
    fn render(
        &mut self,
        delay: &[Vec<f32>; MAX_CHANNELS],
        write_position: usize,
        base_delay: usize,
        period: f32,
        sample_rate: f32,
        voice_index: usize,
        retime_seconds: f32,
    ) -> [f32; MAX_CHANNELS] {
        if retime_seconds <= 0.000_05 {
            self.current_ratio = self.target_ratio;
        } else {
            let coefficient = (-1.0 / (retime_seconds * sample_rate)).exp();
            self.current_ratio =
                self.target_ratio + (self.current_ratio - self.target_ratio) * coefficient;
        }
        self.drift_phase = (self.drift_phase + ROBOTER_DRIFT_HZ[voice_index] / sample_rate).fract();
        let drift_cents = (2.0 * PI * self.drift_phase).sin() * 1.5;
        let ratio = self.current_ratio
            * 2.0_f32.powf((ROBOTER_DETUNE_CENTS[voice_index] + drift_cents) / 1200.0);
        let window = (period * 2.0).clamp(sample_rate / 1000.0 * 2.0, sample_rate / 80.0 * 2.0);
        self.phase = (self.phase + (1.0 - ratio) / window).rem_euclid(1.0);
        let phase_b = (self.phase + 0.5).fract();
        let weight_a = 0.5 - 0.5 * (2.0 * PI * self.phase).cos();
        let weight_b = 1.0 - weight_a;
        let extra_delay = ROBOTER_EXTRA_DELAY_SEC[voice_index] * sample_rate;
        let delay_a = base_delay as f32 + extra_delay + self.phase * window;
        let delay_b = base_delay as f32 + extra_delay + phase_b * window;
        std::array::from_fn(|channel| {
            denormal(
                fractional_read(&delay[channel], write_position as f32 - delay_a) * weight_a
                    + fractional_read(&delay[channel], write_position as f32 - delay_b) * weight_b,
            )
        })
    }

    #[inline(always)]
    fn advance_mix(&mut self, sample_rate: f32) {
        let fade_step = 1.0 / (0.04 * sample_rate).max(1.0);
        if self.gain < self.target_gain {
            self.gain = (self.gain + fade_step).min(self.target_gain)
        } else {
            self.gain = (self.gain - fade_step).max(self.target_gain)
        }
        let pan_step = 1.0 / (0.03 * sample_rate).max(1.0);
        self.pan += (self.target_pan - self.pan).clamp(-pan_step, pan_step)
    }

    #[inline(always)]
    fn mono_bass(&mut self, input: [f32; MAX_CHANNELS], sample_rate: f32) -> [f32; 2] {
        let coefficient = 1.0 - (-2.0 * PI * 200.0 / sample_rate).exp();
        for channel in 0..MAX_CHANNELS {
            self.low_state[channel] += (input[channel] - self.low_state[channel]) * coefficient
        }
        let mono_low = (self.low_state[0] + self.low_state[1]) * 0.5;
        [
            input[0] - self.low_state[0] + mono_low,
            input[1] - self.low_state[1] + mono_low,
        ]
    }
}

/// Monophonic vocal pitch correction and automatic diatonic harmonization.
/// Analysis uses a 1024-sample downsampled YIN frame at a 256-input-sample hop.
/// Six fixed PSOLA-style granular voices are allocated in `prepare`; processing
/// only mutates those buffers and never creates or destroys a voice.
pub struct Roboter {
    sample_rate: f32,
    amount: f32,
    number: usize,
    downsample_factor: usize,
    downsample_sum: f32,
    downsample_count: usize,
    samples_since_analysis: usize,
    pitch_ring: [f32; ROBOTER_YIN_FRAME],
    pitch_position: usize,
    pitch_filled: usize,
    raw_midi: f32,
    smoothed_midi: f32,
    detected_hz: f32,
    pitch_confidence: f32,
    aperiodicity: f32,
    voiced: bool,
    chroma_histogram: [f32; 12],
    chroma_history: Vec<ChromaEvent>,
    chroma_position: usize,
    chroma_weight: f32,
    key_root: usize,
    key_minor: bool,
    key_valid: bool,
    key_confidence: f32,
    active_intervals: [f32; ROBOTER_HARMONY_COUNT],
    voices: [RoboterVoice; ROBOTER_VOICE_COUNT],
    delay: [Vec<f32>; MAX_CHANNELS],
    write_position: usize,
    latency: usize,
    bypassed: bool,
}
impl Roboter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            amount: 0.72,
            number: 0,
            downsample_factor: 2,
            downsample_sum: 0.0,
            downsample_count: 0,
            samples_since_analysis: 0,
            pitch_ring: [0.0; ROBOTER_YIN_FRAME],
            pitch_position: 0,
            pitch_filled: 0,
            raw_midi: 69.0,
            smoothed_midi: 69.0,
            detected_hz: 440.0,
            pitch_confidence: 0.0,
            aperiodicity: 1.0,
            voiced: false,
            chroma_histogram: [0.0; 12],
            chroma_history: Vec::new(),
            chroma_position: 0,
            chroma_weight: 0.0,
            key_root: 0,
            key_minor: false,
            key_valid: false,
            key_confidence: 0.0,
            active_intervals: [0.0; ROBOTER_HARMONY_COUNT],
            voices: [RoboterVoice::new(); ROBOTER_VOICE_COUNT],
            delay: [Vec::new(), Vec::new()],
            write_position: 0,
            latency: 2_768,
            bypassed: false,
        }
    }

    fn analyse_pitch(&mut self) {
        let frame = std::array::from_fn(|index| {
            self.pitch_ring[(self.pitch_position + index) % ROBOTER_YIN_FRAME]
        });
        let analysis_rate = self.sample_rate / self.downsample_factor as f32;
        let estimate = yin_pitch(&frame, analysis_rate);
        self.pitch_confidence = estimate.confidence;
        self.aperiodicity = estimate.aperiodicity;
        self.voiced = estimate.voiced;
        if estimate.voiced {
            self.detected_hz = estimate.frequency;
            self.raw_midi = 69.0 + 12.0 * (estimate.frequency / 440.0).log2();
            if !self.smoothed_midi.is_finite() || (self.raw_midi - self.smoothed_midi).abs() >= 4.0
            {
                self.smoothed_midi = self.raw_midi
            } else {
                self.smoothed_midi += (self.raw_midi - self.smoothed_midi) * 0.35
            }
        }
        self.update_chroma();
        self.update_pitch_targets();
    }

    fn update_chroma(&mut self) {
        if self.chroma_history.is_empty() {
            return;
        }
        let old = self.chroma_history[self.chroma_position];
        if old.pitch_class < 12 {
            self.chroma_histogram[old.pitch_class as usize] =
                (self.chroma_histogram[old.pitch_class as usize] - old.weight).max(0.0);
            self.chroma_weight = (self.chroma_weight - old.weight).max(0.0)
        }
        let event = if self.voiced {
            let pitch_class = self.smoothed_midi.round().rem_euclid(12.0) as u8;
            let weight = self.pitch_confidence * (1.0 - self.aperiodicity).sqrt();
            self.chroma_histogram[pitch_class as usize] += weight;
            self.chroma_weight += weight;
            ChromaEvent {
                pitch_class,
                weight,
            }
        } else {
            ChromaEvent::EMPTY
        };
        self.chroma_history[self.chroma_position] = event;
        self.chroma_position = increment_wrap(self.chroma_position, self.chroma_history.len());

        self.refresh_key_estimate()
    }

    fn refresh_key_estimate(&mut self) {
        let distinct = self
            .chroma_histogram
            .iter()
            .filter(|weight| **weight > self.chroma_weight * 0.025)
            .count();
        let (root, minor, best_score, second_score) = match_key(&self.chroma_histogram);
        let evidence = (self.chroma_weight / 20.0).clamp(0.0, 1.0)
            * ((distinct as f32 - 2.0) / 3.0).clamp(0.0, 1.0);
        self.key_confidence = ((best_score - second_score) * 3.5).clamp(0.0, 1.0) * evidence;
        if !self.key_valid {
            if self.key_confidence >= 0.18 {
                self.key_root = root;
                self.key_minor = minor;
                self.key_valid = true
            }
        } else if root != self.key_root || minor != self.key_minor {
            let current_score =
                key_profile_score(&self.chroma_histogram, self.key_root, self.key_minor);
            if self.key_confidence >= 0.24 && best_score > current_score + 0.08 {
                self.key_root = root;
                self.key_minor = minor
            }
        }
    }

    fn update_pitch_targets(&mut self) {
        if !self.voiced {
            for voice in &mut self.voices {
                voice.target_ratio = 1.0
            }
            return;
        }
        let tonal = self.key_valid && self.key_confidence >= 0.18;
        let (lead_target, degree) = if tonal {
            nearest_scale_note(self.smoothed_midi, self.key_root, self.key_minor)
        } else {
            (self.smoothed_midi.round() as i32, 0)
        };
        let depth = if self.amount <= 0.5 {
            self.amount * 2.0
        } else {
            1.0
        };
        let lead_shift = (lead_target as f32 - self.smoothed_midi) * depth;
        self.voices[0].target_ratio = 2.0_f32.powf(lead_shift / 12.0).clamp(0.9, 1.1);

        for harmony in 0..ROBOTER_HARMONY_COUNT {
            let target = if tonal {
                diatonic_harmony_note(lead_target, degree, harmony, self.key_root, self.key_minor)
            } else {
                lead_target + chromatic_harmony_interval(harmony)
            };
            self.active_intervals[harmony] = (target - lead_target) as f32;
            self.voices[harmony + 1].target_ratio = 2.0_f32
                .powf((target as f32 - self.smoothed_midi) / 12.0)
                .clamp(0.45, 2.1)
        }
    }

    fn configure_voices(&mut self) {
        for harmony in 0..ROBOTER_HARMONY_COUNT {
            let (active, pan) = roboter_voice_layout(self.number, harmony);
            self.voices[harmony + 1].target_gain = if active { 1.0 } else { 0.0 };
            self.voices[harmony + 1].target_pan = pan
        }
    }
}
impl DspEffect for Roboter {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.downsample_factor = (sample_rate / 24_000.0).round().clamp(1.0, 8.0) as usize;
        let analysis_latency = ROBOTER_YIN_FRAME * self.downsample_factor;
        let maximum_period = (sample_rate / 80.0).ceil() as usize * 2;
        let maximum_extra_delay = (sample_rate * 0.015).ceil() as usize;
        self.latency = analysis_latency + maximum_period + maximum_extra_delay;
        let delay_length = self.latency + maximum_period + maximum_extra_delay + MAX_BLOCK_SIZE + 8;
        self.delay = [vec![0.0; delay_length], vec![0.0; delay_length]];
        let history_length = ((sample_rate * 6.0) / ROBOTER_ANALYSIS_HOP as f32)
            .round()
            .max(1.0) as usize;
        self.chroma_history = vec![ChromaEvent::EMPTY; history_length];
        self.reset();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        let retime_seconds = if self.amount <= 0.5 {
            0.04
        } else {
            0.04 * (1.0 - (self.amount - 0.5) * 2.0).max(0.0)
        };
        let harmony_normalization = if self.number == 0 {
            0.0
        } else {
            1.0 / (self.number as f32).sqrt()
        };
        for frame in 0..frames {
            let input = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let mono = (input[0] + input[1]) * 0.5;
            self.delay[0][self.write_position] = input[0];
            self.delay[1][self.write_position] = input[1];

            self.downsample_sum += mono;
            self.downsample_count += 1;
            self.samples_since_analysis += 1;
            if self.downsample_count >= self.downsample_factor {
                self.pitch_ring[self.pitch_position] =
                    self.downsample_sum / self.downsample_count as f32;
                self.pitch_position = increment_wrap(self.pitch_position, ROBOTER_YIN_FRAME);
                self.pitch_filled = (self.pitch_filled + 1).min(ROBOTER_YIN_FRAME);
                self.downsample_sum = 0.0;
                self.downsample_count = 0
            }
            if self.samples_since_analysis >= ROBOTER_ANALYSIS_HOP
                && self.pitch_filled == ROBOTER_YIN_FRAME
            {
                self.samples_since_analysis = 0;
                self.analyse_pitch()
            }

            let delayed = std::array::from_fn(|channel| {
                fractional_read(
                    &self.delay[channel],
                    self.write_position as f32 - self.latency as f32,
                )
            });
            let period = if self.voiced {
                self.sample_rate / self.detected_hz.clamp(80.0, 1_000.0)
            } else {
                self.sample_rate / 220.0
            };
            let mut output = if self.voiced && self.amount > 0.0001 {
                self.voices[0].render(
                    &self.delay,
                    self.write_position,
                    self.latency,
                    period,
                    self.sample_rate,
                    0,
                    retime_seconds,
                )
            } else {
                delayed
            };

            for harmony in 0..ROBOTER_HARMONY_COUNT {
                let voice_index = harmony + 1;
                self.voices[voice_index].advance_mix(self.sample_rate);
                if !self.voiced || self.voices[voice_index].gain <= 0.000_01 {
                    continue;
                }
                let mut shifted = self.voices[voice_index].render(
                    &self.delay,
                    self.write_position,
                    self.latency,
                    period,
                    self.sample_rate,
                    voice_index,
                    0.004,
                );
                if harmony == 0 {
                    shifted = self.voices[voice_index].mono_bass(shifted, self.sample_rate)
                }
                let mut pan = self.voices[voice_index].pan;
                if harmony == 3 && self.detected_hz < 180.0 {
                    pan *= (self.detected_hz / 180.0).clamp(0.25, 1.0)
                }
                let left_pan = if pan > 0.0 { 1.0 - pan } else { 1.0 };
                let right_pan = if pan < 0.0 { 1.0 + pan } else { 1.0 };
                let gain = self.voices[voice_index].gain * harmony_normalization;
                output[0] += shifted[0] * left_pan * gain;
                output[1] += shifted[1] * right_pan * gain
            }
            buffer.channels[0][frame] = denormal(output[0]);
            buffer.channels[1][frame] = denormal(output[1]);
            self.write_position = increment_wrap(self.write_position, self.delay[0].len())
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "amount" | "depth" | "robot" => self.amount = value.clamp(0.0, 1.0),
            "number" | "voices" => {
                self.number = value.round().clamp(0.0, 5.0) as usize;
                self.configure_voices()
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.downsample_sum = 0.0;
        self.downsample_count = 0;
        self.samples_since_analysis = 0;
        self.pitch_ring = [0.0; ROBOTER_YIN_FRAME];
        self.pitch_position = 0;
        self.pitch_filled = 0;
        self.raw_midi = 69.0;
        self.smoothed_midi = 69.0;
        self.detected_hz = 440.0;
        self.pitch_confidence = 0.0;
        self.aperiodicity = 1.0;
        self.voiced = false;
        self.chroma_histogram = [0.0; 12];
        self.chroma_history.fill(ChromaEvent::EMPTY);
        self.chroma_position = 0;
        self.chroma_weight = 0.0;
        self.key_root = 0;
        self.key_minor = false;
        self.key_valid = false;
        self.key_confidence = 0.0;
        self.active_intervals = [0.0; ROBOTER_HARMONY_COUNT];
        self.voices = [RoboterVoice::new(); ROBOTER_VOICE_COUNT];
        self.voices[0].gain = 1.0;
        self.voices[0].target_gain = 1.0;
        self.configure_voices();
        for channel in &mut self.delay {
            channel.fill(0.0)
        }
        self.write_position = 0
    }
    fn tail_samples(&self) -> usize {
        (self.sample_rate * (0.015 + 0.05)).ceil() as usize
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        let mut metrics = [0.0; DISTORTION_SPECTRUM_BINS];
        metrics[0] = if self.voiced {
            self.smoothed_midi
        } else {
            -1.0
        };
        metrics[1] = self.key_root as f32;
        metrics[2] = if self.key_valid && self.key_confidence >= 0.18 {
            if self.key_minor {
                1.0
            } else {
                0.0
            }
        } else {
            2.0
        };
        metrics[3] = self.key_confidence;
        metrics[4] = if self.voiced { 1.0 } else { 0.0 };
        metrics[5] = self.pitch_confidence;
        metrics[6] = self.number as f32;
        metrics[7..(7 + ROBOTER_HARMONY_COUNT)].copy_from_slice(&self.active_intervals);
        Some(metrics)
    }
}

fn yin_pitch(frame: &[f32; ROBOTER_YIN_FRAME], sample_rate: f32) -> PitchEstimate {
    let minimum_lag = (sample_rate / 1_000.0).floor().max(2.0) as usize;
    let maximum_lag = ((sample_rate / 80.0).ceil() as usize)
        .min(ROBOTER_YIN_MAX_LAG)
        .min(ROBOTER_YIN_FRAME / 2 - 1);
    let comparison_samples = ROBOTER_YIN_FRAME - maximum_lag;
    let rms =
        (frame.iter().map(|sample| sample * sample).sum::<f32>() / ROBOTER_YIN_FRAME as f32).sqrt();
    if rms < 0.0015 || maximum_lag <= minimum_lag + 2 {
        return PitchEstimate::default();
    }
    let mut raw_difference = [0.0_f32; ROBOTER_YIN_MAX_LAG + 1];
    for lag in 1..=maximum_lag {
        let mut sum = 0.0;
        for index in 0..comparison_samples {
            let delta = frame[index] - frame[index + lag];
            sum += delta * delta
        }
        raw_difference[lag] = sum
    }
    let mut difference = raw_difference;
    let mut running_sum = 0.0;
    difference[0] = 1.0;
    for lag in 1..=maximum_lag {
        running_sum += difference[lag];
        difference[lag] = difference[lag] * lag as f32 / running_sum.max(1e-12)
    }
    let mut selected = 0;
    for lag in minimum_lag..maximum_lag {
        if difference[lag] < 0.15 && difference[lag] <= difference[lag + 1] {
            selected = lag;
            break;
        }
    }
    if selected == 0 {
        selected = (minimum_lag..=maximum_lag)
            .min_by(|left, right| difference[*left].total_cmp(&difference[*right]))
            .unwrap_or(0)
    }
    if selected == 0 {
        return PitchEstimate::default();
    }
    let aperiodicity = difference[selected].clamp(0.0, 1.0);
    let mut lower = (selected as f32 - 1.0).max(minimum_lag as f32);
    let mut upper = (selected as f32 + 1.0).min(maximum_lag as f32);
    const GOLDEN: f32 = 0.618_034;
    let mut left = upper - (upper - lower) * GOLDEN;
    let mut right = lower + (upper - lower) * GOLDEN;
    let mut left_value = yin_fractional_difference(frame, left, comparison_samples);
    let mut right_value = yin_fractional_difference(frame, right, comparison_samples);
    for _ in 0..12 {
        if left_value <= right_value {
            upper = right;
            right = left;
            right_value = left_value;
            left = upper - (upper - lower) * GOLDEN;
            left_value = yin_fractional_difference(frame, left, comparison_samples)
        } else {
            lower = left;
            left = right;
            left_value = right_value;
            right = lower + (upper - lower) * GOLDEN;
            right_value = yin_fractional_difference(frame, right, comparison_samples)
        }
    }
    let lag = (lower + upper) * 0.5;
    let frequency = sample_rate / lag.max(1.0);
    let confidence = (1.0 - aperiodicity).clamp(0.0, 1.0);
    PitchEstimate {
        frequency,
        confidence,
        aperiodicity,
        voiced: (80.0..=1_000.0).contains(&frequency) && confidence >= 0.68,
    }
}

fn yin_fractional_difference(frame: &[f32; ROBOTER_YIN_FRAME], lag: f32, samples: usize) -> f32 {
    let mut sum = 0.0;
    let safe_end = samples
        .saturating_sub(2)
        .min(ROBOTER_YIN_FRAME.saturating_sub(lag.ceil() as usize + 2));
    for index in 1..safe_end {
        let position = index as f32 + lag;
        let base = position.floor() as usize;
        let fraction = position - base as f32;
        let shifted = frame[base] + (frame[base + 1] - frame[base]) * fraction;
        let delta = frame[index] - shifted;
        sum += delta * delta
    }
    sum
}

fn key_profile_score(histogram: &[f32; 12], root: usize, minor: bool) -> f32 {
    let profile = if minor {
        &MINOR_PROFILE
    } else {
        &MAJOR_PROFILE
    };
    let histogram_mean = histogram.iter().sum::<f32>() / 12.0;
    let profile_mean = profile.iter().sum::<f32>() / 12.0;
    let mut numerator = 0.0;
    let mut histogram_energy = 0.0;
    let mut profile_energy = 0.0;
    for pitch_class in 0..12 {
        let observed = histogram[(root + pitch_class) % 12] - histogram_mean;
        let expected = profile[pitch_class] - profile_mean;
        numerator += observed * expected;
        histogram_energy += observed * observed;
        profile_energy += expected * expected
    }
    numerator / (histogram_energy * profile_energy).sqrt().max(1e-9)
}

fn match_key(histogram: &[f32; 12]) -> (usize, bool, f32, f32) {
    let mut best = (0, false, f32::NEG_INFINITY);
    let mut second = f32::NEG_INFINITY;
    for minor in [false, true] {
        for root in 0..12 {
            let score = key_profile_score(histogram, root, minor);
            if score > best.2 {
                second = best.2;
                best = (root, minor, score)
            } else if score > second {
                second = score
            }
        }
    }
    (best.0, best.1, best.2, second)
}

fn nearest_scale_note(midi: f32, root: usize, minor: bool) -> (i32, usize) {
    let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    let center = midi.round() as i32;
    let mut best_note = center;
    let mut best_degree = 0;
    let mut best_distance = f32::MAX;
    for note in (center - 7)..=(center + 7) {
        let relative = (note - root as i32).rem_euclid(12);
        if let Some(degree) = scale.iter().position(|pitch| *pitch == relative) {
            let distance = (note as f32 - midi).abs();
            if distance < best_distance {
                best_note = note;
                best_degree = degree;
                best_distance = distance
            }
        }
    }
    (best_note, best_degree)
}

fn diatonic_harmony_note(
    lead_note: i32,
    lead_degree: usize,
    harmony: usize,
    root: usize,
    minor: bool,
) -> i32 {
    if harmony == 0 {
        return lead_note - 12;
    }
    let mut steps = match harmony {
        1 => -2,
        2 => 2,
        3 => -5,
        _ => 4,
    };
    if !minor && lead_degree == 6 && harmony == 4 {
        steps = 5
    }
    let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    let tonic = lead_note - scale[lead_degree] - root as i32 + root as i32;
    let target_degree = lead_degree as i32 + steps;
    let octave = target_degree.div_euclid(7);
    let degree = target_degree.rem_euclid(7) as usize;
    tonic + octave * 12 + scale[degree]
}

fn chromatic_harmony_interval(harmony: usize) -> i32 {
    [-12, -4, 4, -9, 7][harmony.min(ROBOTER_HARMONY_COUNT - 1)]
}

fn roboter_voice_layout(number: usize, harmony: usize) -> (bool, f32) {
    const ACTIVE: [[bool; ROBOTER_HARMONY_COUNT]; 6] = [
        [false, false, false, false, false],
        [true, false, false, false, false],
        [false, true, true, false, false],
        [true, true, true, false, false],
        [false, true, true, true, true],
        [true, true, true, true, true],
    ];
    const PAN: [[f32; ROBOTER_HARMONY_COUNT]; 6] = [
        [0.0; 5],
        [0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, -0.60, 0.60, 0.0, 0.0],
        [0.0, -0.75, 0.75, 0.0, 0.0],
        [0.0, -0.40, 0.40, -0.85, 0.85],
        [0.0, -0.45, 0.45, -0.90, 0.90],
    ];
    let number = number.min(5);
    (ACTIVE[number][harmony], PAN[number][harmony])
}

const COLORIZER_FFT_SIZE: usize = 4096;
const COLORIZER_HOP_SIZE: usize = COLORIZER_FFT_SIZE / 4;
const COLORIZER_BINS: usize = COLORIZER_FFT_SIZE / 2 + 1;
const COLORIZER_MASK_SMOOTH_FRAMES: usize = 4;
const MAX_ACTIVE_PITCHES: usize = 12;
const COLORIZER_METER_BINS: usize = DISTORTION_SPECTRUM_BINS / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum ChordSource {
    AutoDetect,
    Manual,
    MidiInput,
}

/// Fixed-capacity control-rate pitch snapshot. It is deliberately independent
/// from both UI presets and MIDI routing so spectral DSP only sees pitch data.
#[derive(Clone, Copy, Debug)]
struct ActivePitches {
    pitches: [u8; MAX_ACTIVE_PITCHES],
    count: usize,
    confidence: f32,
    changed: bool,
}
impl ActivePitches {
    fn from_mask(mask: u16, previous: u16) -> Self {
        let mut pitches = [0; MAX_ACTIVE_PITCHES];
        let mut count = 0;
        for pitch in 0..12 {
            if mask & (1 << pitch) != 0 {
                pitches[count] = pitch as u8;
                count += 1;
            }
        }
        Self {
            pitches,
            count,
            confidence: 1.0,
            changed: mask != previous,
        }
    }
    fn mask(&self) -> u16 {
        self.pitches[..self.count]
            .iter()
            .fold(0_u16, |mask, pitch| mask | (1 << pitch))
    }
}

struct ColorizerChannel {
    input_ring: Vec<f32>,
    dry_delay: Vec<f32>,
    ola_ring: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    held: Vec<f32>,
    phase: Vec<f32>,
    previous_magnitude: Vec<f32>,
}
impl ColorizerChannel {
    fn new(scratch_len: usize) -> Self {
        Self {
            input_ring: vec![0.0; COLORIZER_FFT_SIZE],
            dry_delay: vec![0.0; COLORIZER_FFT_SIZE],
            ola_ring: vec![0.0; COLORIZER_FFT_SIZE],
            spectrum: vec![Complex32::new(0.0, 0.0); COLORIZER_FFT_SIZE],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            held: vec![0.0; COLORIZER_BINS],
            phase: vec![0.0; COLORIZER_BINS],
            previous_magnitude: vec![0.0; COLORIZER_BINS],
        }
    }
    fn clear(&mut self) {
        self.input_ring.fill(0.0);
        self.dry_delay.fill(0.0);
        self.ola_ring.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
        self.scratch.fill(Complex32::new(0.0, 0.0));
        self.held.fill(0.0);
        self.phase.fill(0.0);
        self.previous_magnitude.fill(0.0);
    }
}

/// Harmonic STFT resonator shown to users as `Colorizer`.
///
/// FFT plans, scratch, rings, masks, and phase state are all prepared before
/// processing. `process` performs no heap allocation or locking.
struct Colorizer {
    sample_rate: f32,
    bypassed: bool,
    source: ChordSource,
    manual_mask: u16,
    midi_mask: u16,
    applied_mask: u16,
    midi_notes: [u8; 128],
    resonance: f32,
    decay: f32,
    depth: f32,
    mix: Smoother,
    mask_current: Vec<f32>,
    mask_start: Vec<f32>,
    mask_target: Vec<f32>,
    mask_dirty: bool,
    mask_smooth_remaining: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    channels: [ColorizerChannel; MAX_CHANNELS],
    ring_position: usize,
    samples_seen: usize,
    samples_until_frame: usize,
    meter: [f32; DISTORTION_SPECTRUM_BINS],
}
impl Colorizer {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(COLORIZER_FFT_SIZE);
        let inverse = planner.plan_fft_inverse(COLORIZER_FFT_SIZE);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Self {
            sample_rate: 48_000.0,
            bypassed: false,
            source: ChordSource::Manual,
            manual_mask: 0b1010_1101_0101,
            midi_mask: 0,
            applied_mask: 0,
            midi_notes: [0; 128],
            resonance: 0.62,
            decay: 0.45,
            depth: 0.82,
            mix: Smoother::new(0.72, 48_000.0, 0.015),
            mask_current: vec![1.0; COLORIZER_BINS],
            mask_start: vec![1.0; COLORIZER_BINS],
            mask_target: vec![1.0; COLORIZER_BINS],
            mask_dirty: true,
            mask_smooth_remaining: 0,
            window: vec![0.0; COLORIZER_FFT_SIZE],
            forward,
            inverse,
            channels: [
                ColorizerChannel::new(scratch_len),
                ColorizerChannel::new(scratch_len),
            ],
            ring_position: 0,
            samples_seen: 0,
            samples_until_frame: COLORIZER_FFT_SIZE,
            meter: [0.0; DISTORTION_SPECTRUM_BINS],
        }
    }
    fn active_mask(&self) -> u16 {
        match self.source {
            ChordSource::MidiInput => self.midi_mask,
            ChordSource::Manual | ChordSource::AutoDetect => self.manual_mask,
        }
    }
    fn update_active_pitches(&mut self) {
        let pitches = ActivePitches::from_mask(self.active_mask(), self.applied_mask);
        let _confidence = pitches.confidence;
        if pitches.changed {
            self.applied_mask = pitches.mask();
            self.mask_dirty = true;
        }
    }
    fn rebuild_mask(&mut self) {
        if !self.mask_dirty {
            return;
        }
        self.mask_start.copy_from_slice(&self.mask_current);
        let pitch_mask = self.active_mask();
        let midi_closed = self.source == ChordSource::MidiInput && pitch_mask == 0;
        let sigma = 1.55 * (1.0 - self.resonance).powi(2) + 0.075;
        let bin_width = self.sample_rate / COLORIZER_FFT_SIZE as f32;
        for (bin, gain) in self.mask_target.iter_mut().enumerate() {
            let frequency = bin as f32 * self.sample_rate / COLORIZER_FFT_SIZE as f32;
            if midi_closed {
                *gain = 0.0;
                continue;
            }
            let harmonic_mask = if (40.0..=12_000.0).contains(&frequency) && pitch_mask != 0 {
                let midi_pitch = 69.0 + 12.0 * (frequency / 440.0).log2();
                let pitch_class = midi_pitch.rem_euclid(12.0);
                // A bell narrower than one FFT bin can miss a low note
                // completely. Widen only as much as the local bin resolution
                // requires, preserving the requested logarithmic Q elsewhere.
                let resolution_sigma = if frequency > bin_width * 0.55 {
                    12.0 * ((frequency + bin_width * 0.5) / (frequency - bin_width * 0.5)).log2()
                        * 0.55
                } else {
                    3.0
                };
                let effective_sigma = sigma.max(resolution_sigma);
                let mut sum = 0.0_f32;
                for pitch in 0..12 {
                    if pitch_mask & (1 << pitch) == 0 {
                        continue;
                    }
                    let direct = (pitch_class - pitch as f32).abs();
                    let distance = direct.min(12.0 - direct);
                    sum += (-0.5 * (distance / effective_sigma).powi(2)).exp();
                }
                sum.min(1.0)
            } else {
                0.0
            };
            *gain = harmonic_mask + (1.0 - harmonic_mask) * (1.0 - self.depth);
        }
        self.mask_smooth_remaining = COLORIZER_MASK_SMOOTH_FRAMES;
        self.mask_dirty = false;
    }
    fn advance_mask(&mut self) {
        self.rebuild_mask();
        if self.mask_smooth_remaining == 0 {
            return;
        }
        let completed = COLORIZER_MASK_SMOOTH_FRAMES - self.mask_smooth_remaining + 1;
        let amount = completed as f32 / COLORIZER_MASK_SMOOTH_FRAMES as f32;
        for bin in 0..COLORIZER_BINS {
            self.mask_current[bin] =
                self.mask_start[bin] + (self.mask_target[bin] - self.mask_start[bin]) * amount;
        }
        self.mask_smooth_remaining -= 1;
    }
    fn settle_mask_before_audio(&mut self) {
        if self.samples_seen != 0 {
            return;
        }
        self.rebuild_mask();
        self.mask_current.copy_from_slice(&self.mask_target);
        self.mask_smooth_remaining = 0;
    }
    fn decay_coefficient(&self) -> f32 {
        if self.decay <= 1e-5 {
            return 0.0;
        }
        let seconds = 0.035 * (240.0_f32).powf(self.decay);
        (-(COLORIZER_HOP_SIZE as f32) / (seconds * self.sample_rate))
            .exp()
            .min(0.999_95)
    }
    fn handle_event(&mut self, event: NoteEventKind) {
        match event {
            NoteEventKind::NoteOn {
                pitch, velocity, ..
            } if velocity > 0.0 => {
                self.midi_notes[pitch as usize] = self.midi_notes[pitch as usize].saturating_add(1)
            }
            NoteEventKind::NoteOn { pitch, .. } | NoteEventKind::NoteOff { pitch, .. } => {
                self.midi_notes[pitch as usize] = self.midi_notes[pitch as usize].saturating_sub(1)
            }
            NoteEventKind::AllNotesOff => self.midi_notes.fill(0),
            _ => return,
        }
        let mut mask = 0_u16;
        for (pitch, count) in self.midi_notes.iter().enumerate() {
            if *count > 0 {
                mask |= 1 << (pitch % 12)
            }
        }
        if mask != self.midi_mask {
            self.midi_mask = mask;
            self.update_active_pitches();
        }
    }
    fn render_frame(&mut self) {
        self.advance_mask();
        let decay = self.decay_coefficient();
        self.meter[..COLORIZER_METER_BINS].fill(0.0);
        for channel in &mut self.channels {
            process_colorizer_frame(
                channel,
                &self.forward,
                &self.inverse,
                &self.window,
                &self.mask_current,
                self.sample_rate,
                self.ring_position,
                decay,
                &mut self.meter[..COLORIZER_METER_BINS],
            );
        }
        for value in &mut self.meter[..COLORIZER_METER_BINS] {
            *value *= 0.5;
        }
        for index in 0..COLORIZER_METER_BINS {
            let frequency = colorizer_meter_frequency(index);
            let bin = ((frequency * COLORIZER_FFT_SIZE as f32 / self.sample_rate).round() as usize)
                .min(COLORIZER_BINS - 1);
            self.meter[COLORIZER_METER_BINS + index] = self.mask_current[bin];
        }
    }
}
impl DspEffect for Colorizer {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let mut planner = FftPlanner::<f32>::new();
        self.forward = planner.plan_fft_forward(COLORIZER_FFT_SIZE);
        self.inverse = planner.plan_fft_inverse(COLORIZER_FFT_SIZE);
        let scratch_len = self
            .forward
            .get_inplace_scratch_len()
            .max(self.inverse.get_inplace_scratch_len());
        self.channels = [
            ColorizerChannel::new(scratch_len),
            ColorizerChannel::new(scratch_len),
        ];
        for (index, value) in self.window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * PI * index as f32 / COLORIZER_FFT_SIZE as f32).cos();
        }
        self.mix = Smoother::new(self.mix.target, self.sample_rate, 0.015);
        self.reset();
        self.mask_dirty = true;
        self.rebuild_mask();
        self.mask_current.copy_from_slice(&self.mask_target);
        self.mask_smooth_remaining = 0;
        self.applied_mask = self.active_mask();
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        debug_assert!(events
            .windows(2)
            .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
        let mut event_index = 0;
        for frame in 0..frames.min(MAX_BLOCK_SIZE) {
            if self.source == ChordSource::MidiInput {
                while event_index < events.len()
                    && events[event_index].sample_offset as usize == frame
                {
                    self.handle_event(events[event_index].kind);
                    event_index += 1;
                }
            }
            let mix = self.mix.next();
            for channel in 0..MAX_CHANNELS {
                let input = buffer.channels[channel][frame];
                let state = &mut self.channels[channel];
                state.input_ring[self.ring_position] = input;
                let dry = state.dry_delay[self.ring_position];
                state.dry_delay[self.ring_position] = input;
                let wet = state.ola_ring[self.ring_position];
                state.ola_ring[self.ring_position] = 0.0;
                buffer.channels[channel][frame] = denormal(dry + (wet - dry) * mix);
            }
            self.ring_position = increment_wrap(self.ring_position, COLORIZER_FFT_SIZE);
            self.samples_seen = self.samples_seen.saturating_add(1);
            self.samples_until_frame -= 1;
            if self.samples_until_frame == 0 {
                self.render_frame();
                self.samples_until_frame = COLORIZER_HOP_SIZE;
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "midi" => {
                let source = if value >= 0.5 {
                    ChordSource::MidiInput
                } else {
                    ChordSource::Manual
                };
                if source != self.source {
                    self.source = source;
                    self.applied_mask = self.active_mask();
                    self.mask_dirty = true;
                    self.settle_mask_before_audio();
                }
            }
            "resonance" => {
                self.resonance = value.clamp(0.0, 1.0);
                self.mask_dirty = true;
                self.settle_mask_before_audio();
            }
            "decay" => self.decay = value.clamp(0.0, 1.0),
            "depth" => {
                self.depth = value.clamp(0.0, 1.0);
                self.mask_dirty = true;
                self.settle_mask_before_audio();
            }
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            _ if id.starts_with("pitch") => {
                if let Ok(pitch) = id[5..].parse::<usize>() {
                    if pitch < 12 {
                        if value >= 0.5 {
                            self.manual_mask |= 1 << pitch
                        } else {
                            self.manual_mask &= !(1 << pitch)
                        }
                        self.update_active_pitches();
                        self.settle_mask_before_audio();
                    }
                }
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.clear();
        }
        self.midi_notes.fill(0);
        self.midi_mask = 0;
        self.applied_mask = self.active_mask();
        self.mask_dirty = true;
        self.ring_position = 0;
        self.samples_seen = 0;
        self.samples_until_frame = COLORIZER_FFT_SIZE;
        self.meter.fill(0.0);
    }
    fn tail_samples(&self) -> usize {
        if self.decay <= 1e-5 {
            COLORIZER_FFT_SIZE
        } else {
            let seconds = 0.035 * (240.0_f32).powf(self.decay);
            COLORIZER_FFT_SIZE + (seconds * 9.21 * self.sample_rate) as usize
        }
    }
    fn latency_samples(&self) -> usize {
        COLORIZER_FFT_SIZE
    }
    fn wants_midi(&self) -> bool {
        self.source == ChordSource::MidiInput
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        Some(self.meter)
    }
}

#[allow(clippy::too_many_arguments)]
fn process_colorizer_frame(
    channel: &mut ColorizerChannel,
    forward: &Arc<dyn Fft<f32>>,
    inverse: &Arc<dyn Fft<f32>>,
    window: &[f32],
    mask: &[f32],
    sample_rate: f32,
    ring_position: usize,
    decay: f32,
    meter: &mut [f32],
) {
    for index in 0..COLORIZER_FFT_SIZE {
        let input_index = (ring_position + index) % COLORIZER_FFT_SIZE;
        channel.spectrum[index] =
            Complex32::new(channel.input_ring[input_index] * window[index], 0.0);
    }
    forward.process_with_scratch(&mut channel.spectrum, &mut channel.scratch);
    let mut flux = 0.0_f32;
    let mut energy = 1e-12_f32;
    for bin in 0..COLORIZER_BINS {
        let magnitude = channel.spectrum[bin].norm();
        flux += (magnitude - channel.previous_magnitude[bin]).max(0.0);
        energy += magnitude;
        channel.previous_magnitude[bin] = magnitude;
    }
    let frame_decay = if flux / energy > 0.18 {
        decay * 0.35
    } else {
        decay
    };
    for bin in 0..COLORIZER_BINS {
        let input = channel.spectrum[bin];
        let input_magnitude = input.norm();
        let filtered = input_magnitude * mask[bin];
        let previous = channel.held[bin];
        let held = if frame_decay <= 1e-8 {
            filtered
        } else {
            filtered.max(previous * frame_decay)
        };
        channel.held[bin] = denormal(held);
        if held <= 1e-20 {
            channel.spectrum[bin] = Complex32::new(0.0, 0.0);
            continue;
        }
        let input_phase = input.arg();
        let expected =
            2.0 * PI * bin as f32 * COLORIZER_HOP_SIZE as f32 / COLORIZER_FFT_SIZE as f32;
        let propagated = wrap_phase(channel.phase[bin] + expected);
        let fresh = (filtered / (held + 1e-20)).clamp(0.0, 1.0);
        let phase_vector = Complex32::from_polar(fresh, input_phase)
            + Complex32::from_polar(1.0 - fresh, propagated);
        let phase = if phase_vector.norm_sqr() > 1e-12 {
            phase_vector.arg()
        } else {
            propagated
        };
        channel.phase[bin] = wrap_phase(phase);
        channel.spectrum[bin] = Complex32::from_polar(held, phase);
        if bin > 0 && bin < COLORIZER_FFT_SIZE / 2 {
            channel.spectrum[COLORIZER_FFT_SIZE - bin] = channel.spectrum[bin].conj();
        }
    }
    inverse.process_with_scratch(&mut channel.spectrum, &mut channel.scratch);
    let normalization = 1.0 / (COLORIZER_FFT_SIZE as f32 * 1.5);
    for index in 0..COLORIZER_FFT_SIZE {
        let output_index = (ring_position + index) % COLORIZER_FFT_SIZE;
        channel.ola_ring[output_index] +=
            channel.spectrum[index].re * window[index] * normalization;
    }
    for (index, value) in meter.iter_mut().enumerate() {
        let frequency = colorizer_meter_frequency(index);
        let bin = ((frequency * COLORIZER_FFT_SIZE as f32 / sample_rate).round() as usize)
            .min(COLORIZER_BINS - 1);
        let amplitude = channel.held[bin] * (2.0 / COLORIZER_FFT_SIZE as f32);
        *value += amplitude.max(0.0).sqrt().min(1.0);
    }
}

#[inline(always)]
fn colorizer_meter_frequency(index: usize) -> f32 {
    40.0 * (12_000.0_f32 / 40.0).powf(index as f32 / (COLORIZER_METER_BINS - 1) as f32)
}

#[inline(always)]
fn wrap_phase(phase: f32) -> f32 {
    (phase + PI).rem_euclid(2.0 * PI) - PI
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
        "builtin:mastering-limiter" => Box::new(MasteringLimiter::new()),
        "builtin:vocoder" => Box::new(Vocoder::new()),
        "builtin:lfo-tremolo" => Box::new(LfoTremolo::new()),
        "builtin:clipper" => Box::new(Clipper::new()),
        "builtin:upward-compressor" => Box::new(UpwardCompressor::new()),
        "builtin:roboter" => Box::new(Roboter::new()),
        "builtin:resonator" => Box::new(Colorizer::new()),
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
#[inline(always)]
fn amplitude_to_db(value: f32) -> f32 {
    20.0 * value.max(1e-6).log10()
}
#[inline(always)]
fn power_to_lufs(power: f64) -> f32 {
    if power <= 1e-12 {
        -120.0
    } else {
        (-0.691 + 10.0 * power.log10()) as f32
    }
}
fn integrated_lufs(blocks: &[f32], count: usize) -> f32 {
    let absolute_energy = 10.0_f32.powf((-70.0 + 0.691) / 10.0);
    let mut absolute_sum = 0.0_f64;
    let mut absolute_count = 0_usize;
    for &energy in blocks.iter().take(count) {
        if energy >= absolute_energy {
            absolute_sum += f64::from(energy);
            absolute_count += 1;
        }
    }
    if absolute_count == 0 {
        return -120.0;
    }
    let ungated = absolute_sum / absolute_count as f64;
    let relative_energy = (ungated * 0.1) as f32;
    let mut gated_sum = 0.0_f64;
    let mut gated_count = 0_usize;
    for &energy in blocks.iter().take(count) {
        if energy >= absolute_energy.max(relative_energy) {
            gated_sum += f64::from(energy);
            gated_count += 1;
        }
    }
    if gated_count == 0 {
        -120.0
    } else {
        power_to_lufs(gated_sum / gated_count as f64)
    }
}
#[inline(always)]
fn interpolated_peak(history: [f32; 4], factor: usize) -> f32 {
    let mut peak = history[1].abs().max(history[2].abs());
    for phase in 1..factor {
        peak = peak.max(
            catmull(
                history[0],
                history[1],
                history[2],
                history[3],
                phase as f32 / factor as f32,
            )
            .abs(),
        )
    }
    peak
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
fn clipper_curve(input: f32, knee: f32) -> f32 {
    let knee = knee.clamp(0.0, 1.0);
    if knee <= 1e-5 {
        return input.clamp(-1.0, 1.0);
    }
    let sign = input.signum();
    let magnitude = input.abs();
    let start = 1.0 - knee;
    if magnitude <= start {
        input
    } else {
        sign * (start + knee * ((magnitude - start) / knee).tanh())
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
        effect.process(&[], &mut buffer, 1);
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
        effect.process_with_sidechain(&[], &mut buffer, Some(&detector), MAX_BLOCK_SIZE);
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
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
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
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
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
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        assert!(buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            .all(|sample| sample.is_finite()));

        effect.set_param("amount", 0.0);
        let mut dry = AudioBuffer::new();
        dry.channels[0][0] = 0.25;
        dry.channels[1][0] = -0.25;
        effect.process(&[], &mut dry, 1);
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
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
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
    #[test]
    fn clean_limiter_respects_sample_ceiling_and_reports_loudness() {
        let mut limiter = MasteringLimiter::new();
        limiter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        limiter.set_param("outputDb", -1.0);
        let ceiling = db_to_gain(-1.0) + 1e-5;
        let mut saw_audio = false;
        let mut reconstruction = [[0.0_f32; 4]; MAX_CHANNELS];
        for block in 0..220 {
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let sample_index = block * MAX_BLOCK_SIZE + frame;
                let sample = (2.0 * PI * 997.0 * sample_index as f32 / 48_000.0).sin() * 1.8;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            limiter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for frame in 0..MAX_BLOCK_SIZE {
                for channel in 0..MAX_CHANNELS {
                    let sample = buffer.channels[channel][frame];
                    assert!(sample.is_finite());
                    assert!(sample.abs() <= ceiling);
                    reconstruction[channel].rotate_left(1);
                    reconstruction[channel][3] = sample;
                    let reconstructed = interpolated_peak(reconstruction[channel], 4);
                    assert!(
                        reconstructed <= ceiling + 0.001,
                        "true peak {reconstructed} exceeded ceiling {ceiling}"
                    );
                    saw_audio |= sample.abs() > 0.1;
                }
            }
        }
        assert!(saw_audio);
        let metrics = limiter.limiter_metrics().expect("limiter meters");
        assert!(metrics.iter().all(|value| value.is_finite()));
        assert!(metrics[2] > 0.0);
        assert!(metrics[4] > -120.0)
    }
    #[test]
    fn vocoder_uses_external_modulator_and_stays_finite() {
        let mut vocoder = Vocoder::new();
        vocoder.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        vocoder.set_param("source", 0.0);
        let mut energy = 0.0_f32;
        for block in 0..24 {
            let mut carrier = AudioBuffer::new();
            let mut modulator = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let index = block * MAX_BLOCK_SIZE + frame;
                let c = (2.0 * PI * 220.0 * index as f32 / 48_000.0).sin() * 0.7;
                let m = (2.0 * PI * 440.0 * index as f32 / 48_000.0).sin() * 0.8;
                carrier.channels[0][frame] = c;
                carrier.channels[1][frame] = c;
                modulator.channels[0][frame] = m;
                modulator.channels[1][frame] = m;
            }
            vocoder.process_with_sidechain(&[], &mut carrier, Some(&modulator), MAX_BLOCK_SIZE);
            for sample in carrier
                .channels
                .iter()
                .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            {
                assert!(sample.is_finite());
                energy += sample * sample;
            }
        }
        assert!(energy > 1e-6)
    }
    #[test]
    fn tremolo_modulates_volume_and_pan_without_non_finite_samples() {
        let mut tremolo = LfoTremolo::new();
        tremolo.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        tremolo.set_param("volumeDepth", 1.0);
        tremolo.set_param("panDepth", 1.0);
        tremolo.set_param("rateHz", 10.0);
        let mut minimum = f32::MAX;
        let mut maximum = f32::MIN;
        for _ in 0..32 {
            let mut buffer = AudioBuffer::new();
            buffer.channels[0][..MAX_BLOCK_SIZE].fill(0.5);
            buffer.channels[1][..MAX_BLOCK_SIZE].fill(0.5);
            tremolo.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for sample in buffer
                .channels
                .iter()
                .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            {
                assert!(sample.is_finite());
                minimum = minimum.min(*sample);
                maximum = maximum.max(*sample);
            }
        }
        assert!(maximum - minimum > 0.2)
    }
    #[test]
    fn clipper_limits_peaks_and_publishes_realtime_flow() {
        let mut clipper = Clipper::new();
        clipper.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        clipper.set_param("inputDb", 0.0);
        clipper.set_param("outputDb", -1.0);
        let mut maximum = 0.0_f32;
        for block in 0..8 {
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let index = block * MAX_BLOCK_SIZE + frame;
                let sample = (2.0 * PI * 997.0 * index as f32 / 48_000.0).sin() * 2.0;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            clipper.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for sample in buffer
                .channels
                .iter()
                .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            {
                assert!(sample.is_finite());
                maximum = maximum.max(sample.abs())
            }
        }
        assert!(maximum <= 1.0);
        let flow = clipper.effect_spectrum().expect("clipper peak flow");
        assert!(flow.iter().all(|value| value.is_finite()));
        assert!(flow.iter().any(|value| *value > 1.0));
        assert_eq!(clipper_curve(4.0, 0.0), 1.0)
    }
    #[test]
    fn upward_compressor_lifts_quiet_material_without_non_finite_samples() {
        let mut compressor = UpwardCompressor::new();
        compressor.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        compressor.set_param("threshold", -24.0);
        compressor.set_param("rangeDb", 18.0);
        compressor.set_param("attackMs", 1.0);
        let mut input_energy = 0.0_f32;
        let mut output_energy = 0.0_f32;
        for block in 0..40 {
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let index = block * MAX_BLOCK_SIZE + frame;
                let sample = (2.0 * PI * 220.0 * index as f32 / 48_000.0).sin() * 0.01;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
                if block > 20 {
                    input_energy += sample * sample * 2.0
                }
            }
            compressor.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            if block > 20 {
                for sample in buffer
                    .channels
                    .iter()
                    .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
                {
                    assert!(sample.is_finite());
                    output_energy += sample * sample
                }
            }
        }
        assert!(output_energy > input_energy * 2.0)
    }
    #[test]
    fn roboter_yin_detects_sine_and_saw_within_one_cent() {
        let sample_rate = 24_000.0;
        for frequency in [82.41_f32, 440.0, 987.77] {
            for saw in [false, true] {
                let frame = std::array::from_fn(|index| {
                    let phase = frequency * index as f32 / sample_rate;
                    if saw {
                        (phase.fract() * 2.0 - 1.0) * 0.35
                    } else {
                        (2.0 * PI * phase).sin() * 0.35
                    }
                });
                let estimate = yin_pitch(&frame, sample_rate);
                let error_cents = 1200.0 * (estimate.frequency / frequency).log2();
                assert!(
                    estimate.voiced,
                    "frequency={frequency} saw={saw} confidence={}",
                    estimate.confidence
                );
                let tolerance = if (frequency - 440.0).abs() < 0.01 {
                    1.0
                } else {
                    3.0
                };
                assert!(
                    error_cents.abs() <= tolerance,
                    "frequency={frequency} saw={saw} pitch={} error={error_cents} cents",
                    estimate.frequency
                )
            }
        }
    }
    #[test]
    fn roboter_key_profiles_identify_transposed_major_and_minor() {
        for (root, minor) in [(0, false), (7, false), (9, true), (3, true)] {
            let profile = if minor { MINOR_PROFILE } else { MAJOR_PROFILE };
            let mut histogram = [0.0_f32; 12];
            for pitch_class in 0..12 {
                histogram[(root + pitch_class) % 12] = profile[pitch_class]
            }
            let (detected_root, detected_minor, best, second) = match_key(&histogram);
            assert_eq!((detected_root, detected_minor), (root, minor));
            assert!(best > second)
        }
    }
    #[test]
    fn roboter_key_hysteresis_rejects_marginal_mid_phrase_change() {
        let mut roboter = Roboter::new();
        roboter.key_valid = true;
        roboter.key_root = 0;
        roboter.key_minor = false;
        roboter.chroma_weight = 100.0;
        let mut marginal = None;
        for step in 50..100 {
            let blend = step as f32 / 100.0;
            let histogram = std::array::from_fn(|pitch_class| {
                MAJOR_PROFILE[pitch_class] * (1.0 - blend)
                    + MAJOR_PROFILE[(pitch_class + 12 - 7) % 12] * blend
            });
            let (root, minor, best, _) = match_key(&histogram);
            let current = key_profile_score(&histogram, 0, false);
            if root == 7 && !minor && best <= current + 0.08 {
                marginal = Some(histogram);
                break;
            }
        }
        let histogram = marginal.expect("a marginal C-to-G candidate");
        roboter.chroma_histogram = histogram;
        roboter.refresh_key_estimate();
        assert_eq!((roboter.key_root, roboter.key_minor), (0, false));

        roboter.chroma_histogram =
            std::array::from_fn(|pitch_class| MAJOR_PROFILE[(pitch_class + 12 - 7) % 12]);
        roboter.refresh_key_estimate();
        assert_eq!((roboter.key_root, roboter.key_minor), (7, false))
    }
    #[test]
    fn roboter_diatonic_intervals_follow_scale_and_leading_tone_exception() {
        for root in 0..12 {
            for minor in [false, true] {
                let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
                let tonic = 60 + root as i32;
                for degree in 0..7 {
                    let lead = tonic + scale[degree];
                    let down_third = diatonic_harmony_note(lead, degree, 1, root, minor);
                    let up_third = diatonic_harmony_note(lead, degree, 2, root, minor);
                    assert!([3, 4].contains(&(lead - down_third)));
                    assert!([3, 4].contains(&(up_third - lead)));
                    for harmony in 1..ROBOTER_HARMONY_COUNT {
                        let note = diatonic_harmony_note(lead, degree, harmony, root, minor);
                        assert!(scale.contains(&((note - root as i32).rem_euclid(12))))
                    }
                }
                if !minor {
                    let leading = tonic + MAJOR_SCALE[6];
                    let upper = diatonic_harmony_note(leading, 6, 4, root, false);
                    assert_eq!(upper - leading, 8)
                }
            }
        }
    }
    #[test]
    fn roboter_reports_and_realizes_common_dry_latency() {
        let mut roboter = Roboter::new();
        roboter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        roboter.set_param("amount", 0.0);
        let expected = roboter.latency_samples();
        let mut observed = None;
        let blocks = expected.div_ceil(MAX_BLOCK_SIZE) + 2;
        for block in 0..blocks {
            let mut buffer = AudioBuffer::new();
            if block == 0 {
                buffer.channels[0][0] = 1.0;
                buffer.channels[1][0] = 1.0
            }
            roboter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for frame in 0..MAX_BLOCK_SIZE {
                if buffer.channels[0][frame].abs() > 0.9 {
                    observed = Some(block * MAX_BLOCK_SIZE + frame);
                    break;
                }
            }
        }
        assert_eq!(observed, Some(expected));
        assert!(roboter.tail_samples() >= (48_000.0 * 0.06) as usize)
    }
    #[test]
    fn roboter_harmony_changes_fade_and_processing_stays_finite() {
        let mut roboter = Roboter::new();
        roboter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        roboter.set_param("amount", 1.0);
        let mut energy = 0.0_f32;
        let mut maximum_jump = 0.0_f32;
        let mut previous = [0.0_f32; MAX_CHANNELS];
        let delay_capacity = roboter.delay[0].capacity();
        let history_capacity = roboter.chroma_history.capacity();
        for block in 0..72 {
            if block == 32 {
                roboter.set_param("number", 5.0)
            }
            if block == 56 {
                roboter.set_param("number", 0.0)
            }
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let index = block * MAX_BLOCK_SIZE + frame;
                let sample = (2.0 * PI * 445.0 * index as f32 / 48_000.0).sin() * 0.3;
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            roboter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for channel in 0..MAX_CHANNELS {
                for sample in &buffer.channels[channel][..MAX_BLOCK_SIZE] {
                    assert!(sample.is_finite());
                    energy += sample * sample;
                    maximum_jump = maximum_jump.max((*sample - previous[channel]).abs());
                    previous[channel] = *sample
                }
            }
        }
        assert!(energy > 1.0);
        assert!(roboter.voiced);
        assert!((roboter.voices[0].target_ratio - 1.0).abs() > 0.001);
        assert!(maximum_jump < 0.7, "sample discontinuity {maximum_jump}");
        assert_eq!(roboter.delay[0].capacity(), delay_capacity);
        assert_eq!(roboter.chroma_history.capacity(), history_capacity)
    }
    #[test]
    fn colorizer_reports_and_realizes_exact_latency() {
        let mut effect = Colorizer::new();
        effect.set_param("mix", 0.0);
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        let mut observed = None;
        for block in 0..6 {
            let mut buffer = AudioBuffer::new();
            if block == 0 {
                buffer.channels[0][0] = 1.0;
                buffer.channels[1][0] = 1.0;
            }
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            for frame in 0..MAX_BLOCK_SIZE {
                if buffer.channels[0][frame].abs() > 0.99 {
                    observed = Some(block * MAX_BLOCK_SIZE + frame);
                    break;
                }
            }
        }
        assert_eq!(effect.latency_samples(), COLORIZER_FFT_SIZE);
        assert_eq!(observed, Some(COLORIZER_FFT_SIZE))
    }
    #[test]
    fn colorizer_hann_cola_reconstructs_below_minus_60_db() {
        let mut effect = Colorizer::new();
        effect.set_param("depth", 0.0);
        effect.set_param("decay", 0.0);
        effect.set_param("mix", 1.0);
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        let total = COLORIZER_FFT_SIZE + COLORIZER_HOP_SIZE * 12;
        let input: Vec<f32> = (0..total)
            .map(|sample| {
                let time = sample as f32 / 48_000.0;
                (2.0 * PI * 173.0 * time).sin() * 0.23
                    + (2.0 * PI * 997.0 * time).sin() * 0.17
                    + (2.0 * PI * 4_123.0 * time).sin() * 0.09
            })
            .collect();
        let mut output = Vec::with_capacity(total);
        for start in (0..total).step_by(MAX_BLOCK_SIZE) {
            let frames = (total - start).min(MAX_BLOCK_SIZE);
            let mut buffer = AudioBuffer::new();
            buffer.channels[0][..frames].copy_from_slice(&input[start..start + frames]);
            buffer.channels[1][..frames].copy_from_slice(&input[start..start + frames]);
            effect.process(&[], &mut buffer, frames);
            output.extend_from_slice(&buffer.channels[0][..frames]);
        }
        let start = COLORIZER_FFT_SIZE + COLORIZER_HOP_SIZE * 3;
        let mut signal = 0.0_f64;
        let mut error = 0.0_f64;
        for index in start..total {
            let expected = input[index - COLORIZER_FFT_SIZE];
            signal += f64::from(expected * expected);
            let delta = output[index] - expected;
            error += f64::from(delta * delta);
        }
        let relative_db = 10.0 * (error.max(1e-30) / signal.max(1e-30)).log10();
        assert!(
            relative_db < -60.0,
            "COLA reconstruction error {relative_db:.1} dB"
        )
    }
    #[test]
    fn colorizer_mask_tracks_pitch_classes_across_octaves() {
        let mut effect = Colorizer::new();
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        effect.set_param("depth", 1.0);
        effect.set_param("resonance", 0.9);
        for pitch in 0..12 {
            effect.set_param(&format!("pitch{pitch}"), if pitch == 0 { 1.0 } else { 0.0 });
        }
        effect.rebuild_mask();
        for frequency in [65.406_f32, 130.813, 261.626, 523.251, 1046.502] {
            let bin = (frequency * COLORIZER_FFT_SIZE as f32 / 48_000.0).round() as usize;
            assert!(
                effect.mask_target[bin] > 0.72,
                "C octave {frequency} Hz was closed"
            )
        }
        let off_bin = (369.994 * COLORIZER_FFT_SIZE as f32 / 48_000.0).round() as usize;
        assert!(effect.mask_target[off_bin] < 0.2)
    }
    #[test]
    fn colorizer_decay_is_finite_and_midi_gate_closes_without_notes() {
        let mut effect = Colorizer::new();
        effect.set_param("decay", 1.0);
        effect.set_param("depth", 1.0);
        effect.set_param("mix", 1.0);
        effect.set_param("midi", 1.0);
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        assert!(effect.wants_midi());
        effect.rebuild_mask();
        assert!(effect.mask_target.iter().all(|gain| *gain == 0.0));
        let event = NoteEvent {
            sample_offset: 0,
            kind: NoteEventKind::NoteOn {
                note_id: 1,
                pitch: 60,
                velocity: 1.0,
                tuning_cents: 0.0,
            },
        };
        let mut energy = 0.0_f64;
        for block in 0..20 {
            let mut buffer = AudioBuffer::new();
            if block == 0 {
                buffer.channels[0][COLORIZER_HOP_SIZE / 2] = 1.0;
                buffer.channels[1][COLORIZER_HOP_SIZE / 2] = 1.0;
                effect.process(&[event], &mut buffer, MAX_BLOCK_SIZE);
            } else {
                effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            }
            for sample in &buffer.channels[0][..MAX_BLOCK_SIZE] {
                assert!(sample.is_finite());
                energy += f64::from(sample * sample);
            }
        }
        assert_eq!(effect.midi_mask, 1);
        assert!(energy.is_finite());
        assert!(effect.decay_coefficient() < 1.0)
    }
    #[test]
    fn colorizer_held_phase_is_stable_and_realtime_buffers_do_not_grow() {
        let mut effect = Colorizer::new();
        effect.set_param("decay", 0.88);
        effect.set_param("depth", 0.0);
        effect.set_param("mix", 1.0);
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        let capacities: [(usize, usize, usize, usize, usize); MAX_CHANNELS] =
            std::array::from_fn(|channel| {
                (
                    effect.channels[channel].input_ring.capacity(),
                    effect.channels[channel].ola_ring.capacity(),
                    effect.channels[channel].spectrum.capacity(),
                    effect.channels[channel].scratch.capacity(),
                    effect.channels[channel].held.capacity(),
                )
            });
        let mut output = Vec::new();
        for block in 0..20 {
            let mut buffer = AudioBuffer::new();
            if block < 8 {
                for frame in 0..MAX_BLOCK_SIZE {
                    let sample = block * MAX_BLOCK_SIZE + frame;
                    let value = (2.0 * PI * 440.0 * sample as f32 / 48_000.0).sin() * 0.3;
                    buffer.channels[0][frame] = value;
                    buffer.channels[1][frame] = value;
                }
            }
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
            output.extend_from_slice(&buffer.channels[0][..MAX_BLOCK_SIZE]);
        }
        let tail_start = COLORIZER_FFT_SIZE + 8 * MAX_BLOCK_SIZE + COLORIZER_HOP_SIZE;
        let period = (48_000.0_f32 / 445.3125).round() as usize;
        let mut correlation = 0.0_f64;
        let mut left_energy = 0.0_f64;
        let mut right_energy = 0.0_f64;
        for index in tail_start..output.len() - period {
            correlation += f64::from(output[index] * output[index + period]);
            left_energy += f64::from(output[index] * output[index]);
            right_energy += f64::from(output[index + period] * output[index + period]);
        }
        let normalized = correlation / (left_energy * right_energy).sqrt().max(1e-20);
        assert!(
            normalized > 0.9,
            "unstable held phase correlation {normalized:.3}"
        );
        for channel in 0..MAX_CHANNELS {
            assert_eq!(
                capacities[channel],
                (
                    effect.channels[channel].input_ring.capacity(),
                    effect.channels[channel].ola_ring.capacity(),
                    effect.channels[channel].spectrum.capacity(),
                    effect.channels[channel].scratch.capacity(),
                    effect.channels[channel].held.capacity(),
                )
            )
        }
    }
}
