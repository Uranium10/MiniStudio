# VST3 and CLAP hosting

MiniDAW now has a first working external plug-in host path for VST3 and CLAP effects and instruments.

## Current path

- The device picker scans the standard system VST3 and CLAP folders. Up to four binaries are inspected in parallel disposable MiniDAW probe processes with a 15-second per-binary timeout, so a crashing or hanging scanner does not take down the main application or stall the scan indefinitely. Results are cached for the application session.
- Project data stores the format, class/plugin ID, display metadata, and binary path. VST3 and CLAP devices therefore use the same ordered effect chain as built-in DSP.
- VST3 audio buses are enumerated when the plug-in is loaded. Every declared input and output bus is activated when possible; the chain signal feeds the main input and a selected MiniDAW track or return feeds every auxiliary input bus. VST3 instruments receive sample-offset MIDI note, controller, pressure, pitch-bend, and all-notes-off events.
- CLAP audio ports are enumerated through `CLAP_EXT_AUDIO_PORTS`. The main input receives the chain signal and non-main inputs receive the selected external-sidechain signal. CLAP instruments receive timestamped MIDI events. Event, port, and audio storage is reserved when the instance is created and reused by the callback.
- VST3 latency and tail values feed the existing graph latency/tail contract.

## External sidechain routing

- Sidechain-capable built-in effects and scanned VST3/CLAP effects expose the same sidechain picker in the device rack.
- A track, return bus, or master insert can select any track or return bus as its detector/auxiliary-input source. Self-routes and transitive cycles are rejected before the native graph is rebuilt.
- Source taps are post-track/post-return gain and use the previous realtime block. This fixed one-block delay keeps routing deterministic and prevents feedback inside a callback.
- VST3 auxiliary buses are activated before processing. CLAP ports preserve their declared bus and channel layout. Mono auxiliary ports receive a stereo average; stereo ports receive left/right.

## Standard folders

- Windows: `C:\Program Files\Common Files\VST3`, `C:\Program Files\Common Files\CLAP`, and per-user `LocalAppData\Programs\Common` variants.
- macOS: `/Library/Audio/Plug-Ins/{VST3,CLAP}` and the matching user Library folders.
- Linux: `~/.vst3`, `~/.clap`, `/usr/lib/{vst3,clap}`, and `/usr/local/lib/{vst3,clap}`.

## Deliberate first-release limits

- MiniDAW's effect chain still consumes the plug-in's main output as stereo. Additional output buses are negotiated and processed, but independent stem routing for those outputs is a later mixer feature.
- Plug-in-specific native editor windows, parameter enumeration/UI, automation, and opaque state/preset persistence are separate follow-up work. External devices currently run their saved/default state and MiniDAW's generic card identifies the loaded binary.
- CLAP latency/tail extensions are not yet included in graph compensation.
- A native plug-in can still crash while it is actively processing. Discovery is process-isolated; realtime sandboxing is not yet implemented.

These limits should be treated as compatibility work, not as an ABI or project-model redesign: format, ID, path, effect-chain ordering, and the realtime adapter boundary are already persistent.
