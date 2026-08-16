use rustfft::num_complex::Complex32;

use super::{
    analysis::{SpectralAnalyzer, MAX_SPECTRAL_PEAKS},
    stft::SpectralFrame,
    wrap_phase,
};
use crate::MAX_CHANNELS;

#[cfg(test)]
use super::analysis::SpectralPeak;

const MAX_GROUPS: usize = 4;
const MAX_SHIFT_CENTS: f32 = 150.0;
const MIN_MAPPABLE_HZ: f32 = 40.0;
const RATIO_SMOOTH_SECONDS: f32 = 0.025;
const PEAK_TRACK_CENTS: f32 = 60.0;
const GROUP_TRACK_CENTS: f32 = 80.0;
const SILENCE_ENERGY: f32 = 1e-9;

#[derive(Clone, Copy, Debug)]
struct PeakPlan {
    source_bin: usize,
    region_start: usize,
    region_end: usize,
    source_frequency: f32,
    ratio: f32,
    target_frequency: f32,
    delta_bins: isize,
    synthesis_phase: [f32; MAX_CHANNELS],
}

impl PeakPlan {
    const EMPTY: Self = Self {
        source_bin: 0,
        region_start: 0,
        region_end: 0,
        source_frequency: 0.0,
        ratio: 1.0,
        target_frequency: 0.0,
        delta_bins: 0,
        synthesis_phase: [0.0; MAX_CHANNELS],
    };
}

#[derive(Clone, Copy, Debug)]
struct PeakTrack {
    frequency: f32,
    ratio: f32,
    synthesis_phase: [f32; MAX_CHANNELS],
    active: bool,
}

impl PeakTrack {
    const EMPTY: Self = Self {
        frequency: 0.0,
        ratio: 1.0,
        synthesis_phase: [0.0; MAX_CHANNELS],
        active: false,
    };
}

#[derive(Clone, Copy, Debug)]
struct GroupTrack {
    fundamental: f32,
    ratio: f32,
    active: bool,
}

impl GroupTrack {
    const EMPTY: Self = Self {
        fundamental: 0.0,
        ratio: 1.0,
        active: false,
    };
}

