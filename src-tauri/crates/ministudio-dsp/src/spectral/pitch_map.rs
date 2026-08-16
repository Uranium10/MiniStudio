use rustfft::num_complex::Complex32;

use super::{
    analysis::{SpectralAnalyzer, MAX_SPECTRAL_PEAKS},
    stft::SpectralFrame,
    wrap_phase,
};
use crate::MAX_CHANNELS;

#[cfg(test)]
use super::analysis::SpectralPeak;

const MAX_FUNDAMENTALS: usize = 8;

/// Pitch judgment is unstable below this frequency (too few cycles per
/// analysis window, and the ear localizes bass pitch poorly anyway). Peaks
/// down here are left at their source frequency instead of chasing a target
/// pitch class.
const MIN_MAPPABLE_HZ: f32 = 60.0;

/// A peak that would have to move further than this to reach an enabled
/// pitch class is more likely a different note entirely than an out-of-tune
/// one. Forcing it onto the grid anyway produces an audible, un-musical
/// jump, so it is left unshifted instead.
const MAX_SHIFT_CENTS: f32 = 400.0;

/// The phase-locked mapped component (never the residual pass-through, which
/// already exactly tracks the dry signal) is gated toward silence once the
/// dry source's own broadband envelope has dropped well below its recent
/// level. This is what actually shortens ringing: the mapped signal can be
/// frequency-shifted well away from wherever the dry signal still has
/// energy, so gating bin-for-bin against dry would just erase the shift
/// itself instead of the tail left behind after the source releases.
/// Fraction of the remaining gap to `target` covered per hop: attack (gate
/// opening) is fast so ringing does not linger, release (gate closing) is
/// slow so a genuine decaying tone does not chatter through the gate.
const GATE_ATTACK_STEP: f32 = 0.35;
const GATE_RELEASE_STEP: f32 = 0.05;
/// Decay rate of the slow reference envelope that the current dry level is
/// compared against, applied once per hop.
const GATE_REFERENCE_DECAY: f32 = 0.995;

#[derive(Clone, Copy)]
struct Fundamental {
    frequency_hz: f32,
    score: f32,
    ratio: f32,
}

impl Fundamental {
    const EMPTY: Self = Self {
        frequency_hz: 0.0,
        score: 0.0,
        ratio: 1.0,
    };
}

/// Bounded, stereo-coherent tonal pitch mapper.
///
/// The analyzer and all synthesis workspaces are prepared up-front. One linked
/// map is applied to the original L/R spectra; the channel complex values are
/// never collapsed to mono. Stable peak regions use identity-style relative
/// phase locking, while transient and residual energy stays at its source bin.
pub(crate) struct PitchMapProcessor {
    analyzer: SpectralAnalyzer,
    fft_size: usize,
    hop_size: usize,
    sample_rate: f32,
    original: [Vec<Complex32>; MAX_CHANNELS],
    output: [Vec<Complex32>; MAX_CHANNELS],
    mapped_magnitude: [Vec<f32>; MAX_CHANNELS],
    mapped_linked: Vec<f32>,
    mapped_omega: Vec<f32>,
    mapped_source: Vec<u16>,
    mapped_weight: Vec<f32>,
    mapped_region_peak: Vec<u16>,
    synthesis_phase: [Vec<f32>; MAX_CHANNELS],
    previous_source: Vec<u16>,
    next_active: Vec<bool>,
    previous_phase_anchor: Vec<bool>,
    next_phase_anchor: Vec<bool>,
    peak_bins: [usize; MAX_SPECTRAL_PEAKS],
    peak_frequencies: [f32; MAX_SPECTRAL_PEAKS],
    peak_ratios: [f32; MAX_SPECTRAL_PEAKS],
    peak_strengths: [f32; MAX_SPECTRAL_PEAKS],
    sorted_frequencies: [f32; MAX_SPECTRAL_PEAKS],
    sorted_ratios: [f32; MAX_SPECTRAL_PEAKS],
    sorted_strengths: [f32; MAX_SPECTRAL_PEAKS],
    mapped_peaks: [usize; MAX_SPECTRAL_PEAKS],
    fundamentals: [Fundamental; MAX_FUNDAMENTALS],
    fundamental_count: usize,
    /// Current smoothed gain applied to the mapped component only.
    gate_gain: f32,
    /// Slow envelope of recent dry broadband energy; the gate's threshold.
    gate_reference: f32,
}

