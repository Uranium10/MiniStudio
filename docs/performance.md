# Performance notes

MiniStudio keeps high-frequency UI state and realtime audio controls off the full
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
- Extended timelines cap each backing canvas at 30,000 horizontal pixels while
  retaining the full CSS/time coordinate space, avoiding browser canvas-size
  failures and unbounded per-track bitmap allocation at extreme zoom levels.
- Clip-gain and fade edits redraw waveform amplitude at most once per animation
  frame. Only visible waveform columns are traversed; gain nodes are sorted once
  per clip, sampled with a forward cursor, and converted through a 0.1 dB lookup
  table instead of repeated exponentiation.
- Arrangement scrolling uses one shared listener. Vertical-only movement stays
  entirely on the compositor; track canvases are invalidated only when the
  horizontal time window changes, avoiding the former listener/redraw fan-out
  across every track.
- The piano roll uses separate grid, note, gesture, and velocity Canvas layers.
  Sorted notes enter the visible range through binary search; drag previews stay
  transient, and multi-note move/resize/quantize operations commit as one batch
  instead of rebuilding the graph once per note.

## Native development profile

`run.bat` uses Cargo's `dev-dsp` profile. The MiniStudio crate is compiled with
`opt-level = 2`, while dependency crates stay at `opt-level = 0` to keep the
large Tauri dependency graph cheaper to rebuild. The target cache lives in
`C:\tmp\ministudio-msvc-target` to avoid Desktop indexer and antivirus file locks.
The regular `dev` and `test` profiles retain Cargo incremental compilation and
disable dependency debug symbols. The realtime `dev-dsp` profile is
non-incremental because reusing per-CGU LLVM objects with `rust-lld` can produce
stale anonymous-symbol references after an interrupted or invalidated build.
The layered workspace and fast linker retain short warm rebuilds, and the
external target directory remains fully disposable.

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
cargo build --manifest-path src-tauri/Cargo.toml --profile dev-dsp --bin ministudio
```

`src/store/projectStore.performance.test.ts` locks down waveform sharing,
continuous-edit history coalescing, and transient playhead behavior.
`src/components/PianoRoll.performance.test.ts` performs 10,000 viewport lookups
over 5,000 sorted notes and enforces a conservative 250 ms regression ceiling.

## Performance Tier P1 entry baseline

Before scheduler/silence propagation work, the Windows VST3 helper must preserve realtime audio
while a native vendor editor is created or moved. The helper therefore separates its GUI-only
controller/view capability from processor ownership, uses bounded lock-free editor feedback, treats
single deadline misses as diagnostics, and coalesces terminal recovery requests.

The entry baseline was verified on 2026-08-16 at 48 kHz / 256 frames:

- Serum: native editor creation overlapped 192 realtime blocks; output stayed non-silent and the
  endpoint reported zero deadline misses.
- BBC Symphony Orchestra: native editor creation overlapped 96 realtime blocks; the endpoint
  reported zero deadline misses and the custom window pin/close lifecycle passed.
- `cargo test --workspace`: passed.
- frontend: 17 files / 98 tests passed and the production Vite build completed.
- `dev-dsp` MiniStudio application build: passed.

P1 may introduce scheduler-owned Running/Tail/Sleeping states. It must not revive the old
working-set-trim loop: a helper may reclaim resources only after the graph has put its node into a
real Sleeping state and preserved PDC, wake events and plug-in lifetime.

## P1 preflight usability boundary

- Piano-roll creation and keyboard audition use one note lifecycle: held gestures release on pointer
  up with a 360 ms minimum, while one-shot move/listen previews use 360 ms and teardown still sends
  immediate Note Off.
- Downstream DSP/VST parameter and bypass edits no longer alter the graph signature, avoiding graph
  rebuilds and silent blocks during ordinary rack work.
- VST3 editor feedback is bounded and rides the existing realtime completion rather than adding a
  control poll or plug-in mutex. Project updates are coalesced by target and covered by store tests.
- Serum editor/audio overlap and BBC editor/topmost/unpin/close passed with installed plug-ins after
  the ownership changes; both used the optimized `dev-dsp` executable.