/// Linked-stereo, fixed-capacity harmonic-family pitch mapper.
///
/// Detection and target decisions are shared between channels. Resynthesis is
/// channel-specific and moves each complete source peak region with one bin
/// offset while retaining every bin's phase relative to its source peak.
pub(crate) struct PitchMapProcessor {
    analyzer: SpectralAnalyzer,
    fft_size: usize,
    hop_size: usize,
    sample_rate: f32,
    original: [Vec<Complex32>; MAX_CHANNELS],
    output: [Vec<Complex32>; MAX_CHANNELS],
    shifted: [Vec<Complex32>; MAX_CHANNELS],
    resonance_hold: [Vec<Complex32>; MAX_CHANNELS],
    region_owner: Vec<i16>,
    plans: [PeakPlan; MAX_SPECTRAL_PEAKS],
    peak_groups: [i8; MAX_SPECTRAL_PEAKS],
    group_ratios: [f32; MAX_GROUPS],
    previous_peaks: [PeakTrack; MAX_SPECTRAL_PEAKS],
    next_peaks: [PeakTrack; MAX_SPECTRAL_PEAKS],
    previous_peak_count: usize,
    previous_groups: [GroupTrack; MAX_GROUPS],
    next_groups: [GroupTrack; MAX_GROUPS],
    group_count: usize,
    gate_gain: f32,
    gate_reference: f32,
    energy_gain: f32,
    resonance_enabled: bool,
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
            shifted: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); bins]),
            resonance_hold: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); bins]),
            region_owner: vec![-1; bins],
            plans: [PeakPlan::EMPTY; MAX_SPECTRAL_PEAKS],
            peak_groups: [-1; MAX_SPECTRAL_PEAKS],
            group_ratios: [1.0; MAX_GROUPS],
            previous_peaks: [PeakTrack::EMPTY; MAX_SPECTRAL_PEAKS],
            next_peaks: [PeakTrack::EMPTY; MAX_SPECTRAL_PEAKS],
            previous_peak_count: 0,
            previous_groups: [GroupTrack::EMPTY; MAX_GROUPS],
            next_groups: [GroupTrack::EMPTY; MAX_GROUPS],
            group_count: 0,
            gate_gain: 1.0,
            gate_reference: 0.0,
            energy_gain: 1.0,
            resonance_enabled: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.analyzer.reset();
        for channel in 0..MAX_CHANNELS {
            self.original[channel].fill(Complex32::new(0.0, 0.0));
            self.output[channel].fill(Complex32::new(0.0, 0.0));
            self.shifted[channel].fill(Complex32::new(0.0, 0.0));
            self.resonance_hold[channel].fill(Complex32::new(0.0, 0.0));
        }
        self.region_owner.fill(-1);
        self.plans.fill(PeakPlan::EMPTY);
        self.peak_groups.fill(-1);
        self.previous_peaks.fill(PeakTrack::EMPTY);
        self.next_peaks.fill(PeakTrack::EMPTY);
        self.previous_peak_count = 0;
        self.previous_groups.fill(GroupTrack::EMPTY);
        self.next_groups.fill(GroupTrack::EMPTY);
        self.group_count = 0;
        self.gate_gain = 1.0;
        self.gate_reference = 0.0;
        self.energy_gain = 1.0;
        self.resonance_enabled = false;
    }

    pub(crate) fn analyzer(&self) -> &SpectralAnalyzer {
        &self.analyzer
    }

    pub(crate) fn clear_resonance(&mut self) {
        if !self.resonance_enabled {
            return;
        }
        for channel in &mut self.resonance_hold {
            channel.fill(Complex32::new(0.0, 0.0));
        }
        self.resonance_enabled = false;
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

    pub(crate) fn process(
        &mut self,
        frame: &mut SpectralFrame<'_>,
        target_mask: u16,
        morph: f32,
        gate: f32,
        resonance: f32,
    ) {
        self.analyzer.analyze(frame);
        let bins = frame.positive_bins();
        let frame_energy = self.analyzer.magnitudes()[..bins]
            .iter()
            .map(|value| value * value)
            .sum::<f32>();
        for channel in 0..MAX_CHANNELS {
            self.original[channel].copy_from_slice(frame.channel(channel));
            self.output[channel].fill(Complex32::new(0.0, 0.0));
            self.shifted[channel].fill(Complex32::new(0.0, 0.0));
        }

        if frame_energy <= SILENCE_ENERGY {
            self.clear_tracking();
            if resonance > 1e-5 {
                self.render_resonance_only(frame, resonance);
            } else {
                for channel in 0..MAX_CHANNELS {
                    frame.channel_mut(channel).fill(Complex32::new(0.0, 0.0));
                }
            }
            return;
        }

        let mask = target_mask & 0x0fff;
        if mask == 0 || mask == 0x0fff {
            self.copy_identity(frame);
            self.clear_tracking();
            if resonance <= 1e-5 && self.resonance_enabled {
                self.clear_resonance();
            }
            return;
        }

        let peak_count = self.analyzer.peaks().len().min(MAX_SPECTRAL_PEAKS);
        if peak_count == 0 {
            self.copy_identity(frame);
            self.clear_tracking();
            return;
        }

        self.build_regions(peak_count, bins);
        self.build_harmonic_groups(peak_count, mask);
        self.build_peak_plans(peak_count, mask);
        self.render_regions(peak_count, bins);

        let gate_gain = self.update_gate(frame_energy.sqrt(), gate);
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                self.output[channel][bin] += self.shifted[channel][bin] * gate_gain;
            }
        }

        self.apply_energy_compensation(bins);
        self.apply_transient_morph(bins, morph);
        if resonance > 1e-5 {
            self.apply_resonance(bins, resonance);
        } else if self.resonance_enabled {
            self.clear_resonance();
        }

        for channel in 0..MAX_CHANNELS {
            frame
                .channel_mut(channel)
                .copy_from_slice(&self.output[channel]);
        }
        self.commit_tracking(peak_count);
    }

    fn copy_identity(&mut self, frame: &mut SpectralFrame<'_>) {
        for channel in 0..MAX_CHANNELS {
            frame
                .channel_mut(channel)
                .copy_from_slice(&self.original[channel]);
        }
    }

    fn clear_tracking(&mut self) {
        self.previous_peaks.fill(PeakTrack::EMPTY);
        self.previous_peak_count = 0;
        self.previous_groups.fill(GroupTrack::EMPTY);
        self.group_count = 0;
    }

    fn build_regions(&mut self, peak_count: usize, bins: usize) {
        self.region_owner.fill(-1);
        let peaks = self.analyzer.peaks();
        let magnitudes = self.analyzer.magnitudes();
        let mut first_start = peaks[0].bin;
        while first_start > 0 && magnitudes[first_start - 1] <= magnitudes[first_start] {
            first_start -= 1;
        }
        let mut left_boundary = first_start;
        for peak_index in 0..peak_count {
            let peak_bin = peaks[peak_index].bin;
            let right_boundary = if peak_index + 1 < peak_count {
                let next = peaks[peak_index + 1].bin;
                let mut valley = peak_bin;
                let mut valley_magnitude = magnitudes[peak_bin];
                for bin in peak_bin..=next.min(bins - 1) {
                    if magnitudes[bin] < valley_magnitude {
                        valley = bin;
                        valley_magnitude = magnitudes[bin];
                    }
                }
                valley
            } else {
                let mut valley = peak_bin;
                while valley + 1 < bins && magnitudes[valley + 1] <= magnitudes[valley] {
                    valley += 1;
                }
                valley
            };
            let start = left_boundary.min(peak_bin);
            let end = right_boundary.max(peak_bin).min(bins - 1);
            self.plans[peak_index] = PeakPlan {
                source_bin: peak_bin,
                region_start: start,
                region_end: end,
                source_frequency: peaks[peak_index].frequency_hz,
                ..PeakPlan::EMPTY
            };
            for owner in &mut self.region_owner[start..=end] {
                *owner = peak_index as i16;
            }
            left_boundary = end.saturating_add(1).min(bins - 1);
        }
    }

    fn build_harmonic_groups(&mut self, peak_count: usize, target_mask: u16) {
        self.peak_groups.fill(-1);
        self.next_groups.fill(GroupTrack::EMPTY);
        self.group_ratios.fill(1.0);
        self.group_count = 0;

        // Seed from prior fundamentals first so a sustained family does not
        // change identity just because two partial magnitudes cross.
        for previous in self.previous_groups {
            if !previous.active || self.group_count == MAX_GROUPS {
                continue;
            }
            if let Some(fundamental) =
                self.refine_supported_fundamental(previous.fundamental, peak_count, true)
            {
                self.install_group(fundamental, peak_count, target_mask, Some(previous));
            }
        }

        while self.group_count < MAX_GROUPS {
            let Some(strongest) = self.strongest_unassigned_peak(peak_count) else {
                break;
            };
            let peak = self.analyzer.peaks()[strongest];
            let mut best_fundamental = 0.0;
            let mut best_score = 0.0;
            let mut best_members = 0;
            for harmonic in 1..=4 {
                let candidate = peak.frequency_hz / harmonic as f32;
                let (score, members, refined) = self.score_fundamental(candidate, peak_count, true);
                if members > best_members || (members == best_members && score > best_score) {
                    best_fundamental = refined;
                    best_score = score;
                    best_members = members;
                }
            }
            if best_members < 2 || best_score < peak.magnitude * 1.1 {
                // Leave it for the individual-peak path and prevent this peak
                // from repeatedly becoming the next group seed.
                self.peak_groups[strongest] = -2;
                continue;
            }
            self.install_group(best_fundamental, peak_count, target_mask, None);
        }

        for assignment in &mut self.peak_groups[..peak_count] {
            if *assignment == -2 {
                *assignment = -1;
            }
        }
    }

    fn strongest_unassigned_peak(&self, peak_count: usize) -> Option<usize> {
        let mut strongest = None;
        let mut strength = 0.0;
        for index in 0..peak_count {
            let peak = self.analyzer.peaks()[index];
            if self.peak_groups[index] == -1 && peak.magnitude > strength {
                strongest = Some(index);
                strength = peak.magnitude;
            }
        }
        strongest
    }

    fn refine_supported_fundamental(
        &self,
        fundamental: f32,
        peak_count: usize,
        only_unassigned: bool,
    ) -> Option<f32> {
        let (_, members, refined) =
            self.score_fundamental(fundamental, peak_count, only_unassigned);
        (members >= 2).then_some(refined)
    }

    fn score_fundamental(
        &self,
        fundamental: f32,
        peak_count: usize,
        only_unassigned: bool,
    ) -> (f32, usize, f32) {
        if fundamental < MIN_MAPPABLE_HZ || fundamental > 4_000.0 {
            return (0.0, 0, fundamental);
        }
        let mut score = 0.0;
        let mut members = 0;
        let mut weighted_fundamental = 0.0;
        let mut total_weight = 0.0;
        for index in 0..peak_count {
            if only_unassigned && self.peak_groups[index] != -1 {
                continue;
            }
            let peak = self.analyzer.peaks()[index];
            let harmonic = (peak.frequency_hz / fundamental).round().max(1.0);
            if harmonic > 32.0 {
                continue;
            }
            let expected = fundamental * harmonic;
            let cents = cents_distance(peak.frequency_hz, expected);
            let tolerance = (40.0 + harmonic * 1.25).min(65.0);
            if cents <= tolerance {
                let weight = peak.magnitude / harmonic.sqrt();
                score += weight;
                total_weight += weight;
                weighted_fundamental += peak.frequency_hz / harmonic * weight;
                members += 1;
            }
        }
        let refined = if total_weight > 1e-12 {
            weighted_fundamental / total_weight
        } else {
            fundamental
        };
        (score, members, refined)
    }

    fn install_group(
        &mut self,
        fundamental: f32,
        peak_count: usize,
        target_mask: u16,
        seeded: Option<GroupTrack>,
    ) {
        if self.group_count == MAX_GROUPS {
            return;
        }
        let group = self.group_count;
        let desired =
            constrained_ratio(fundamental, nearest_allowed_ratio(fundamental, target_mask));
        let previous = seeded.or_else(|| {
            self.previous_groups
                .iter()
                .copied()
                .filter(|item| item.active)
                .min_by(|left, right| {
                    cents_distance(fundamental, left.fundamental)
                        .total_cmp(&cents_distance(fundamental, right.fundamental))
                })
                .filter(|item| cents_distance(fundamental, item.fundamental) <= GROUP_TRACK_CENTS)
        });
        let alpha = ratio_smoothing_alpha(self.hop_size, self.sample_rate);
        let ratio = previous
            .map(|item| item.ratio + (desired - item.ratio) * alpha)
            .unwrap_or(desired);
        self.group_ratios[group] = ratio;
        self.next_groups[group] = GroupTrack {
            fundamental,
            ratio: self.group_ratios[group],
            active: true,
        };

        for index in 0..peak_count {
            if self.peak_groups[index] != -1 {
                continue;
            }
            let peak = self.analyzer.peaks()[index];
            let harmonic = (peak.frequency_hz / fundamental).round().max(1.0);
            if harmonic > 32.0 {
                continue;
            }
            let tolerance = (40.0 + harmonic * 1.25).min(65.0);
            if cents_distance(peak.frequency_hz, fundamental * harmonic) <= tolerance {
                self.peak_groups[index] = group as i8;
            }
        }
        self.group_count += 1;
    }

    fn build_peak_plans(&mut self, peak_count: usize, target_mask: u16) {
        self.next_peaks.fill(PeakTrack::EMPTY);
        let mut previous_used = [false; MAX_SPECTRAL_PEAKS];
        let bin_hz = self.sample_rate / self.fft_size as f32;
        let alpha = ratio_smoothing_alpha(self.hop_size, self.sample_rate);

        for index in 0..peak_count {
            let source_frequency = self.analyzer.peaks()[index].frequency_hz;
            let group = self.peak_groups[index];
            let desired = if group >= 0 {
                self.group_ratios[group as usize]
            } else {
                constrained_ratio(
                    source_frequency,
                    nearest_allowed_ratio(source_frequency, target_mask),
                )
            };
            let previous = self.match_previous_peak(source_frequency, &mut previous_used);
            let ratio = if group >= 0 {
                desired
            } else if let Some(track) = previous {
                track.ratio + (desired - track.ratio) * alpha
            } else {
                desired
            };
            let target_frequency = source_frequency * ratio;
            let delta_bins = ((target_frequency - source_frequency) / bin_hz).round() as isize;
            let source_bin = self.plans[index].source_bin;
            let mut phase = [0.0; MAX_CHANNELS];
            for channel in 0..MAX_CHANNELS {
                phase[channel] = if let Some(track) = previous {
                    wrap_phase(
                        track.synthesis_phase[channel]
                            + 2.0 * std::f32::consts::PI * target_frequency * self.hop_size as f32
                                / self.sample_rate,
                    )
                } else {
                    self.original[channel][source_bin].arg()
                };
            }
            self.plans[index].source_frequency = source_frequency;
            self.plans[index].ratio = ratio;
            self.plans[index].target_frequency = target_frequency;
            self.plans[index].delta_bins = delta_bins;
            self.plans[index].synthesis_phase = phase;
            self.next_peaks[index] = PeakTrack {
                frequency: source_frequency,
                ratio,
                synthesis_phase: phase,
                active: true,
            };
        }
    }

    fn match_previous_peak(
        &self,
        frequency: f32,
        used: &mut [bool; MAX_SPECTRAL_PEAKS],
    ) -> Option<PeakTrack> {
        let mut best = None;
        let mut best_cents = PEAK_TRACK_CENTS;
        for index in 0..self.previous_peak_count {
            let track = self.previous_peaks[index];
            if used[index] || !track.active {
                continue;
            }
            let cents = cents_distance(frequency, track.frequency);
            if cents <= best_cents {
                best = Some(index);
                best_cents = cents;
            }
        }
        best.map(|index| {
            used[index] = true;
            self.previous_peaks[index]
        })
    }

    fn render_regions(&mut self, peak_count: usize, bins: usize) {
        for bin in 0..bins {
            let owner = self.region_owner[bin];
            if owner < 0 || owner as usize >= peak_count {
                for channel in 0..MAX_CHANNELS {
                    self.output[channel][bin] += self.original[channel][bin];
                }
                continue;
            }
            let plan = self.plans[owner as usize];
            if (plan.ratio - 1.0).abs() <= 1e-6 {
                for channel in 0..MAX_CHANNELS {
                    self.output[channel][bin] += self.original[channel][bin];
                }
                continue;
            }
            let destination = bin as isize + plan.delta_bins;
            if destination < 0 || destination >= bins as isize {
                for channel in 0..MAX_CHANNELS {
                    self.output[channel][bin] += self.original[channel][bin];
                }
                continue;
            }
            let destination = destination as usize;
            for channel in 0..MAX_CHANNELS {
                let relative = wrap_phase(
                    self.original[channel][bin].arg()
                        - self.original[channel][plan.source_bin].arg(),
                );
                let phase = wrap_phase(plan.synthesis_phase[channel] + relative);
                self.shifted[channel][destination] +=
                    Complex32::from_polar(self.original[channel][bin].norm(), phase);
            }
        }
    }

    fn update_gate(&mut self, dry_envelope: f32, amount: f32) -> f32 {
        let amount = amount.clamp(0.0, 1.0);
        if amount <= 1e-5 {
            self.gate_gain = 1.0;
            self.gate_reference = dry_envelope;
            return 1.0;
        }
        let release = (-(self.hop_size as f32) / (self.sample_rate * 0.12)).exp();
        self.gate_reference =
            self.gate_reference.max(dry_envelope) * release + dry_envelope * (1.0 - release);
        let threshold = self.gate_reference * (0.003 + amount * 0.35) + 1e-12;
        let ratio = (dry_envelope / threshold).clamp(0.0, 1.0);
        let target = ratio * ratio * (3.0 - 2.0 * ratio);
        let time = if target > self.gate_gain {
            0.003
        } else {
            0.085
        };
        let coefficient = 1.0 - (-(self.hop_size as f32) / (self.sample_rate * time)).exp();
        self.gate_gain += (target - self.gate_gain) * coefficient;
        self.gate_gain
    }

    fn apply_energy_compensation(&mut self, bins: usize) {
        let mut input_energy = 0.0_f32;
        let mut output_energy = 0.0_f32;
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                input_energy += self.original[channel][bin].norm_sqr();
                output_energy += self.output[channel][bin].norm_sqr();
            }
        }
        if input_energy <= 1e-12 || output_energy <= 1e-12 {
            return;
        }
        let target = (input_energy / output_energy)
            .sqrt()
            .clamp(0.707_945_76, 1.412_537_6);
        self.energy_gain += (target - self.energy_gain) * 0.2;
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                self.output[channel][bin] *= self.energy_gain;
            }
        }
    }

    fn apply_transient_morph(&mut self, bins: usize, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        if amount <= 1e-5 {
            return;
        }
        let flux = self.analyzer.transient().spectral_flux;
        let transient = smoothstep(0.10, 0.25, flux);
        let blend = 1.0 - transient * amount;
        if blend >= 0.999_99 {
            return;
        }
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                self.output[channel][bin] = self.original[channel][bin]
                    + (self.output[channel][bin] - self.original[channel][bin]) * blend;
            }
        }
    }

    fn apply_resonance(&mut self, bins: usize, amount: f32) {
        self.resonance_enabled = true;
        let amount = amount.clamp(0.0, 1.0);
        let decay = (0.96 + amount * 0.0385).min(0.9985);
        let boost = amount * 0.35;
        for channel in 0..MAX_CHANNELS {
            for bin in 0..bins {
                let current = self.shifted[channel][bin];
                let decayed = self.resonance_hold[channel][bin] * decay;
                self.resonance_hold[channel][bin] = if current.norm_sqr() >= decayed.norm_sqr() {
                    current
                } else {
                    decayed
                };
                self.output[channel][bin] += self.resonance_hold[channel][bin] * boost;
            }
        }
    }

    fn render_resonance_only(&mut self, frame: &mut SpectralFrame<'_>, amount: f32) {
        let bins = frame.positive_bins();
        self.output
            .iter_mut()
            .for_each(|channel| channel.fill(Complex32::new(0.0, 0.0)));
        self.shifted
            .iter_mut()
            .for_each(|channel| channel.fill(Complex32::new(0.0, 0.0)));
        self.apply_resonance(bins, amount);
        for channel in 0..MAX_CHANNELS {
            frame
                .channel_mut(channel)
                .copy_from_slice(&self.output[channel]);
        }
    }

    fn commit_tracking(&mut self, peak_count: usize) {
        self.previous_peaks[..peak_count].copy_from_slice(&self.next_peaks[..peak_count]);
        self.previous_peaks[peak_count..].fill(PeakTrack::EMPTY);
        self.previous_peak_count = peak_count;
        self.previous_groups = self.next_groups;
    }

    #[cfg(test)]
    fn build_peak_map_for_test(&mut self, target_mask: u16) {
        let count = self.analyzer.peaks().len();
        for (index, peak) in self.analyzer.peaks().iter().enumerate() {
            self.plans[index].source_bin = peak.bin;
            self.plans[index].source_frequency = peak.frequency_hz;
        }
        self.build_harmonic_groups(count, target_mask);
    }
}

