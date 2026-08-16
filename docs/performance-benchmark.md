# MiniStudio performance benchmark

## Reproduction

```powershell
cargo run --manifest-path src-tauri\Cargo.toml --release -p ministudio-audio --bin p1-benchmark
```

The fixture uses 48 kHz, 256-sample blocks, 256 warm-up blocks and 4,096 measured blocks. It is a
DSP scheduling microbenchmark, not a CPAL/device benchmark: xrun and callback percentiles remain
available through `StreamStatus` for physical-device soak tests.

## Environment

- OS: Microsoft Windows NT 10.0.26200.0, x86-64
- CPU identifier: AMD64 Family 23 Model 113 Stepping 0, AuthenticAMD
- Rust: 1.97.1, LLVM 22.1.6, `x86_64-pc-windows-msvc`
- Build: Rust `release`, thin LTO, one codegen unit
- Sample rate: 48,000 Hz
- Block size: 256 samples (5.333 ms deadline)

## Results

| Scenario | Scheduler | ns/block | Deadline % | Sleeping/Total | Cumulative skips |
|---|---:|---:|---:|---:|---:|
| Empty | OFF | 837 | 0.016 | 0/0 | 0 |
| Empty | ON | 1,134 | 0.021 | 0/0 | 0 |
| Silent FX 100 | OFF | 246,198 | 4.616 | 0/100 | 0 |
| Silent FX 100 | ON | 5,418 | 0.102 | 100/100 | 435,100 |
| Silent FX 500 | OFF | 1,198,945 | 22.480 | 0/500 | 0 |
| Silent FX 500 | ON | 13,806 | 0.259 | 500/500 | 2,175,500 |
| Active FX 100 | OFF | 253,830 | 4.759 | 0/101 | 0 |
| Active FX 100 | ON | 251,737 | 4.720 | 0/101 | 0 |
| Tail Heavy | OFF | 563,902 | 10.573 | 0/41 | 0 |
| Tail Heavy | ON | 560,999 | 10.519 | 0/41 | 0 |
| Sidechain | OFF | 70,658 | 1.325 | 0/19 | 0 |
| Sidechain | ON | 62,360 | 1.169 | 1/19 | 4,294 |
| Live | OFF | 91,636 | 1.718 | 0/33 | 0 |
| Live | ON | 94,064 | 1.764 | 0/33 | 0 |

Silent FX 100 reduces measured block time by about 97.8%; Silent FX 500 by about 98.8%. Active,
tail-heavy and live fixtures remain within ordinary run-to-run variance because their nodes are
correctly kept awake. The empty-graph difference is hundreds of nanoseconds and is not treated as
a meaningful regression.

## Interpretation

- Scheduler OFF is the pre-P1 all-process baseline in the same binary.
- Scheduler ON may skip only explicitly qualified native nodes.
- Active, unknown, generator, VST3 and CLAP nodes remain conservative.
- Absolute wall-clock figures vary by CPU and background load; compare ON/OFF on the same machine.
