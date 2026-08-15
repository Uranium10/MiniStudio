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
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize, oversample: usize, enabled: bool) {
        if !enabled {
            self.gain.advance(frames);
            self.drive.advance(frames);
            self.mix.advance(frames);
            return;
        }
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
                for phase in 0..oversample {
                    let t = (phase + 1) as f32 / oversample as f32;
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
        // 60 Hz is cheap for this fixed 256-point FFT and removes the visible
        // stepping caused by redrawing a 30 Hz source on a 60 Hz canvas.
        self.interval = (sample_rate / 60.0).round() as usize;
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
    band_enabled: [bool; 3],
    oversample: usize,
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
            band_enabled: [true; 3],
            oversample: 4,
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
        for (index, (processor, band)) in self.processors.iter_mut().zip(self.bands.iter_mut()).enumerate() {
            processor.process(band, frames, self.oversample, self.band_enabled[index]);
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
            "oversample" => self.oversample = match value.round() as usize { 4 => 4, 2 => 2, _ => 1 },
            _ => {
                for (prefix, index) in [("low", 0), ("mid", 1), ("high", 2)] {
                    if let Some(parameter) = id.strip_prefix(prefix) {
                        if parameter == "Enabled" { self.band_enabled[index] = value >= 0.5 } else { self.set_band_param(index, parameter, value) }
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
        if self.oversample > 1 { 7_usize.div_ceil(self.oversample) } else { 0 }
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