impl PitchMapProcessor {
    pub(crate) fn new(sample_rate: f32, fft_size: usize, hop_size: usize) -> Self {
        let bins = fft_size / 2 + 1;
        Self {
            analyzer: SpectralAnalyzer::new(sample_rate, fft_size, hop_size),
            fft_size,
            hop_size,
            sample_rate,
            original: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); bins]),
            output: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); bins]),
            mapped_magnitude: std::array::from_fn(|_| vec![0.0; bins]),
            mapped_linked: vec![0.0; bins],
            mapped_omega: vec![0.0; bins],
            mapped_source: vec![0; bins],
            mapped_weight: vec![0.0; bins],
            mapped_region_peak: vec![0; bins],
            synthesis_phase: std::array::from_fn(|_| vec![0.0; bins]),
            previous_source: vec![u16::MAX; bins],
            next_active: vec![false; bins],
            previous_phase_anchor: vec![false; bins],
            next_phase_anchor: vec![false; bins],
            peak_bins: [0; MAX_SPECTRAL_PEAKS],
            peak_frequencies: [0.0; MAX_SPECTRAL_PEAKS],
            peak_ratios: [1.0; MAX_SPECTRAL_PEAKS],
            peak_strengths: [0.0; MAX_SPECTRAL_PEAKS],
            sorted_frequencies: [0.0; MAX_SPECTRAL_PEAKS],
            sorted_ratios: [1.0; MAX_SPECTRAL_PEAKS],
            sorted_strengths: [0.0; MAX_SPECTRAL_PEAKS],
            mapped_peaks: [0; MAX_SPECTRAL_PEAKS],
            fundamentals: [Fundamental::EMPTY; MAX_FUNDAMENTALS],
            fundamental_count: 0,
            gate_gain: 1.0,
            gate_reference: 0.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.analyzer.reset();
        for channel in 0..MAX_CHANNELS {
            self.original[channel].fill(Complex32::new(0.0, 0.0));
            self.output[channel].fill(Complex32::new(0.0, 0.0));
            self.mapped_magnitude[channel].fill(0.0);
            self.synthesis_phase[channel].fill(0.0);
        }
        self.mapped_linked.fill(0.0);
        self.mapped_omega.fill(0.0);
        self.mapped_source.fill(0);
        self.mapped_weight.fill(0.0);
        self.mapped_region_peak.fill(0);
        self.previous_source.fill(u16::MAX);
        self.next_active.fill(false);
        self.previous_phase_anchor.fill(false);
        self.next_phase_anchor.fill(false);
        self.fundamentals.fill(Fundamental::EMPTY);
        self.fundamental_count = 0;
        self.gate_gain = 1.0;
        self.gate_reference = 0.0;
    }

    pub(crate) fn analyzer(&self) -> &SpectralAnalyzer {
        &self.analyzer
    }

    #[cfg(test)]
    pub(crate) fn stereo_relation_error(
        &self,
        right_over_left: f32,
    ) -> (f32, f32, usize, Complex32, Complex32) {
        let error = |spectra: &[Vec<Complex32>; MAX_CHANNELS]| {
            spectra[0]
                .iter()
                .zip(&spectra[1])
                .map(|(left, right)| (*right - *left * right_over_left).norm())
                .fold(0.0, f32::max)
        };
        let mut worst_bin = 0;
        let mut worst_error = 0.0;
        for bin in 0..self.output[0].len() {
            let candidate = (self.output[1][bin] - self.output[0][bin] * right_over_left).norm();
            if candidate > worst_error {
                worst_error = candidate;
                worst_bin = bin;
            }
        }
        (
            error(&self.original),
            error(&self.output),
            worst_bin,
            self.output[0][worst_bin],
            self.output[1][worst_bin],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process(
        &mut self,
        frame: &mut SpectralFrame<'_>,
        target_mask: u16,
        map_amount: f32,
        transient_preserve: f32,
        color: f32,
        gate: f32,
    ) {
        self.analyzer.analyze(frame);
        let bins = frame.positive_bins();
        for channel in 0..MAX_CHANNELS {
            self.original[channel].copy_from_slice(frame.channel(channel));
            self.output[channel].fill(Complex32::new(0.0, 0.0));
            self.mapped_magnitude[channel].fill(0.0);
        }
        self.mapped_linked.fill(0.0);
        self.mapped_omega.fill(0.0);
        self.mapped_source.fill(0);
        self.mapped_weight.fill(0.0);
        self.next_active.fill(false);
        self.next_phase_anchor.fill(false);

        if target_mask == 0 || map_amount <= 1e-5 {
            for channel in 0..MAX_CHANNELS {
                frame
                    .channel_mut(channel)
                    .copy_from_slice(&self.original[channel]);
            }
            return;
        }

        self.build_peak_map(target_mask, color);
        let peak_count = self.analyzer.peaks().len().min(MAX_SPECTRAL_PEAKS);
        if peak_count == 0 {
            for channel in 0..MAX_CHANNELS {
                frame
                    .channel_mut(channel)
                    .copy_from_slice(&self.original[channel]);
            }
            return;
        }

        let sorted_count = peak_count;
        for index in 0..peak_count {
            let peak = self.analyzer.peaks()[index];
            let insertion = (0..index)
                .find(|sorted| peak.bin < self.peak_bins[*sorted])
                .unwrap_or(index);
            for slot in (insertion + 1..=index).rev() {
                self.peak_bins[slot] = self.peak_bins[slot - 1];
                self.sorted_frequencies[slot] = self.sorted_frequencies[slot - 1];
                self.sorted_ratios[slot] = self.sorted_ratios[slot - 1];
                self.sorted_strengths[slot] = self.sorted_strengths[slot - 1];
            }
            self.peak_bins[insertion] = peak.bin;
            self.sorted_frequencies[insertion] = self.peak_frequencies[index];
            self.sorted_ratios[insertion] = self.peak_ratios[index];
            self.sorted_strengths[insertion] = self.peak_strengths[index];
        }

        let width = 2.2 + color.clamp(0.0, 1.0) * 4.8;
        let amount = map_amount.clamp(0.0, 1.0);
        let transient = transient_preserve.clamp(0.0, 1.0);
        let mut sorted_peak_index = 0;

        for bin in 0..bins {
            while sorted_peak_index + 1 < sorted_count
                && self.peak_bins[sorted_peak_index + 1].abs_diff(bin)
                    <= self.peak_bins[sorted_peak_index].abs_diff(bin)
            {
                sorted_peak_index += 1;
            }
            let peak_bin = self.peak_bins[sorted_peak_index];
            let ratio = self.sorted_ratios[sorted_peak_index];
            let peak_frequency = self.sorted_frequencies[sorted_peak_index];
            let peak_strength = self.sorted_strengths[sorted_peak_index];
            let distance = peak_bin.abs_diff(bin) as f32;
            let region_weight = (-0.5 * (distance / width).powi(2)).exp();
            let local_energy = if self.analyzer.magnitudes()[peak_bin] > 1e-12 {
                (self.analyzer.magnitudes()[bin] / self.analyzer.magnitudes()[peak_bin])
                    .sqrt()
                    .min(1.0)
            } else {
                0.0
            };
            let tonal =
                (peak_strength * region_weight * (0.55 + 0.45 * local_energy)).clamp(0.0, 1.0);
            let mapped = (amount * tonal * (1.0 - transient * self.analyzer.transient_mask()[bin]))
                .clamp(0.0, 1.0);
            let residual = 1.0 - mapped;
            for channel in 0..MAX_CHANNELS {
                self.output[channel][bin] += self.original[channel][bin] * residual;
            }
            if mapped <= 1e-5 {
                continue;
            }

            let bin_hz = self.sample_rate / self.fft_size as f32;
            let source_frequency =
                (peak_frequency + (bin as f32 - peak_bin as f32) * bin_hz).max(0.0);
            let mapped_frequency = source_frequency * ratio;
            let destination = mapped_frequency / bin_hz;
            let lower = destination.floor() as usize;
            if lower >= bins {
                continue;
            }
            let fraction = destination - lower as f32;
            self.scatter_mapped_bin(bin, lower, 1.0 - fraction, mapped_frequency, mapped);
            if fraction > 1e-5 && lower + 1 < bins {
                self.scatter_mapped_bin(bin, lower + 1, fraction, mapped_frequency, mapped);
            }
        }

        let gate = gate.clamp(0.0, 1.0);
        if gate > 1e-5 {
            self.apply_spectral_gate(bins, gate);
        } else {
            // Gate fully open when the control is at zero: skip the
            // envelope/threshold work entirely and let it re-settle to unity
            // so a later re-enable does not resume from a stale gain.
            self.gate_gain = 1.0;
        }

        for bin in 0..bins {
            self.mapped_linked[bin] = (0.5
                * (self.mapped_magnitude[0][bin].powi(2) + self.mapped_magnitude[1][bin].powi(2)))
            .sqrt();
        }
        let mapped_peak_count = self.assign_mapped_phase_regions();
        if mapped_peak_count > 0 {
            self.render_phase_locked_component(mapped_peak_count);
        }

        for channel in 0..MAX_CHANNELS {
            frame
                .channel_mut(channel)
                .copy_from_slice(&self.output[channel]);
        }
        std::mem::swap(&mut self.previous_phase_anchor, &mut self.next_phase_anchor);
        self.previous_source.copy_from_slice(&self.mapped_source);
    }

    fn build_peak_map(&mut self, target_mask: u16, color: f32) {
        let peaks = self.analyzer.peaks();
        self.fundamentals.fill(Fundamental::EMPTY);
        self.fundamental_count = 0;

        for candidate in peaks
            .iter()
            .filter(|peak| (40.0..=2_000.0).contains(&peak.frequency_hz) && peak.prominence > 0.08)
        {
            let mut score = candidate.magnitude * (0.35 + 0.65 * candidate.prominence);
            for partial in peaks {
                let harmonic = (partial.frequency_hz / candidate.frequency_hz).round();
                if !(2.0..=16.0).contains(&harmonic) {
                    continue;
                }
                let expected = candidate.frequency_hz * harmonic;
                let cents = 1200.0 * (partial.frequency_hz / expected).log2().abs();
                if cents < 38.0 {
                    score += partial.magnitude * partial.prominence / harmonic.sqrt();
                }
            }
            if self.fundamentals[..self.fundamental_count]
                .iter()
                .any(|existing| {
                    let ratio = candidate.frequency_hz / existing.frequency_hz;
                    let harmonic = ratio.round().max(1.0);
                    let harmonic_cents = 1200.0 * (ratio / harmonic).log2().abs();
                    (1200.0 * ratio.log2()).abs() < 70.0
                        || (harmonic <= 16.0 && harmonic_cents < 38.0)
                })
            {
                continue;
            }
            let fundamental = Fundamental {
                frequency_hz: candidate.frequency_hz,
                score,
                ratio: constrained_ratio(
                    candidate.frequency_hz,
                    nearest_allowed_ratio(candidate.frequency_hz, target_mask),
                ),
            };
            let insertion = (0..self.fundamental_count)
                .find(|index| score > self.fundamentals[*index].score)
                .unwrap_or(self.fundamental_count);
            if insertion < MAX_FUNDAMENTALS {
                let end = self.fundamental_count.min(MAX_FUNDAMENTALS - 1);
                for index in (insertion + 1..=end).rev() {
                    self.fundamentals[index] = self.fundamentals[index - 1];
                }
                self.fundamentals[insertion] = fundamental;
                self.fundamental_count = (self.fundamental_count + 1).min(MAX_FUNDAMENTALS);
            }
        }

        for (index, peak) in peaks.iter().enumerate() {
            let mut ratio = nearest_allowed_ratio(peak.frequency_hz, target_mask);
            let mut best_cents = 39.0_f32;
            for fundamental in &self.fundamentals[..self.fundamental_count] {
                let harmonic = (peak.frequency_hz / fundamental.frequency_hz).round();
                if !(1.0..=16.0).contains(&harmonic) {
                    continue;
                }
                let expected = fundamental.frequency_hz * harmonic;
                let cents = 1200.0 * (peak.frequency_hz / expected).log2().abs();
                if cents < best_cents {
                    best_cents = cents;
                    ratio = fundamental.ratio;
                }
            }
            self.peak_ratios[index] = constrained_ratio(peak.frequency_hz, ratio);
            self.peak_frequencies[index] = peak.frequency_hz;
            let exponent = 1.75 - color.clamp(0.0, 1.0) * 1.25;
            // Prominence distinguishes a tonal peak from its floor, but must
            // not cap the requested correction amount: a Hann-windowed sine
            // deliberately has energetic shoulders and modest prominence.
            let prominence = peak.prominence.powf(exponent);
            self.peak_strengths[index] = if best_cents < 39.0 {
                0.84 + 0.16 * prominence
            } else {
                0.45 + 0.35 * prominence
            }
            .clamp(0.0, 1.0);
        }
    }

    fn scatter_mapped_bin(
        &mut self,
        source: usize,
        destination: usize,
        interpolation: f32,
        mapped_frequency: f32,
        amount: f32,
    ) {
        let gain = amount * interpolation.max(0.0);
        if gain <= 1e-7 {
            return;
        }
        for channel in 0..MAX_CHANNELS {
            self.mapped_magnitude[channel][destination] +=
                self.original[channel][source].norm() * gain;
        }
        let weight = self.analyzer.magnitudes()[source] * gain;
        if weight > self.mapped_weight[destination] {
            self.mapped_weight[destination] = weight;
            self.mapped_source[destination] = source.min(u16::MAX as usize) as u16;
            self.mapped_omega[destination] =
                2.0 * std::f32::consts::PI * mapped_frequency / self.sample_rate;
        }
        self.next_active[destination] = true;
    }

    /// Shortens ringing left over after the dry source releases, by gating
    /// the phase-locked mapped component against the dry signal's own
    /// broadband envelope. Deliberately not a per-bin dry-vs-wet comparison:
    /// the mapped content is frequency-*shifted*, so it legitimately lands
    /// where dry has nothing, and gating that bin-for-bin would erase the
    /// pitch correction itself instead of just its tail.
    fn apply_spectral_gate(&mut self, bins: usize, gate: f32) {
        let dry = self.analyzer.magnitudes();
        let frame_energy: f32 = dry[..bins].iter().sum();
        self.gate_reference = if frame_energy > self.gate_reference {
            frame_energy
        } else {
            self.gate_reference * GATE_REFERENCE_DECAY + frame_energy * (1.0 - GATE_REFERENCE_DECAY)
        };
        // `gate` sweeps the relative threshold from barely-there (only once
        // the source has nearly fully released) to aggressive (closes as
        // soon as the source dips modestly below its recent level).
        let threshold = self.gate_reference * (0.02 + gate * 0.6) + 1e-12;
        let headroom = (frame_energy / threshold).min(1.0);
        // Cubic soft knee: unity once the source is present, an inaudible
        // taper rather than a hard chop as it falls toward silence.
        let target = headroom * headroom * headroom;
        let step = if target > self.gate_gain {
            GATE_ATTACK_STEP
        } else {
            GATE_RELEASE_STEP
        };
        self.gate_gain += (target - self.gate_gain) * step;
        let applied = self.gate_gain;
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                self.mapped_magnitude[channel][bin] *= applied;
            }
        }
    }

    fn assign_mapped_phase_regions(&mut self) -> usize {
        let bins = self.mapped_linked.len();
        let maximum = self.mapped_linked.iter().copied().fold(0.0_f32, f32::max);
        let threshold = maximum * 1e-4;
        let mut count = 0;
        for bin in 1..bins.saturating_sub(1) {
            if self.mapped_linked[bin] >= threshold
                && self.mapped_linked[bin] > self.mapped_linked[bin - 1]
                && self.mapped_linked[bin] >= self.mapped_linked[bin + 1]
            {
                if count < MAX_SPECTRAL_PEAKS {
                    self.mapped_peaks[count] = bin;
                    count += 1;
                }
            }
        }
        if count == 0 && maximum > 1e-12 {
            self.mapped_peaks[0] = self
                .mapped_linked
                .iter()
                .position(|value| *value == maximum)
                .unwrap_or(0);
            count = 1;
        }
        if count == 0 {
            return 0;
        }
        let mut peak_index = 0;
        for bin in 0..bins {
            while peak_index + 1 < count
                && self.mapped_peaks[peak_index + 1].abs_diff(bin)
                    <= self.mapped_peaks[peak_index].abs_diff(bin)
            {
                peak_index += 1;
            }
            self.mapped_region_peak[bin] = self.mapped_peaks[peak_index] as u16;
        }
        count
    }

    fn render_phase_locked_component(&mut self, peak_count: usize) {
        for &peak in &self.mapped_peaks[..peak_count] {
            let source = self.mapped_source[peak] as usize;
            self.next_phase_anchor[peak] = true;
            for channel in 0..MAX_CHANNELS {
                let restart = !self.previous_phase_anchor[peak]
                    || self.previous_source[peak] != self.mapped_source[peak];
                self.synthesis_phase[channel][peak] = if restart {
                    self.original[channel][source].arg()
                } else {
                    wrap_phase(
                        self.synthesis_phase[channel][peak]
                            + self.mapped_omega[peak] * self.hop_size as f32,
                    )
                };
            }
        }

        for bin in 0..self.mapped_linked.len() {
            if !self.next_active[bin] || self.mapped_linked[bin] <= 1e-12 {
                continue;
            }
            let peak = self.mapped_region_peak[bin] as usize;
            let source = self.mapped_source[bin] as usize;
            let peak_source = self.mapped_source[peak] as usize;
            for channel in 0..MAX_CHANNELS {
                let relative = wrap_phase(
                    self.original[channel][source].arg()
                        - self.original[channel][peak_source].arg(),
                );
                let phase = wrap_phase(self.synthesis_phase[channel][peak] + relative);
                self.output[channel][bin] +=
                    Complex32::from_polar(self.mapped_magnitude[channel][bin], phase);
            }
        }
    }
}

