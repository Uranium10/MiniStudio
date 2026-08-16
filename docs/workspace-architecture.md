# Rust workspace architecture

The native backend is split by rebuild and ownership boundaries:

```text
ministudio-app (src-tauri root; Tauri IPC and application lifecycle)
  -> ministudio-audio (engine, graph, scheduling, devices, MIDI, asset IO)
       -> ministudio-plugin (VST3/CLAP scan and realtime adapters)
            -> ministudio-dsp (native effects and realtime contracts)

ministudio-contracts (serializable domain/IPC structures)
  <- shared by app, audio, plugin, and DSP where required
```

## Boundary rules

- `ministudio-contracts` contains data only. It must not depend on audio,
  plug-in, DSP, or Tauri implementations.
- `ministudio-dsp` may depend on math/FFT and contracts, but not CPAL,
  Symphonia, VST3/CLAP, or Tauri.
- Reusable transform mechanics belong to `ministudio-dsp/src/spectral`.
  Effect files own musical policy and may not duplicate FFT planning, WOLA ring
  scheduling, reconstruction normalization, phase utility, linked peak/HPCP,
  transient-mask, or harmonic-family mapping code.
- `ministudio-plugin` owns VST3/CLAP dependencies and implements the DSP and
  instrument contracts. It must not depend on the audio engine or Tauri.
- `ministudio-audio` owns realtime control, CPAL streams, graph scheduling,
  decode/export, and MIDI device IO. It must not depend on Tauri.
- `ministudio-app` owns Tauri commands, channels, application state, and the
  out-of-process plug-in probe orchestration.

The app crate intentionally remains at the `src-tauri` root. Moving it under
`crates/` would separate it cosmetically but break Tauri's conventional lookup
of `build.rs`, `tauri.conf.json`, capabilities, and the package manifest.

## Focused commands

```powershell
cargo check --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path src-tauri/Cargo.toml -p ministudio-dsp
cargo test --manifest-path src-tauri/Cargo.toml -p ministudio-audio
cargo build --manifest-path src-tauri/Cargo.toml -p ministudio-app
```
