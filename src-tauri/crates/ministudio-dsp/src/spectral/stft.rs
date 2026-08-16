use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{f32::consts::PI, sync::Arc};

use crate::MAX_CHANNELS;

/// Immutable analysis/synthesis settings. Power-of-two transforms keep the
/// realtime cost predictable across RustFFT's AVX/SSE/NEON backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StftConfig {
    pub(crate) fft_size: usize,
    pub(crate) hop_size: usize,
}

impl StftConfig {
    pub(crate) const fn new(fft_size: usize, hop_size: usize) -> Self {
        Self { fft_size, hop_size }
    }
}

/// Shareable transform plan and a numerically derived WOLA window pair.
///
/// Planning and allocation happen on the control thread. The synthesis window
/// includes both RustFFT's inverse normalization and the exact overlap sum, so
/// the streaming loop needs no divisions and reconstructs unity independently
/// of the selected valid overlap factor.
pub(crate) struct StftPlan {
    config: StftConfig,
    analysis_window: Vec<f32>,
    synthesis_window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch_len: usize,
}

impl StftPlan {
    pub(crate) fn new(config: StftConfig) -> Result<Arc<Self>, &'static str> {
        let size = config.fft_size;
        let hop = config.hop_size;
        if size < 16 || !size.is_power_of_two() {
            return Err("STFT size must be a power of two and at least 16");
        }
        if hop == 0 || hop > size || size % hop != 0 {
            return Err("STFT hop must be a non-zero divisor of the FFT size");
        }

        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());

        // A periodic sqrt-Hann analysis/synthesis pair gives a Hann product.
        // Derive the phase-dependent overlap sum instead of relying on a magic
        // 1.5/2.0 constant, allowing future 1/2, 1/4, or 1/8 hop plans safely.
        let analysis_window: Vec<f32> = (0..size)
            .map(|index| {
                (0.5 - 0.5 * (2.0 * PI * index as f32 / size as f32).cos())
                    .max(0.0)
                    .sqrt()
            })
            .collect();
        let mut overlap_sum = vec![0.0_f32; hop];
        for index in 0..size {
            overlap_sum[index % hop] += analysis_window[index] * analysis_window[index];
        }
        if overlap_sum.iter().any(|value| *value <= f32::EPSILON) {
            return Err("STFT window/hop pair cannot be perfectly reconstructed");
        }
        let synthesis_window = (0..size)
            .map(|index| analysis_window[index] / (size as f32 * overlap_sum[index % hop]))
            .collect();

        Ok(Arc::new(Self {
            config,
            analysis_window,
            synthesis_window,
            forward,
            inverse,
            scratch_len,
        }))
    }

    pub(crate) fn config(&self) -> StftConfig {
        self.config
    }

    /// The causal ring closes one complete analysis window before a sample can
    /// be reconstructed. Spectral effects must report this value to graph PDC.
    pub(crate) fn latency_samples(&self) -> usize {
        self.config.fft_size
    }
}

/// Positive-frequency view presented to an effect once per analysis hop.
/// Negative bins are rebuilt automatically before synthesis, so processors
/// cannot accidentally violate the Hermitian symmetry required for real audio.
pub(crate) struct SpectralFrame<'a> {
    spectra: &'a mut [Vec<Complex32>; MAX_CHANNELS],
    positive_bins: usize,
}

impl SpectralFrame<'_> {
    pub(crate) fn positive_bins(&self) -> usize {
        self.positive_bins
    }

    pub(crate) fn channel(&self, channel: usize) -> &[Complex32] {
        &self.spectra[channel][..self.positive_bins]
    }

    pub(crate) fn channel_mut(&mut self, channel: usize) -> &mut [Complex32] {
        &mut self.spectra[channel][..self.positive_bins]
    }
}

/// Causal stereo STFT/WOLA streamer. All vectors and FFT scratch are allocated
/// by `new`; `process_sample` performs no allocation, locking, logging, or I/O.
pub(crate) struct StftEngine {
    plan: Arc<StftPlan>,
    input_ring: [Vec<f32>; MAX_CHANNELS],
    ola_ring: [Vec<f32>; MAX_CHANNELS],
    spectra: [Vec<Complex32>; MAX_CHANNELS],
    scratch: [Vec<Complex32>; MAX_CHANNELS],
    ring_position: usize,
    samples_until_frame: usize,
}

