use super::analysis::{SpectralPeak, HPCP_BINS};

pub(crate) struct Hpcp {
    values: [f32; HPCP_BINS],
}

impl Hpcp {
    pub(crate) const fn new() -> Self {
        Self {
            values: [0.0; HPCP_BINS],
        }
    }

    pub(crate) fn clear(&mut self) {
        self.values.fill(0.0);
    }

    pub(crate) fn analyze(&mut self, peaks: &[SpectralPeak], tuning_hz: f32) {
        self.clear();
        for peak in peaks {
            if !(40.0..=6_000.0).contains(&peak.frequency_hz) || peak.prominence < 0.02 {
                continue;
            }
            let pitch = 69.0 + 12.0 * (peak.frequency_hz / tuning_hz).log2();
            let position = pitch.rem_euclid(12.0) * (HPCP_BINS as f32 / 12.0);
            let center = position.round() as i32;
            // Squared-cosine contribution within one semitone, matching the
            // robust pitch-class weighting used by established HPCP systems.
            for offset in -3..=3 {
                let target = center + offset;
                let distance = (position - target as f32).abs() / 3.0;
                if distance > 1.0 {
                    continue;
                }
                let weight = (std::f32::consts::FRAC_PI_2 * distance).cos().powi(2);
                let index = target.rem_euclid(HPCP_BINS as i32) as usize;
                self.values[index] += peak.magnitude.sqrt() * peak.prominence * weight;
            }
        }
        let maximum = self.values.iter().copied().fold(0.0_f32, f32::max);
        if maximum > 1e-12 {
            for value in &mut self.values {
                *value /= maximum;
            }
        }
    }

    pub(crate) fn values(&self) -> &[f32; HPCP_BINS] {
        &self.values
    }

    pub(crate) fn strongest_pitch_class(&self) -> (usize, f32) {
        let mut class_energy = [0.0_f32; 12];
        for (index, value) in self.values.iter().enumerate() {
            class_energy[index / 3] += *value;
        }
        let mut strongest = 0;
        for index in 1..12 {
            if class_energy[index] > class_energy[strongest] {
                strongest = index;
            }
        }
        let total = class_energy.iter().sum::<f32>().max(1e-12);
        (strongest, (class_energy[strongest] / total * 3.0).min(1.0))
    }
}
