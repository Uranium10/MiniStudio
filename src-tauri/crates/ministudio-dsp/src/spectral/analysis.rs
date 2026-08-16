use rustfft::num_complex::Complex32;

use super::{hpcp::Hpcp, peaks::detect_spectral_peaks, stft::SpectralFrame};

pub(crate) const MAX_SPECTRAL_PEAKS: usize = 96;
pub(crate) const HPCP_BINS: usize = 36;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SpectralPeak {
    pub(crate) bin: usize,
    pub(crate) frequency_hz: f32,
    pub(crate) magnitude: f32,
    pub(crate) prominence: f32,
}

impl SpectralPeak {
    pub(crate) const EMPTY: Self = Self {
        bin: 0,
        frequency_hz: 0.0,
        magnitude: 0.0,
        prominence: 0.0,
    };
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TransientSummary {
    pub(crate) spectral_flux: f32,
    pub(crate) strength: f32,
}

/// One linked-stereo analysis pass shared by pitch mapping, transient
/// preservation, automatic tonality, and the UI analyzer.
///
/// All vectors are allocated by `new`. `analyze` performs no allocation and
/// never mutates the channel spectra it observes.
pub(crate) struct SpectralAnalyzer {
    sample_rate: f32,
    fft_size: usize,
    hop_size: usize,
    magnitudes: Vec<f32>,
    phases: Vec<f32>,
    previous_phases: Vec<f32>,
    previous_magnitudes: Vec<f32>,
    transient_mask: Vec<f32>,
    peaks: [SpectralPeak; MAX_SPECTRAL_PEAKS],
    peak_count: usize,
    hpcp: Hpcp,
    transient: TransientSummary,
    initialized: bool,
}

impl SpectralAnalyzer {
    pub(crate) fn new(sample_rate: f32, fft_size: usize, hop_size: usize) -> Self {
        let bins = fft_size / 2 + 1;
        Self {
            sample_rate: sample_rate.max(8_000.0),
            fft_size,
            hop_size,
            magnitudes: vec![0.0; bins],
            phases: vec![0.0; bins],
            previous_phases: vec![0.0; bins],
            previous_magnitudes: vec![0.0; bins],
            transient_mask: vec![0.0; bins],
            peaks: [SpectralPeak::EMPTY; MAX_SPECTRAL_PEAKS],
            peak_count: 0,
            hpcp: Hpcp::new(),
            transient: TransientSummary::default(),
            initialized: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.magnitudes.fill(0.0);
        self.phases.fill(0.0);
        self.previous_phases.fill(0.0);
        self.previous_magnitudes.fill(0.0);
        self.transient_mask.fill(0.0);
        self.peaks.fill(SpectralPeak::EMPTY);
        self.peak_count = 0;
        self.hpcp.clear();
        self.transient = TransientSummary::default();
        self.initialized = false;
    }

    pub(crate) fn analyze(&mut self, frame: &SpectralFrame<'_>) {
        debug_assert_eq!(frame.positive_bins(), self.magnitudes.len());
        let left = frame.channel(0);
        let right = frame.channel(1);
        let mut positive_flux = 0.0_f32;
        let mut frame_energy = 1e-20_f32;

        for bin in 0..self.magnitudes.len() {
            let linked_energy = 0.5 * (left[bin].norm_sqr() + right[bin].norm_sqr());
            let magnitude = linked_energy.sqrt();
            let linked = left[bin] + right[bin];
            let phase_source = if linked.norm_sqr() > linked_energy * 1e-6 {
                linked
            } else if left[bin].norm_sqr() >= right[bin].norm_sqr() {
                left[bin]
            } else {
                right[bin]
            };
            let rise = (magnitude - self.previous_magnitudes[bin]).max(0.0);
            positive_flux += rise;
            frame_energy += magnitude;
            self.magnitudes[bin] = magnitude;
            self.phases[bin] = phase_source.arg();
        }

        let normalized_flux = (positive_flux / frame_energy).clamp(0.0, 1.0);
        // A broadband onset should preserve more of the original spectrum, but
        // isolated amplitude modulation must not classify every tonal bin as a
        // transient. Combining local rise with global flux supplies both tests.
        let global = ((normalized_flux - 0.035) / 0.22).clamp(0.0, 1.0);
        for bin in 0..self.magnitudes.len() {
            let magnitude = self.magnitudes[bin];
            let local_rise = if self.initialized {
                ((magnitude - self.previous_magnitudes[bin]).max(0.0) / (magnitude + 1e-12))
                    .clamp(0.0, 1.0)
            } else {
                1.0
            };
            let target = (global * (0.35 + 0.65 * local_rise)).clamp(0.0, 1.0);
            // Attack immediately; release over several frames to avoid a
            // transient mask chattering around a decaying drum hit.
            self.transient_mask[bin] = if target > self.transient_mask[bin] {
                target
            } else {
                self.transient_mask[bin] * 0.58 + target * 0.42
            };
        }

        self.peak_count = detect_spectral_peaks(
            &self.magnitudes,
            &self.phases,
            &self.previous_phases,
            self.sample_rate,
            self.fft_size,
            self.hop_size,
            self.initialized,
            &mut self.peaks,
        );
        self.hpcp.analyze(&self.peaks[..self.peak_count], 440.0);
        self.transient = TransientSummary {
            spectral_flux: normalized_flux,
            strength: global,
        };
        self.previous_phases.copy_from_slice(&self.phases);
        self.previous_magnitudes.copy_from_slice(&self.magnitudes);
        self.initialized = true;
    }

    pub(crate) fn magnitudes(&self) -> &[f32] {
        &self.magnitudes
    }

    pub(crate) fn phases(&self) -> &[f32] {
        &self.phases
    }

    pub(crate) fn transient_mask(&self) -> &[f32] {
        &self.transient_mask
    }

    pub(crate) fn transient(&self) -> TransientSummary {
        self.transient
    }

    pub(crate) fn peaks(&self) -> &[SpectralPeak] {
        &self.peaks[..self.peak_count]
    }

    pub(crate) fn hpcp(&self) -> &[f32; HPCP_BINS] {
        self.hpcp.values()
    }

    pub(crate) fn strongest_pitch_class(&self) -> (usize, f32) {
        self.hpcp.strongest_pitch_class()
    }

    #[cfg(test)]
    pub(crate) fn replace_peaks_for_test(&mut self, peaks: &[SpectralPeak]) {
        self.peaks.fill(SpectralPeak::EMPTY);
        self.peak_count = peaks.len().min(MAX_SPECTRAL_PEAKS);
        self.peaks[..self.peak_count].copy_from_slice(&peaks[..self.peak_count]);
    }
}

#[inline(always)]
pub(crate) fn complex_phase(value: Complex32) -> f32 {
    value.im.atan2(value.re)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectral::stft::{StftConfig, StftEngine, StftPlan};
    use std::f32::consts::PI;

    #[test]
    fn linked_analysis_tracks_true_frequency_and_pitch_class() {
        let plan = StftPlan::new(StftConfig::new(1024, 256)).unwrap();
        let mut engine = StftEngine::new(plan);
        let mut analyzer = SpectralAnalyzer::new(48_000.0, 1024, 256);
        for sample in 0..4096 {
            let value = (2.0 * PI * 440.0 * sample as f32 / 48_000.0).sin() * 0.4;
            let _ = engine.process_sample([value, value], |frame| analyzer.analyze(frame));
        }
        let peak = analyzer
            .peaks()
            .iter()
            .min_by(|left, right| {
                (left.frequency_hz - 440.0)
                    .abs()
                    .total_cmp(&(right.frequency_hz - 440.0).abs())
            })
            .unwrap();
        assert!(
            (peak.frequency_hz - 440.0).abs() < 1.0,
            "frequency={}",
            peak.frequency_hz
        );
        let (pitch_class, confidence) = analyzer.strongest_pitch_class();
        assert_eq!(pitch_class, 9);
        assert!(confidence > 0.6, "confidence={confidence}");
        assert!(analyzer.transient().strength < 0.08);
    }

    #[test]
    fn broadband_onset_produces_a_transient_mask_then_releases() {
        let plan = StftPlan::new(StftConfig::new(512, 128)).unwrap();
        let mut engine = StftEngine::new(plan);
        let mut analyzer = SpectralAnalyzer::new(48_000.0, 512, 128);
        for sample in 0..128 {
            let impulse = if sample == 0 { 1.0 } else { 0.0 };
            let _ = engine.process_sample([impulse, impulse], |frame| analyzer.analyze(frame));
        }
        assert!(analyzer.transient().strength > 0.7);
        assert!(
            analyzer
                .transient_mask()
                .iter()
                .copied()
                .fold(0.0_f32, f32::max)
                > 0.7
        );
        for _ in 0..1024 {
            let _ = engine.process_sample([0.0, 0.0], |frame| analyzer.analyze(frame));
        }
        assert!(analyzer.transient().strength < 0.01);
    }
}
