# MiniStudio core runtime audit

Status: gates 1-4 partially landed on `stabilize/core-runtime-realignment-20260816`; gates 5-6
remain intentionally blocked on cross-platform helper parity and legacy removal tests.

## Confirmed failure chain

The native editor bridge polled editor actions every 100 ms. For an isolated VST3 this
eventually called `take_editor_host_actions()` while holding the same outer
`Arc<Mutex<Plugin>>` that guarded the audio adapter. The audio adapter used `try_lock()` and
returned without processing when control owned that mutex, producing a silent block.

Dragging a native editor makes the platform GUI thread enter a modal move/resize loop. The
synchronous editor-action request then waits for that thread while retaining the outer plug-in
mutex. This turns a control-plane delay into continuous audio silence. The MIDI symptom and the
window-move symptom therefore have the same ownership violation.

Pin state was also duplicated between the WebView bridge, Tauri state, thread-local state and
the helper chrome. None of those stores was an authoritative editor-session state machine, and
the helper did not consistently apply the platform topmost transition.

## Required ownership

| Domain | Sole owner | Communication |
|---|---|---|
| Realtime plug-in processing | audio graph adapter | preallocated realtime endpoint |
| Plug-in lifecycle and state | plug-in supervisor | typed bounded intents/events |
| Native editor and z-order | helper platform UI thread | asynchronous editor intents/events |
| MIDI devices and routing | MIDI service | timestamped bounded packets |
| UI-visible state | immutable runtime snapshot | push events / snapshot reads |

## Forbidden dependencies

- The audio callback must not acquire a plug-in lifecycle or editor mutex.
- Native editor movement, painting or modal UI must not gate realtime processing.
- Tauri/WebView code must not own native editor lifetime or pin truth.
- Plug-in format types must not escape their VST3 or CLAP adapter.
- A control timeout must not be interpreted as an audio failure.
- No callback path may allocate, log, access files, or perform synchronous control IPC.

## Existing seams retained

- Stable external plug-in instances across graph rebuilds.
- Bounded audio command and hardware MIDI queues.
- Triple-buffered meter publication.
- The Windows VST3 shared-memory realtime transport.
- Existing project schema, target IDs, TempoMap and plug-in state blobs.

## Migration gates

1. Common API and supervisor state-machine tests.
2. Realtime endpoint ownership separated from VST3 control ownership.
3. Bounded callback command work and runtime metrics.
4. Event-driven editor lifecycle with one pin authority.
5. VST3 and CLAP helpers behind the same contract on Windows, macOS and Linux.
6. Legacy polling and compatibility wrappers removed only after parity and performance gates.

## Landed in this migration

- Added format-neutral `ministudio-plugin-api`, supervisor state machine and helper placement
  policy crates. Project schema, target IDs, state blobs and `TempoMap` remain unchanged.
- Detached the Windows VST3 `RealtimeClient` from plug-in control ownership. Native editor/state
  calls and audio processing no longer share the outer plug-in mutex.
- Replaced editor-action and realtime-fault timer polls with bounded shared memory plus independent
  Windows events. The owner actor sleeps on channel/event work while idle.
- Added generation-fenced editor open/close/pin state. Old generations cannot resurrect a closed or
  replaced editor; Windows pin is committed only after the helper acknowledges applying
  `HWND_TOPMOST`/`HWND_NOTOPMOST` at the native window.
- Added bounded callback command/MIDI drains, WebView MIDI batching and allocation-free realtime
  callback metrics exposed in `StreamStatus`.
- Converted the CLAP main-thread callback owner from an unconditional 8 ms timer to event-driven,
  atomically coalesced `request_callback` wakeups. A full bounded control channel cannot lose the
  pending callback. CLAP audio is structurally separate but remains in-process.
- Added dedicated-first placement, a four-instance module-group policy and path-independent
  compatibility records. Actual group multiplexing remains disabled until its helper protocol is
  implemented and qualified.
- Added architecture guard tests plus physical Serum/BBC tests for rendering, editor close actions,
  helper recovery and a 500 ms control-lock stall.

## Gates not yet satisfied

- CLAP out-of-process realtime transport and macOS/Linux native helper/event adapters.
- Actual multiplexed module-group helper execution and automatic placement wiring.
- Complete removal of the experimental embedded-editor TLS compatibility registry.
- Replacement of the short-call `NativeEngine` mutex with a control actor/runtime snapshot.
- OS hot-plug notifications, shared-clock `MidiPacket` timestamps and immutable selected-track
  routing snapshots. `MidiService` ownership and route-transition `AllNotesOff` are landed.
- MMCSS/real-time scheduler setup, three-OS CI, ten-minute soak and 60-second window-move reports.
