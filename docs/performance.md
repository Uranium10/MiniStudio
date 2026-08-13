# Performance notes

MiniDAW keeps high-frequency UI state and realtime audio controls off the full
project rebuild path.

## Frontend and state

- The playhead is transient Zustand state, so 30 Hz engine polling does not
  replace the persistent project object or invalidate the whole workspace.
- Undo snapshots structurally share immutable audio assets and waveform
  `Float32Array` buffers. Continuous edits coalesce into one history entry.
- Mixer, send, clip, and effect edits replace only the affected branch. Other
  tracks keep referential identity, allowing memoized channel and timeline
  components to skip work.
- Full native graph synchronization is reserved for structural changes. Fader,
  pan, mute, solo, send, bus, master, and effect parameter changes use
  coalesced realtime IPC commands. Store-level diffing also keeps undo/redo and
  project loading synchronized with the native engine.
- All level meters share one 30 Hz animation scheduler and update DOM styles
  directly instead of scheduling React renders per meter.
- Timeline canvases cap device pixel ratio at 1.5, redraw only the visible
  horizontal region, cull off-screen clips, and coalesce scroll drawing to one
  animation frame per lane.
- The piano roll uses separate grid, note, gesture, and velocity Canvas layers.
  Sorted notes enter the visible range through binary search; drag previews stay
  transient, and multi-note move/resize/quantize operations commit as one batch
  instead of rebuilding the graph once per note.

## Native development profile

`run.bat` uses Cargo's `dev-dsp` profile. The MiniDAW crate is compiled with
`opt-level = 2`, while dependency crates stay at `opt-level = 0` to keep the
large Tauri dependency graph cheaper to rebuild. The target cache lives in
`C:\tmp\minidaw-msvc-target` to avoid Desktop indexer and antivirus file locks.
The regular `dev` and `test` profiles disable Cargo incremental compilation and
dependency debug symbols. This trades a little warm-check latency for a much
smaller and more stable cache; the realtime `dev-dsp` profile was already
non-incremental. The external target directory remains fully disposable.

The first build of a new profile is intentionally expensive. On the reference
Windows machine used on 2026-08-12:

- first `dev-dsp` cache build: 6m 23s;
- first Tauri-specific `cfg(dev)` build: 2m 05s;
- warm Cargo rebuild: 20.79s;
- warm `run.bat` to both visible window and HTTP 200: 13.94s.

These are local diagnostic measurements, not general performance guarantees.

## Native DSP hot paths

- EQ and Disperser filters traverse audio stage-first, keeping recursive state
  and coefficients cache-local across each block.
- Compressor attack/release coefficients are calculated once per block. The
  normal stereo-linked path also shares one detector envelope and publishes its
  gain-reduction meter once per block instead of evaluating duplicate
  exponentials and logarithms for every sample.
- Reverb precomputes FDN decay gains and advances its modulation with normalized
  recursive oscillators. This removes per-sample `powf` and per-line `sin`
  calls while retaining smoothly modulated fractional delay reads.
- Delay and reverb use branch-based single-wrap ring indices instead of integer
  division in their sample loops. The cubic reader remains sample-equivalent to
  the wrapped reference path over its supported delay range.
- Waveshaper and 3-band Distortion share a sparse half-band reconstruction FIR,
  calculate drive compensation once per frame, and fast-forward parameter
  smoothers when a distortion band is Off.
- Spectrum analyzers precompute the Hann window and logarithmic display-bin
  positions. Multiband compression folds stereo metering into the output
  recombination pass rather than scanning all three bands again.

## Regression checks

Run:

```text
npm run build
npm run lint
npm test
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib
cargo build --manifest-path src-tauri/Cargo.toml --profile dev-dsp --bin minidaw
```

`src/store/projectStore.performance.test.ts` locks down waveform sharing,
continuous-edit history coalescing, and transient playhead behavior.
`src/components/PianoRoll.performance.test.ts` performs 10,000 viewport lookups
over 5,000 sorted notes and enforces a conservative 250 ms regression ceiling.