impl StftEngine {
    pub(crate) fn new(plan: Arc<StftPlan>) -> Self {
        let size = plan.config.fft_size;
        let hop_size = plan.config.hop_size;
        let scratch_len = plan.scratch_len;
        Self {
            plan,
            input_ring: std::array::from_fn(|_| vec![0.0; size]),
            ola_ring: std::array::from_fn(|_| vec![0.0; size]),
            spectra: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); size]),
            scratch: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); scratch_len]),
            ring_position: 0,
            // Start after one hop with zero-padded history. Waiting a complete
            // window would attenuate/loss the stream's first transient.
            samples_until_frame: hop_size,
        }
    }

    pub(crate) fn plan(&self) -> &Arc<StftPlan> {
        &self.plan
    }

    pub(crate) fn reset(&mut self) {
        for channel in 0..MAX_CHANNELS {
            self.input_ring[channel].fill(0.0);
            self.ola_ring[channel].fill(0.0);
            self.spectra[channel].fill(Complex32::new(0.0, 0.0));
            self.scratch[channel].fill(Complex32::new(0.0, 0.0));
        }
        self.ring_position = 0;
        self.samples_until_frame = self.plan.config.hop_size;
    }

    #[inline]
    pub(crate) fn process_sample<F>(
        &mut self,
        input: [f32; MAX_CHANNELS],
        mut process_frame: F,
    ) -> [f32; MAX_CHANNELS]
    where
        F: FnMut(&mut SpectralFrame<'_>),
    {
        let position = self.ring_position;
        let mut output = [0.0_f32; MAX_CHANNELS];
        for channel in 0..MAX_CHANNELS {
            self.input_ring[channel][position] = input[channel];
            output[channel] = self.ola_ring[channel][position];
            self.ola_ring[channel][position] = 0.0;
        }

        self.ring_position += 1;
        if self.ring_position == self.plan.config.fft_size {
            self.ring_position = 0;
        }
        self.samples_until_frame -= 1;
        if self.samples_until_frame == 0 {
            self.samples_until_frame = self.plan.config.hop_size;
            self.render_frame(&mut process_frame);
        }
        output
    }

    fn render_frame<F>(&mut self, process_frame: &mut F)
    where
        F: FnMut(&mut SpectralFrame<'_>),
    {
        let size = self.plan.config.fft_size;
        let bins = size / 2 + 1;
        let position = self.ring_position;

        for channel in 0..MAX_CHANNELS {
            for index in 0..size {
                let input_index = position + index;
                let input_index = if input_index >= size {
                    input_index - size
                } else {
                    input_index
                };
                self.spectra[channel][index] = Complex32::new(
                    self.input_ring[channel][input_index] * self.plan.analysis_window[index],
                    0.0,
                );
            }
            self.plan
                .forward
                .process_with_scratch(&mut self.spectra[channel], &mut self.scratch[channel]);
        }

        process_frame(&mut SpectralFrame {
            spectra: &mut self.spectra,
            positive_bins: bins,
        });

        for channel in 0..MAX_CHANNELS {
            self.spectra[channel][0].im = 0.0;
            self.spectra[channel][bins - 1].im = 0.0;
            for bin in 1..bins - 1 {
                self.spectra[channel][size - bin] = self.spectra[channel][bin].conj();
            }
            self.plan
                .inverse
                .process_with_scratch(&mut self.spectra[channel], &mut self.scratch[channel]);
            for index in 0..size {
                let output_index = position + index;
                let output_index = if output_index >= size {
                    output_index - size
                } else {
                    output_index
                };
                self.ola_ring[channel][output_index] +=
                    self.spectra[channel][index].re * self.plan.synthesis_window[index];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_wola_reconstructs_impulse_at_declared_latency() {
        let plan = StftPlan::new(StftConfig::new(1024, 256)).unwrap();
        let latency = plan.latency_samples();
        let mut engine = StftEngine::new(plan);
        let mut rendered = Vec::with_capacity(latency * 2);
        for sample in 0..latency * 2 {
            let input = if sample == 0 { 1.0 } else { 0.0 };
            rendered.push(engine.process_sample([input, -input], |_| {}));
        }
        let (peak_index, peak) = rendered
            .iter()
            .enumerate()
            .map(|(index, frame)| (index, frame[0].abs()))
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap();
        assert_eq!(peak_index, latency);
        assert!((peak - 1.0).abs() < 1e-5, "peak={peak}");
        assert!((rendered[latency][1] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn identity_wola_is_transparent_after_latency() {
        let plan = StftPlan::new(StftConfig::new(2048, 512)).unwrap();
        let latency = plan.latency_samples();
        let mut engine = StftEngine::new(plan);
        let length = latency * 4;
        let mut input = Vec::with_capacity(length);
        let mut output = Vec::with_capacity(length);
        for sample in 0..length {
            let value = (2.0 * PI * 997.0 * sample as f32 / 48_000.0).sin() * 0.37;
            input.push(value);
            output.push(engine.process_sample([value, value], |_| {})[0]);
        }
        let maximum_error = (latency..length)
            .map(|sample| (output[sample] - input[sample - latency]).abs())
            .fold(0.0_f32, f32::max);
        assert!(maximum_error < 2e-5, "maximum error={maximum_error}");
    }

    #[test]
    fn frame_exposes_positive_bins_and_restores_real_symmetry() {
        let plan = StftPlan::new(StftConfig::new(512, 128)).unwrap();
        let mut engine = StftEngine::new(plan);
        let mut visited = false;
        for _ in 0..128 {
            let _ = engine.process_sample([0.1, -0.1], |frame| {
                assert_eq!(frame.positive_bins(), 257);
                assert_eq!(frame.channel(0).len(), 257);
                frame.channel_mut(1)[3] = Complex32::new(0.25, -0.5);
                visited = true;
            });
        }
        assert!(visited);
    }
}