fn nearest_allowed_ratio(frequency_hz: f32, target_mask: u16) -> f32 {
    if frequency_hz <= 0.0 || target_mask == 0 {
        return 1.0;
    }
    let pitch = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    let center = pitch.round() as i32;
    let mut best = center;
    let mut best_distance = f32::MAX;
    for candidate in center - 12..=center + 12 {
        if target_mask & (1 << candidate.rem_euclid(12)) == 0 {
            continue;
        }
        let distance = (candidate as f32 - pitch).abs();
        if distance < best_distance {
            best = candidate;
            best_distance = distance;
        }
    }
    2.0_f32.powf((best as f32 - pitch) / 12.0)
}

/// Refuses a shift that isn't musically defensible: unstable low-frequency
/// pitch judgment, or a target so far away it reads as a different note
/// rather than a correction. See `MIN_MAPPABLE_HZ`/`MAX_SHIFT_CENTS`.
fn constrained_ratio(frequency_hz: f32, ratio: f32) -> f32 {
    if frequency_hz < MIN_MAPPABLE_HZ {
        return 1.0;
    }
    let cents = 1200.0 * ratio.log2().abs();
    if cents > MAX_SHIFT_CENTS {
        return 1.0;
    }
    ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ratio_snaps_to_enabled_pitch_class() {
        let c_only = 1 << 0;
        let ratio = nearest_allowed_ratio(277.182_65, c_only); // C#4 -> C4
        let mapped = 277.182_65 * ratio;
        assert!((mapped - 261.625_55).abs() < 0.01, "mapped={mapped}");
    }

    #[test]
    fn harmonic_family_uses_the_fundamental_ratio() {
        let mut processor = PitchMapProcessor::new(48_000.0, 1024, 256);
        processor.analyzer.replace_peaks_for_test(&[
            SpectralPeak {
                bin: 6,
                frequency_hz: 277.182_65,
                magnitude: 1.0,
                prominence: 0.95,
            },
            SpectralPeak {
                bin: 12,
                frequency_hz: 554.365_3,
                magnitude: 0.6,
                prominence: 0.9,
            },
        ]);
        processor.build_peak_map(1 << 0, 0.7);
        assert_eq!(processor.fundamental_count, 1);
        assert!((processor.peak_ratios[0] - processor.peak_ratios[1]).abs() < 1e-6);
    }
}
