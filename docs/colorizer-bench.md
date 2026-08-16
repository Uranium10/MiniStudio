# Colorizer performance benchmark

## Reproduction

```powershell
cargo test --manifest-path src-tauri\Cargo.toml -p ministudio-dsp --release colorizer_map_bench_reports_callback_cost -- --ignored --nocapture
```

Fixture: Windows x86-64, 48 kHz, 256-frame callback, four-note polyphonic
material plus a stereo noise floor, release profile, warmed plans and rings.
The callback deadline is 5.333 ms.

| Quality | FFT / hop | Latency | Cost/callback | Deadline load |
|---|---:|---:|---:|---:|
| Fast | 512 / 128 | 512 samples | 38.691 us | 0.73% |
| Clean | 1024 / 256 | 1024 samples | 36.345 us | 0.68% |

Both are comfortably below the 3% single-core target. Fast prioritizes lower
latency, not minimum CPU: its complete analysis/mapping pass runs twice per
256-frame callback, while Clean runs once.

These are deterministic DSP microbenchmark figures, not an end-to-end audio
device callback percentile. Physical-device p99/xrun telemetry remains the
authoritative system-level gate.
