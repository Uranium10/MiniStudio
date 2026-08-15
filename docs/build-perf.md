# Build performance

Measured on Windows 11 with Rust 1.97.1 (MSVC), Cargo 1.97.1, and the external
target directory used by `run.bat`:

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\ministudio-msvc-target'
```

The clean-build numbers include `cargo clean`; they naturally vary with Windows
Defender, filesystem cache state, and CPU temperature. The edit/rebuild number
is the primary development metric.

## Results

| Stage | Clean build | One DSP file rebuild | Test build (`--no-run`) | `cargo check` |
|---|---:|---:|---:|---:|
| Baseline (`incremental = false`, MSVC linker) | 92.02 s | 28.61 s | 14.53 s | 47.88 s |
| Incremental restored | 109.15 s | 28.56 s | 26.98 s | — |
| `rust-lld` | 102.09 s | 5.20 s | 13.07 s | — |
| DSP source split | pure move; covered by final measurement | pure move; covered by final measurement | 47/50 full tests passed, 3 hardware tests ignored | passed |
| `ministudio-dsp` workspace crate | 99.92 s | **5.50 s** | **8.65 s** | 41.87 s |
| Full layered workspace (final) | **84.40 s** | **5.44 s** | **8.94 s** full workspace | **3.12 s** warm workspace |

The application edit/rebuild cycle improved from **28.61 s to 5.50 s**
(approximately 80.8%) and is below the 10-second target. A one-file DSP test
rebuild with `cargo test -p ministudio-dsp --no-run` now takes **1.13 s** warm.
Layer-specific application rebuilds measure 5.44 s after a DSP edit, 4.85 s
after a plug-in host edit, 4.61 s after an audio-engine edit, and 4.23 s after
an app-only edit.

Incremental caching alone did not help the original monolithic app crate. The
large improvement came from using `rust-lld`; the independent DSP crate then
made focused DSP tests much cheaper and removed Tauri from that test path.

## Cargo timing hotspots

Baseline top ten units from Cargo's timing report:

| Unit | Duration |
|---|---:|
| windows 0.61.3 | 44.57 s |
| tauri-utils 2.9.3 | 19.70 s |
| windows 0.54.0 | 19.53 s |
| ministudio 0.2.0 | 14.70 s |
| tauri-utils 2.9.3 (second unit) | 14.42 s |
| tokio 1.53.1 | 12.55 s |
| syn 2.0.119 | 11.13 s |
| tauri 2.11.5 | 8.55 s |
| rustfft 6.4.1 | 8.37 s |
| regex-automata 0.4.18 | 8.13 s |

Final top ten units after the complete workspace split:

| Unit | Duration |
|---|---:|
| windows 0.61.3 | 38.82 s |
| tauri-utils 2.9.3 | 20.01 s |
| windows 0.54.0 | 17.99 s |
| ministudio-dsp 0.1.0 | 14.95 s |
| tauri-utils 2.9.3 (second unit) | 14.02 s |
| vst3-host 0.9.0 | 11.08 s |
| syn 2.0.119 | 9.17 s |
| ministudio-app 0.2.0 | 7.88 s |
| windows-sys 0.61.2 | 7.44 s |
| tauri 2.11.5 | 7.28 s |

The reports are generated under
`C:\tmp\ministudio-msvc-target\cargo-timings`. They are machine-local build
artifacts and are not committed.

## Implemented changes

- Restored incremental compilation for `dev` and `test`. `dev-dsp` deliberately
  remains non-incremental: after the workspace split its warm rebuild benefit is
  negligible, while reusing per-CGU LLVM objects with `rust-lld` reproduced an
  undefined `anon.*.llvm.*` link failure.
- Configured `rust-lld` only for `x86_64-pc-windows-msvc` in
  `src-tauri/.cargo/config.toml`. Supplying `-fuse-ld=lld` to `rust-lld` was
  intentionally omitted because rust-lld reports it as an unknown, ignored
  argument; selecting `rust-lld.exe` is sufficient.
- Split the 4,981-line DSP source into common helpers and per-effect files;
  the app-level `audio/dsp/mod.rs` is now a small factory facade.
- Added the Tauri-free `ministudio-dsp` workspace crate. Its tests no longer
  compile or link the GUI stack.
- Added `ministudio-contracts` for serializable graph/IPC data and moved the
  realtime `Instrument` trait into `ministudio-dsp`.
- Isolated VST3/CLAP discovery and hosting in `ministudio-plugin`.
- Isolated CPAL, scheduling, graph, decoding, MIDI, and export code in
  `ministudio-audio`.
- Renamed the root Tauri package to `ministudio-app`; it stays at `src-tauri`
  because Tauri resolves its build script, manifest, configuration, and
  capabilities there.
- Kept release `lto = "thin"` and `codegen-units = 1` unchanged.
- Did not add sccache because the primary rebuild target is already met.

The dependency direction is now strictly
`ministudio-app -> ministudio-audio -> ministudio-plugin -> ministudio-dsp`,
with `ministudio-contracts` shared below those layers. DSP and plug-in crates do
not depend on CPAL, Symphonia, or Tauri; the audio crate does not depend on
Tauri.

## Dependency audit

`cargo tree --duplicates` found version splits dominated by transitive Tauri,
Windows, CPAL, and MIDI requirements. There is no safe direct version override
in this workspace.

All configured Symphonia features remain intentional. Import filters expose
WAV, MP3, FLAC, OGG, M4A, and AAC, so removing `wav`, `mp3`, `flac`, `ogg`,
`vorbis`, `isomp4`, or `aac` would remove an advertised format. Unsupported
files continue to fail in the decoder rather than silently disappearing from
the browser.

The measured frontend production build was 12.55 s total; Vite's bundle phase
was 5.45 s. TypeScript already stores build info under `node_modules/.tmp`, so
no speculative frontend configuration was added.

## Cache recovery

If an old cache still reports LNK2019/LNK1120 or an undefined
`anon.*.llvm.*` symbol, first close running MiniStudio processes, verify the
Defender/indexer exclusions in [dev-setup.md](dev-setup.md), then clear the
affected cache once. The normal `dev` and `test` profiles remain incremental;
`dev-dsp` is intentionally non-incremental to prevent this class of mixed-CGU
failure:

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\ministudio-msvc-target'
cargo clean --manifest-path src-tauri/Cargo.toml
```
