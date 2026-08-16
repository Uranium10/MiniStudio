# MiniStudio audio runtime architecture

## Ownership model

`NativeEngine` is currently the control-side façade. It builds immutable `AudioGraph` values,
owns decoded assets and external plug-in control handles, and publishes commands to the running
audio stream. The CPAL callback owns `AudioCore` and renders only the currently installed graph.
Graph replacement is transactional: a complete graph is built off the callback, submitted through
a bounded SPSC queue, and the retired graph is returned to the control side for destruction.

External plug-in control and realtime processing are separate owners. In particular, the Windows
VST3 audio adapter owns a detached `RealtimeClient`; editor, state and lifecycle operations retain
only the control handle. A stalled native editor can therefore not acquire anything needed by the
audio callback.

## Realtime data plane

- Audio commands use a preallocated `rtrb` queue. At most 256 control commands and 512 live MIDI
  packets are drained in one callback so control traffic cannot consume an unbounded deadline.
- Meter state is a fixed-size `triple_buffer::Output<MeterFrame>` containing transport position,
  track/master levels, PDC and bounded DSP analyzer data.
- Windows VST3 audio, timestamped MIDI and sample-accurate parameters use one versioned shared
  memory mapping per instance plus request/response OS events. JSON/stdin/stdout is control-only.
- Editor actions and realtime transport faults have their own OS events. Neither requires a timer
  poll, a WebView round trip, or a plug-in lifecycle lock.
- Every callback buffer, event list and graph scratch area is allocated before streaming begins.

The callback must not use mutexes, blocking channels, synchronous control IPC, file I/O, logging,
or heap allocation. `tests/realtime_architecture.rs` rejects direct regressions in the callback and
`AudioCore::render` surfaces; this is a guardrail rather than a substitute for profiling.

## Bounded failure behavior

A VST3 transport timeout latches the realtime endpoint fault, increments a lock-free deadline
counter, produces a safe block, and signals the control actor through a dedicated fault event. The
control owner then respawns/restores the helper and reattaches the same mapping. Recovery never runs
inline in the callback. GUI latency alone is not an audio failure.

Stream errors increment an atomic xrun counter. Device initialization first tries the selected
backend/device and then the operating-system default. Failed streams are restarted from the control
side while preserving the transport snapshot when possible.

## Telemetry

`RealtimeMetrics` records callback count, a logarithmic duration histogram, maximum duration,
command queue high-water/overflow and plug-in deadline misses using relaxed atomics. Percentiles are
computed only when the control side requests an `EngineSnapshot`, never in the callback. The UI
receives p50/p95/p99/max milliseconds, xrun count and queue diagnostics through `StreamStatus`.

## Time, routing and PDC

Project time is represented as `u64` samples inside the renderer. Clip edges, fades, loop splits and
tempo boundaries are calculated at sample precision. Sends and sidechains use deterministic taps;
feedback within one callback is prevented with the documented one-block source delay. Track paths
are delayed to the longest effective latency and the maximum is published as `pdcSamples`.

## Remaining migration gate

The global `NativeEngine` control mutex still serializes short Tauri control calls. Slow editor and
MIDI operating-system calls are already cloned/prepared under that lock and executed after release,
so it is not on the audio path. It will be replaced by the planned control actor and immutable
runtime snapshot only after command parity tests exist. CLAP and non-Windows external plug-ins also
still need the same out-of-process realtime transport before the legacy compatibility layer can be
removed.
