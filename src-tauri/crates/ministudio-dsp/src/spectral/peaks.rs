use super::{analysis::SpectralPeak, wrap_phase};

#[allow(clippy::too_many_arguments)]
pub(crate) fn detect_spectral_peaks(
    magnitudes: &[f32],
    phases: &[f32],
    previous_phases: &[f32],
    sample_rate: f32,
    fft_size: usize,
    hop_size: usize,
    phase_initialized: bool,
    output: &mut [SpectralPeak],
) -> usize {
    output.fill(SpectralPeak::EMPTY);
    let maximum = magnitudes.iter().copied().fold(0.0_f32, f32::max);
    let mean = magnitudes.iter().copied().sum::<f32>() / magnitudes.len().max(1) as f32;
    // -60 dB relative to the frame maximum, with a linked-spectrum dynamic
    // floor that keeps broadband material from filling all 96 peak slots.
    let threshold = (maximum * 1e-3).max(mean * 0.25).max(1e-10);
    let mut count = 0;

    for bin in 2..magnitudes.len().saturating_sub(2) {
        let center = magnitudes[bin];
        if center < threshold
            || center <= magnitudes[bin - 1]
            || center <= magnitudes[bin + 1]
            || center <= magnitudes[bin - 2]
            || center <= magnitudes[bin + 2]
        {
            continue;
        }

        let left = magnitudes[bin - 1].max(1e-20).ln();
        let middle = center.max(1e-20).ln();
        let right = magnitudes[bin + 1].max(1e-20).ln();
        let denominator = left - 2.0 * middle + right;
        let interpolation = if denominator.abs() > 1e-12 {
            (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let interpolated_bin = bin as f32 + interpolation;

        let phase_bin = if phase_initialized {
            let expected =
                2.0 * std::f32::consts::PI * bin as f32 * hop_size as f32 / fft_size as f32;
            let deviation = wrap_phase(phases[bin] - previous_phases[bin] - expected);
            bin as f32
                + deviation * fft_size as f32 / (2.0 * std::f32::consts::PI * hop_size as f32)
        } else {
            interpolated_bin
        };
        // Phase advance is highly precise on stationary partials. During an
        // onset it can be ambiguous, so reject estimates that leave the local
        // peak region and fall back to log-parabolic interpolation.
        let true_bin = if (phase_bin - bin as f32).abs() <= 1.5 {
            phase_bin
        } else {
            interpolated_bin
        };
        let shoulder = 0.25
            * (magnitudes[bin - 2]
                + magnitudes[bin - 1]
                + magnitudes[bin + 1]
                + magnitudes[bin + 2]);
        let prominence = ((center - shoulder) / (center + 1e-20)).clamp(0.0, 1.0);
        let candidate = SpectralPeak {
            bin,
            frequency_hz: (true_bin * sample_rate / fft_size as f32).clamp(0.0, sample_rate * 0.5),
            magnitude: center,
            prominence,
        };

        // Detection scans bins in ascending order, so the retained array stays
        // frequency-sorted without a post-pass sort. Once full, discard only
        // the weakest retained peak and append this later-bin candidate.
        if count < output.len() {
            output[count] = candidate;
            count += 1;
        } else {
            let mut weakest = 0;
            for index in 1..count {
                if output[index].magnitude < output[weakest].magnitude {
                    weakest = index;
                }
            }
            if candidate.magnitude > output[weakest].magnitude {
                for index in weakest..count - 1 {
                    output[index] = output[index + 1];
                }
                output[count - 1] = candidate;
            }
        }
    }
    count
}
