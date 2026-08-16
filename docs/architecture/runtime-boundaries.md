# Core runtime boundaries

This document is the dependency and migration contract for audio, MIDI and external plug-ins.

## Crate direction

```text
ministudio-plugin-api
        ^              ^
        |              |
ministudio-plugin-runtime    ministudio-plugin-helper
        ^              ^
        |              |
   Tauri control      format adapters (VST3 / CLAP)
                           ^
                           |
                    realtime endpoint
                           ^
                           |
                    ministudio-audio
```

- `ministudio-plugin-api` contains IDs, generations, capabilities, intents/events, MIDI packets and
  metric snapshots. It cannot depend on Tauri, windowing or a plug-in SDK.
- `ministudio-plugin-runtime` is the authoritative lifecycle state machine. Platform code executes
  its intents but cannot invent generations or pin truth.
- `ministudio-plugin-helper` owns format-neutral placement and compatibility policy. Its cache key is
  `(format, architecture, binary fingerprint)`, never an absolute installation path or vendor name.
- Format adapters own VST3/CLAP SDK objects and translate only at the edge.
- `ministudio-audio` sees preallocated realtime processors/endpoints, not windows, Tauri or SDK
  lifecycle objects.

## Thread and channel rules

| Plane | May block? | Allocation | Transport |
|---|---:|---:|---|
| Audio callback | No | No | bounded SPSC/shared memory + OS event signal |
| Plug-in realtime worker | bounded deadline only | preallocated | shared memory request/response |
| Control actor | Yes, off callback | Yes | typed bounded commands/events |
| Native GUI thread | vendor/platform rules | Yes | posted lifecycle intents and push events |
| WebView | never owns native resources | Yes | Tauri commands + pushed snapshots/events |

GUI unresponsiveness changes editor health only. A realtime timeout changes realtime health and wakes
control recovery. Neither path may infer the other without direct evidence.

## Lifecycle invariants

1. Every asynchronous editor message carries `(instance_id, generation)`.
2. Closing or retiring an instance fences that generation before native teardown begins.
3. A late open/resize/toolbar response from a stale generation is discarded.
4. Pin is acknowledged only after the native platform applies topmost/floating state.
5. Unpinned replacement closes asynchronously and never holds the audio endpoint.
6. Helper failure retires all leases owned by that helper; recovery attaches a fresh control process
   to the existing realtime mapping only from the control plane.

## Compatibility policy

Dedicated helper placement is the stabilization default. Module grouping is eligible only after a
multiplexed helper protocol passes parity tests, admits at most four instances, and uses a binary
fingerprint failure domain. Any crash, hang or sustained realtime deadline stall promotes that
fingerprint to dedicated placement in a relative user-data cache. An isolated deadline miss remains
diagnostic data. A changed fingerprint receives a fresh probation period; no manufacturer-specific
branch belongs in the engine.

## ARA/source-layer compatibility

Future ARA integration enters through a separate `SourceRuntime` adapter. It may publish immutable
source snapshots and offline render work to the control plane, but it must not add model mutation,
file access or SDK callbacks to the audio callback and must not become part of plug-in editor
lifecycle ownership.
