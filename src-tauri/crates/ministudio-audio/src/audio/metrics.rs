//! Allocation-free realtime telemetry.
//!
//! The callback only increments atomics. Percentiles are calculated by the control plane when a
//! snapshot is requested, so observing performance cannot consume the audio deadline.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ministudio_plugin_api::RuntimeMetricsSnapshot;

const HISTOGRAM_BUCKETS: usize = 32;

pub struct RealtimeMetrics {
    callbacks: AtomicU64,
    callback_histogram: [AtomicU64; HISTOGRAM_BUCKETS],
    callback_max_ns: AtomicU64,
    command_high_water: AtomicU32,
    command_overflow: AtomicU64,
    plugin_deadline_misses: AtomicU64,
}

impl Default for RealtimeMetrics {
    fn default() -> Self {
        Self {
            callbacks: AtomicU64::new(0),
            callback_histogram: std::array::from_fn(|_| AtomicU64::new(0)),
            callback_max_ns: AtomicU64::new(0),
            command_high_water: AtomicU32::new(0),
            command_overflow: AtomicU64::new(0),
            plugin_deadline_misses: AtomicU64::new(0),
        }
    }
}

impl RealtimeMetrics {
    #[inline]
    pub fn observe_callback(&self, elapsed_ns: u64) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.callback_histogram[histogram_bucket(elapsed_ns)].fetch_add(1, Ordering::Relaxed);
        self.callback_max_ns
            .fetch_max(elapsed_ns, Ordering::Relaxed);
    }

    #[inline]
    pub fn observe_command_depth(&self, depth: usize) {
        self.command_high_water
            .fetch_max(depth.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    #[inline]
    pub fn command_overflow(&self) {
        self.command_overflow.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn plugin_deadline_miss(&self) {
        self.plugin_deadline_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self, xruns: u64) -> RuntimeMetricsSnapshot {
        let callbacks = self.callbacks.load(Ordering::Relaxed);
        let buckets =
            std::array::from_fn(|index| self.callback_histogram[index].load(Ordering::Relaxed));
        RuntimeMetricsSnapshot {
            callbacks,
            callback_p50_ns: percentile(&buckets, callbacks, 50),
            callback_p95_ns: percentile(&buckets, callbacks, 95),
            callback_p99_ns: percentile(&buckets, callbacks, 99),
            callback_max_ns: self.callback_max_ns.load(Ordering::Relaxed),
            xruns,
            command_high_water: self.command_high_water.load(Ordering::Relaxed),
            command_overflow: self.command_overflow.load(Ordering::Relaxed),
            plugin_deadline_misses: self.plugin_deadline_misses.load(Ordering::Relaxed),
        }
    }
}

#[inline]
fn histogram_bucket(value: u64) -> usize {
    if value == 0 {
        return 0;
    }
    (u64::BITS - value.leading_zeros())
        .saturating_sub(1)
        .min((HISTOGRAM_BUCKETS - 1) as u32) as usize
}

fn percentile(buckets: &[u64; HISTOGRAM_BUCKETS], total: u64, percentile: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let target = total.saturating_mul(percentile).div_ceil(100).max(1);
    let mut accumulated = 0_u64;
    for (index, count) in buckets.iter().enumerate() {
        accumulated = accumulated.saturating_add(*count);
        if accumulated >= target {
            return if index == 0 { 1 } else { 1_u64 << index };
        }
    }
    1_u64 << (HISTOGRAM_BUCKETS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_percentiles_are_computed_off_callback() {
        let metrics = RealtimeMetrics::default();
        for value in [100, 200, 300, 400, 500, 600, 700, 8_000, 9_000, 10_000] {
            metrics.observe_callback(value);
        }
        metrics.observe_command_depth(27);
        metrics.command_overflow();
        let snapshot = metrics.snapshot(3);
        assert_eq!(snapshot.callbacks, 10);
        assert!(snapshot.callback_p50_ns <= snapshot.callback_p95_ns);
        assert!(snapshot.callback_p95_ns <= snapshot.callback_p99_ns);
        assert_eq!(snapshot.callback_max_ns, 10_000);
        assert_eq!(snapshot.command_high_water, 27);
        assert_eq!(snapshot.command_overflow, 1);
        assert_eq!(snapshot.xruns, 3);
    }
}
