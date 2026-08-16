//! Immutable, allocation-free-on-read musical time conversion.
//!
//! Maps are prepared on the control thread while an [`AudioGraph`](super::graph::AudioGraph)
//! is built. The audio thread only performs binary searches and closed-form arithmetic.

use ministudio_contracts::{TempoCurveSpec, TempoPointSpec, TimeSignaturePointSpec};

pub const MIDI_PPQ: u64 = 960;
const MIN_BPM: f64 = 1.0;
const MAX_BPM: f64 = 999.0;
const FLAT_SLOPE: f64 = 1.0e-12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempoCurve {
    Jump,
    Linear,
}

impl From<TempoCurveSpec> for TempoCurve {
    fn from(value: TempoCurveSpec) -> Self {
        match value {
            TempoCurveSpec::Jump => Self::Jump,
            TempoCurveSpec::Linear => Self::Linear,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TempoPoint {
    pub tick: u64,
    pub bpm: f64,
    pub curve: TempoCurve,
}

#[derive(Clone, Copy, Debug)]
pub struct TimeSignaturePoint {
    /// One-based bar number.
    pub bar: u32,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Clone, Debug)]
pub struct TempoMap {
    sample_rate: u32,
    tempo_points: Vec<TempoPoint>,
    time_sigs: Vec<TimeSignaturePoint>,
    /// Rounded sample position at each tempo point. Built only on the control thread.
    cumulative_samples: Vec<u64>,
    /// Tick position at the start of each time-signature segment.
    time_sig_start_ticks: Vec<u64>,
}

impl TempoMap {
    pub fn new(bpm: f64, sample_rate: u32) -> Self {
        Self::from_specs(
            &[TempoPointSpec {
                tick: 0,
                bpm,
                curve: TempoCurveSpec::Jump,
            }],
            &[TimeSignaturePointSpec {
                bar: 1,
                numerator: 4,
                denominator: 4,
            }],
            sample_rate,
        )
    }

    pub fn from_specs(
        tempo_points: &[TempoPointSpec],
        time_sigs: &[TimeSignaturePointSpec],
        sample_rate: u32,
    ) -> Self {
        let mut points: Vec<_> = tempo_points
            .iter()
            .filter(|point| point.bpm.is_finite())
            .map(|point| TempoPoint {
                tick: point.tick,
                bpm: point.bpm.clamp(MIN_BPM, MAX_BPM),
                curve: point.curve.into(),
            })
            .collect();
        points.sort_by_key(|point| point.tick);
        points.dedup_by_key(|point| point.tick);
        if points.first().is_none_or(|point| point.tick != 0) {
            points.insert(
                0,
                TempoPoint {
                    tick: 0,
                    bpm: points.first().map_or(120.0, |point| point.bpm),
                    curve: TempoCurve::Jump,
                },
            );
        }

        let mut signatures: Vec<_> = time_sigs
            .iter()
            .map(|signature| TimeSignaturePoint {
                bar: signature.bar.max(1),
                numerator: signature.numerator.clamp(1, 32),
                denominator: sanitize_denominator(signature.denominator),
            })
            .collect();
        signatures.sort_by_key(|signature| signature.bar);
        signatures.dedup_by_key(|signature| signature.bar);
        if signatures
            .first()
            .is_none_or(|signature| signature.bar != 1)
        {
            signatures.insert(
                0,
                TimeSignaturePoint {
                    bar: 1,
                    numerator: 4,
                    denominator: 4,
                },
            );
        }

        let mut map = Self {
            sample_rate: sample_rate.max(1),
            cumulative_samples: vec![0; points.len()],
            time_sig_start_ticks: vec![0; signatures.len()],
            tempo_points: points,
            time_sigs: signatures,
        };
        map.rebuild_caches();
        map
    }

    fn rebuild_caches(&mut self) {
        for index in 1..self.tempo_points.len() {
            let previous = self.tempo_points[index - 1];
            let next = self.tempo_points[index];
            let samples = self.segment_ticks_to_samples(index - 1, next.tick - previous.tick);
            self.cumulative_samples[index] =
                self.cumulative_samples[index - 1].saturating_add(samples);
        }
        for index in 1..self.time_sigs.len() {
            let previous = self.time_sigs[index - 1];
            let bars = u64::from(self.time_sigs[index].bar - previous.bar);
            self.time_sig_start_ticks[index] = self.time_sig_start_ticks[index - 1]
                .saturating_add(bars.saturating_mul(ticks_per_bar(previous)));
        }
    }

    #[inline]
    pub fn ticks_to_samples(&self, ticks: u64) -> u64 {
        let index = self.tempo_segment_for_tick(ticks);
        self.cumulative_samples[index].saturating_add(
            self.segment_ticks_to_samples(index, ticks - self.tempo_points[index].tick),
        )
    }

    #[inline]
    pub fn samples_to_ticks(&self, samples: u64) -> u64 {
        let index = self.tempo_segment_for_sample(samples);
        self.tempo_points[index].tick.saturating_add(
            self.segment_samples_to_ticks(index, samples - self.cumulative_samples[index]),
        )
    }

    #[inline]
    pub fn bpm_at_tick(&self, ticks: u64) -> f64 {
        let index = self.tempo_segment_for_tick(ticks);
        let point = self.tempo_points[index];
        match (point.curve, self.tempo_points.get(index + 1)) {
            (TempoCurve::Linear, Some(next)) if next.tick > point.tick => {
                let ratio = (ticks.saturating_sub(point.tick) as f64
                    / (next.tick - point.tick) as f64)
                    .clamp(0.0, 1.0);
                point.bpm + (next.bpm - point.bpm) * ratio
            }
            _ => point.bpm,
        }
    }

    /// First tempo or time-signature boundary strictly after `sample`.
    /// Used by the control-free render loop to keep host transport snapshots
    /// from straddling an abrupt musical-time change.
    pub fn next_change_sample_after(&self, sample: u64) -> Option<u64> {
        let tick = self.samples_to_ticks(sample);
        let tempo_index = self
            .tempo_points
            .partition_point(|point| point.tick <= tick);
        let next_tempo = self.tempo_points.get(tempo_index).map(|point| point.tick);
        let signature_index = self
            .time_sig_start_ticks
            .partition_point(|start| *start <= tick);
        let next_signature = self.time_sig_start_ticks.get(signature_index).copied();
        next_tempo
            .into_iter()
            .chain(next_signature)
            .map(|boundary| self.ticks_to_samples(boundary))
            .filter(|boundary| *boundary > sample)
            .min()
    }

    /// Returns a one-based bar and a zero-based beat offset within that bar.
    pub fn tick_to_bar_beat(&self, ticks: u64) -> (u32, f64) {
        let index = self.time_signature_segment_for_tick(ticks);
        let signature = self.time_sigs[index];
        let local = ticks - self.time_sig_start_ticks[index];
        let bar_ticks = ticks_per_bar(signature).max(1);
        let bar_offset = local / bar_ticks;
        let within = local % bar_ticks;
        let beat_ticks = ticks_per_beat(signature);
        (
            signature
                .bar
                .saturating_add(bar_offset.min(u64::from(u32::MAX)) as u32),
            within as f64 / beat_ticks as f64,
        )
    }

    pub fn bar_beat_to_tick(&self, bar: u32, beat: f64) -> u64 {
        let bar = bar.max(1);
        let index = self.time_signature_segment_for_bar(bar);
        let signature = self.time_sigs[index];
        let bar_offset = u64::from(bar - signature.bar);
        let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        self.time_sig_start_ticks[index]
            .saturating_add(bar_offset.saturating_mul(ticks_per_bar(signature)))
            .saturating_add((beat * ticks_per_beat(signature) as f64).round() as u64)
    }

    pub fn bar_start_ticks(&self, bar: u32) -> u64 {
        self.bar_beat_to_tick(bar, 0.0)
    }

    pub fn time_signature_at_tick(&self, ticks: u64) -> TimeSignaturePoint {
        self.time_sigs[self.time_signature_segment_for_tick(ticks)]
    }

    pub fn time_signature_at_bar(&self, bar: u32) -> TimeSignaturePoint {
        self.time_sigs[self.time_signature_segment_for_bar(bar.max(1))]
    }

    pub fn ticks_per_beat_at_bar(&self, bar: u32) -> u64 {
        ticks_per_beat(self.time_signature_at_bar(bar))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn tempo_segment_for_tick(&self, tick: u64) -> usize {
        self.tempo_points
            .partition_point(|point| point.tick <= tick)
            .saturating_sub(1)
    }

    fn tempo_segment_for_sample(&self, sample: u64) -> usize {
        self.cumulative_samples
            .partition_point(|position| *position <= sample)
            .saturating_sub(1)
    }

    fn time_signature_segment_for_bar(&self, bar: u32) -> usize {
        self.time_sigs
            .partition_point(|signature| signature.bar <= bar)
            .saturating_sub(1)
    }

    fn time_signature_segment_for_tick(&self, tick: u64) -> usize {
        self.time_sig_start_ticks
            .partition_point(|start| *start <= tick)
            .saturating_sub(1)
    }

    fn segment_ticks_to_samples(&self, index: usize, delta_ticks: u64) -> u64 {
        let point = self.tempo_points[index];
        let ticks = delta_ticks as f64;
        let quarter_samples = self.sample_rate as f64 * 60.0 / MIDI_PPQ as f64;
        let value = match (point.curve, self.tempo_points.get(index + 1)) {
            (TempoCurve::Linear, Some(next)) if next.tick > point.tick => {
                let slope = (next.bpm - point.bpm) / (next.tick - point.tick) as f64;
                if slope.abs() < FLAT_SLOPE {
                    quarter_samples * ticks / point.bpm
                } else {
                    let end_bpm = (point.bpm + slope * ticks).max(MIN_BPM);
                    quarter_samples * (end_bpm / point.bpm).ln() / slope
                }
            }
            _ => quarter_samples * ticks / point.bpm,
        };
        value.round().clamp(0.0, u64::MAX as f64) as u64
    }

    fn segment_samples_to_ticks(&self, index: usize, delta_samples: u64) -> u64 {
        let point = self.tempo_points[index];
        let samples = delta_samples as f64;
        let quarter_samples = self.sample_rate as f64 * 60.0 / MIDI_PPQ as f64;
        let value = match (point.curve, self.tempo_points.get(index + 1)) {
            (TempoCurve::Linear, Some(next)) if next.tick > point.tick => {
                let slope = (next.bpm - point.bpm) / (next.tick - point.tick) as f64;
                if slope.abs() < FLAT_SLOPE {
                    samples * point.bpm / quarter_samples
                } else {
                    point.bpm * (samples * slope / quarter_samples).exp_m1() / slope
                }
            }
            _ => samples * point.bpm / quarter_samples,
        };
        value.round().clamp(0.0, u64::MAX as f64) as u64
    }
}

fn sanitize_denominator(value: u8) -> u8 {
    match value {
        1 | 2 | 4 | 8 | 16 | 32 => value,
        _ => 4,
    }
}

fn ticks_per_beat(signature: TimeSignaturePoint) -> u64 {
    MIDI_PPQ * 4 / u64::from(signature.denominator)
}

fn ticks_per_bar(signature: TimeSignaturePoint) -> u64 {
    ticks_per_beat(signature) * u64::from(signature.numerator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varied() -> TempoMap {
        TempoMap::from_specs(
            &[
                TempoPointSpec {
                    tick: 0,
                    bpm: 120.0,
                    curve: TempoCurveSpec::Jump,
                },
                TempoPointSpec {
                    tick: 3_840,
                    bpm: 90.0,
                    curve: TempoCurveSpec::Linear,
                },
                TempoPointSpec {
                    tick: 11_520,
                    bpm: 180.0,
                    curve: TempoCurveSpec::Jump,
                },
            ],
            &[
                TimeSignaturePointSpec {
                    bar: 1,
                    numerator: 4,
                    denominator: 4,
                },
                TimeSignaturePointSpec {
                    bar: 5,
                    numerator: 3,
                    denominator: 4,
                },
            ],
            48_000,
        )
    }

    #[test]
    fn one_point_is_sample_exact_with_the_old_formula() {
        let map = TempoMap::new(137.5, 48_000);
        for tick in [0, 1, 240, 960, 3_840, 9_999_999] {
            let expected =
                ((tick as f64 * 60.0 * 48_000.0) / (137.5 * MIDI_PPQ as f64)).round() as u64;
            assert_eq!(map.ticks_to_samples(tick), expected);
        }
    }

    #[test]
    fn multi_segment_round_trip_stays_within_one_sample() {
        let map = varied();
        for tick in (0..200_000).step_by(137) {
            let sample = map.ticks_to_samples(tick);
            let recovered = map.samples_to_ticks(sample);
            assert!(
                map.ticks_to_samples(recovered).abs_diff(sample) <= 1,
                "tick={tick}"
            );
        }
    }

    #[test]
    fn linear_integral_matches_fine_numerical_reference() {
        let map = TempoMap::from_specs(
            &[
                TempoPointSpec {
                    tick: 0,
                    bpm: 60.0,
                    curve: TempoCurveSpec::Linear,
                },
                TempoPointSpec {
                    tick: 9_600,
                    bpm: 180.0,
                    curve: TempoCurveSpec::Jump,
                },
            ],
            &[],
            48_000,
        );
        let steps = 200_000_u64;
        let width = 9_600.0 / steps as f64;
        let mut seconds = 0.0;
        for index in 0..steps {
            let tick = (index as f64 + 0.5) * width;
            let bpm = 60.0 + 120.0 * tick / 9_600.0;
            seconds += width * 60.0 / (MIDI_PPQ as f64 * bpm);
        }
        assert!((map.ticks_to_samples(9_600) as f64 - seconds * 48_000.0).abs() <= 1.0);
    }

    #[test]
    fn signatures_change_only_on_bar_boundaries() {
        let map = varied();
        assert_eq!(map.bar_start_ticks(5), 15_360);
        assert_eq!(map.bar_start_ticks(6), 18_240);
        assert_eq!(map.tick_to_bar_beat(18_240), (6, 0.0));
        assert_eq!(map.bar_beat_to_tick(6, 2.0), 20_160);
    }

    #[test]
    fn next_change_reports_the_first_strict_transport_boundary() {
        let map = TempoMap::from_specs(
            &[
                TempoPointSpec {
                    tick: 0,
                    bpm: 120.0,
                    curve: TempoCurveSpec::Jump,
                },
                TempoPointSpec {
                    tick: 3_840,
                    bpm: 90.0,
                    curve: TempoCurveSpec::Jump,
                },
            ],
            &[
                TimeSignaturePointSpec {
                    bar: 1,
                    numerator: 4,
                    denominator: 4,
                },
                TimeSignaturePointSpec {
                    bar: 3,
                    numerator: 3,
                    denominator: 4,
                },
            ],
            48_000,
        );
        let tempo_boundary = map.ticks_to_samples(3_840);
        let signature_boundary = map.ticks_to_samples(map.bar_start_ticks(3));
        assert_eq!(map.next_change_sample_after(0), Some(tempo_boundary));
        assert_eq!(
            map.next_change_sample_after(tempo_boundary),
            Some(signature_boundary)
        );
        assert_eq!(map.next_change_sample_after(signature_boundary), None);
    }
}