fn nearest_allowed_ratio(frequency_hz: f32, target_mask: u16) -> f32 {
    if frequency_hz <= 0.0 || target_mask == 0 {
        return 1.0;
    }
    let pitch = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    let center = pitch.round() as i32;
    let mut best_distance = f32::MAX;
    let mut best_pitch = pitch;
    for candidate in center - 12..=center + 12 {
        if target_mask & (1 << candidate.rem_euclid(12)) == 0 {
            continue;
        }
        let distance = (candidate as f32 - pitch).abs();
        if distance < best_distance {
            best_distance = distance;
            best_pitch = candidate as f32;
        }
    }
    2.0_f32.powf((best_pitch - pitch) / 12.0)
}

fn constrained_ratio(frequency_hz: f32, ratio: f32) -> f32 {
    let cents = cents_from_ratio(ratio).abs();
    if frequency_hz < MIN_MAPPABLE_HZ || cents < 5.0 || cents > MAX_SHIFT_CENTS {
        1.0
    } else {
        ratio
    }
}

#[inline]
fn cents_from_ratio(ratio: f32) -> f32 {
    1200.0 * ratio.max(1e-12).log2()
}

#[inline]
fn cents_distance(left: f32, right: f32) -> f32 {
    if left <= 0.0 || right <= 0.0 {
        f32::MAX
    } else {
        cents_from_ratio(left / right).abs()
    }
}

