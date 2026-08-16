# Performance Tier P1 entry audit

Date: 2026-08-16  
Branch: `stabilize/core-runtime-realignment-20260816`

## Entry decision

The tempo/source-layer work is already present and its immutable `TempoMap` and resolved
`AudioSourceRef` boundaries are retained. P1 therefore starts beside the existing graph rather
than moving project/source resolution into the callback.

The pre-P1 audit found two callback-call-graph hazards that the original source-text guards did
not cover:

- `AudioGraph::process` used stable `sort_by_key` for the per-track event buffer every block;
- meter publication called `active_voice_counts()`, which collected a new `Vec` every block.

The event sort is now in-place `sort_unstable_by_key`, its bounded buffer cannot grow while live
events are merged, and voice counts are written into the existing fixed meter array. A test-only
thread-local allocation tracker executes the complete `AudioCore::render -> AudioGraph::process ->
native instrument/effect -> meter publish` path and requires zero allocations and zero
deallocations. The cheap source scan remains as a second guard and now covers the graph process
body too.

## Ownership and dependency constraints

- Scheduler metadata is created while the replacement graph is built on the control plane.
- The callback mutates only fixed per-node runtime state and preallocated activity arrays.
- Sleeping never removes a node from topology or latency calculation.
- Project schema, target IDs, plug-in state, `TempoMap`, and `AudioSourceRef` are unchanged.
- The scheduler has no Tauri, window, VST3, CLAP, file-system, or project-model dependency.

## Known migration gaps retained deliberately

### Embedded-editor TLS compatibility registry

`src-tauri/src/lib.rs` still contains the experimental `EMBEDDED_PLUGIN_EDITORS` and
`PINNED_PLUGIN_EDITORS` thread-local compatibility registries. Removing these belongs to core
runtime migration gate 6, after editor-session parity has been demonstrated. P1 does not add a
dependency on them and does not change Tauri/editor lifetime code.

### CLAP process isolation

CLAP audio remains in-process. Its adapter participates in the common runtime capability contract,
but reports conservative `AlwaysProcess` until out-of-process realtime transport and CLAP
process-status/tail semantics are qualified. P1 must not describe CLAP sleep as complete merely
because the common type exists.

### External plug-in sleep

VST3 tail metadata remains available for export, but neither VST3 nor CLAP is host-slept by
default. Unknown/vendor behavior, noise generators, free-running processors and transport-sensitive
plug-ins remain audible and correct at the cost of continued processing. A future fingerprint
compatibility database can opt qualified modules into Auto without changing the scheduler.

## Baseline method

`p1-benchmark` runs the same Release binary twice per fixture. Scheduler OFF is the legacy
all-process baseline; Scheduler ON is the candidate. This avoids comparing different builds or
machines. Scenarios cover Empty, Silent FX 100/500, Active FX, Tail Heavy, Sidechain and Live.

See `docs/performance-benchmark.md` for the command, environment and captured result.

