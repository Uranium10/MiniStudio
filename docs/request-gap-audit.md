# Request gap audit

This audit separates confirmed gaps from work that is already present. It is
based on the current source tree, the Tier 0 readiness notes, and the stored
implementation documents.

## Confirmed gaps

| Area | Current evidence | Next work |
|---|---|---|
| Realtime plug-in crash containment | Scanning is isolated, but an external plug-in still runs in the realtime host process. | Design processing isolation without putting IPC or allocation on the audio callback. |
| External plug-in editor/state | Plug-in parameter metadata has a generic host UI, but plug-in-specific editor windows and opaque preset/state persistence remain explicitly deferred. | Add VST3/CLAP editor lifecycle and state save/restore. |
| Colorizer MIDI control | The device UI explicitly reports `MIDI ROUTING NOT AVAILABLE YET`. | Route track/clip MIDI into the effect and expose the source/mode controls described by the readiness patch. |
| Automation completion | Playback is UI control-rate. Write/Touch/Latch and sample-offset parameter queues are documented as future work. | Move playback scheduling native-side, implement write modes, and verify offline export parity. |
| Audio recording | REC currently records virtual-piano MIDI only; there is no input-stream/audio-file recording path. | Add input device negotiation, direct-to-file capture, monitoring, and crash-safe take recovery after autosave exists. |
| External single-effect offline API | `RustEngine.renderOffline()` is still an explicit phase-three stub. | Implement it when isolated external plug-in rendering is introduced. |

## Already implemented (not missing)

- Piano-roll subscription/layer performance has a focused regression test.
- `dsp/mod.rs` has already been split into `common/` and per-effect modules.
- VST3/CLAP discovery is already out-of-process, up to four probes in parallel,
  with a 15-second timeout per binary.
- Native project export uses the same graph/DSP path; the `renderOffline()` stub
  above is the separate external single-plug-in interface.
- EQ defaults and hidden disabled points are covered by store tests.
- Autosave/crash recovery, Distortion oversampling and band bypass, and the
  complete native latency/PDC audit were completed in Tier 0.

## Partial or lower-priority follow-ups

- CLAP latency/tail extensions are not yet included in graph compensation.
- Additional plug-in output buses are processed but not independently routed as
  mixer stems.
- Return-bus metering and engine CPU-load telemetry still need native data if
  those roadmap diagnostics are promoted into the active milestone.
- Metronome/count-in, audio-input recording, and modulation tracks remain
  roadmap work rather than completed production features.