#[inline]
fn ratio_smoothing_alpha(hop_size: usize, sample_rate: f32) -> f32 {
    1.0 - (-(hop_size as f32) / (sample_rate * RATIO_SMOOTH_SECONDS)).exp()
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ratio_obeys_deadband_and_shift_limit() {
        let c_only = 1 << 0;
        assert_eq!(
            constrained_ratio(261.625_55, nearest_allowed_ratio(261.625_55, c_only)),
            1.0
        );
        let near_c = constrained_ratio(264.0, nearest_allowed_ratio(264.0, c_only));
        assert!((264.0 * near_c - 261.625_55).abs() < 0.05);
        assert_eq!(
            constrained_ratio(440.0, nearest_allowed_ratio(440.0, c_only)),
            1.0
        );
        let a_ratio = constrained_ratio(445.0, nearest_allowed_ratio(445.0, 1 << 9));
        assert!((445.0 * a_ratio - 440.0).abs() < 0.05, "ratio={a_ratio}");
    }

    #[test]
    fn harmonic_family_receives_one_shared_ratio() {
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
                magnitude: 0.7,
                prominence: 0.9,
            },
            SpectralPeak {
                bin: 18,
                frequency_hz: 831.547_9,
                magnitude: 0.45,
                prominence: 0.85,
            },
        ]);
        processor.build_peak_map_for_test(1 << 0);
        assert_eq!(processor.group_count, 1);
        assert_eq!(processor.peak_groups[0], processor.peak_groups[1]);
        assert_eq!(processor.peak_groups[1], processor.peak_groups[2]);
        let ratio = processor.group_ratios[0];
        assert!((277.182_65 * ratio - 261.625_55).abs() < 0.1);
    }
}
