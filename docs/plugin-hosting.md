# VST3 and CLAP hosting

This document is the binding architecture contract for MiniStudio's external
plug-in host. Code that contradicts an invariant below is a host bug, even if
it happens to work with one vendor plug-in.

## Production invariants

1. **One control owner for one instance.** VST3 component/controller creation,
   initialization, state restore, parameter/controller synchronization, GUI
   calls and final destruction run on the instance's message-pumped owner
   thread. CLAP lifecycle, main-thread callbacks and GUI calls obey the same
   ownership rule. Audio processing is the only realtime operation.
2. **A graph edit does not recreate an unchanged plug-in.** Native instances
   live in `ExternalPluginRegistry`, keyed by stable project target ID plus
   format/path/class ID/sample rate. Replacement graphs receive lock-free
   processing proxies. Removed instances remain alive through the retired
   graph and are destroyed only after the callback has crossed the swap.
3. **No vendor call under the global engine mutex.** IPC clones a
   `PluginControl`, releases the engine lock, and only then opens, saves or
   loads a vendor plug-in.
4. **No lock, allocation, file/network I/O or UI call in the audio callback.**
   Proxy sharing is single-writer: only the active graph dereferences a
   processor and swaps occur between callbacks.
5. **Failure is data, not absence.** External factory errors propagate with the
   target ID and format. They must never become `None` and later surface as the
   unrelated “instance is not available” error.
6. **Discovery is never in-process.** A scanner crash, modal license prompt or
   hang cannot take down the DAW. Cache entries are portable metadata keyed by
   binary fingerprint, never a project hardcoded to one machine's path.

These constraints follow the format contracts rather than vendor-specific
exceptions:

- <https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/API%2BDocumentation/Index.html>
- <https://github.com/free-audio/clap/blob/main/include/clap/plugin.h>
- <https://github.com/free-audio/clap/blob/main/include/clap/ext/gui.h>
- <https://github.com/free-audio/clap/blob/main/include/clap/ext/thread-check.h>

## Commercial-host baseline

MiniStudio uses documented host behavior as the baseline; closed DAW source
code is not guessed at. Studio One scans in an external process and caches
subsequent scans; Cubase exposes normal/full rescan, blocked plug-ins and a
plug-in report; REAPER offers automatic/separate/dedicated process modes;
Bitwig offers shared, per-vendor, per-plug-in and per-instance isolation plus
selective crash recovery.

- <https://support.presonus.com/hc/en-us/articles/360045185092-What-is-the-difference-between-skip-and-disable-in-the-new-plug-in-scan-and-how-do-I-manage-my-plug-ins-now>
- <https://www.steinberg.help/r/cubase-pro/15.0/en/cubase_nuendo/topics/installing_and_managing_plugins/installing_and_managing_plugins_vst_plug_in_manager_toolbar_r.html>
- <https://www.reaper.fm/userguide/ReaperUserGuide681c.pdf>
- <https://www.bitwig.com/userguide/latest/vst_plug_in_handling_and_options/>

## Current path

- The device picker scans the standard system VST3 and CLAP folders. Up to four binaries are inspected in parallel disposable MiniStudio probe processes with a 15-second per-binary timeout, so a crashing or hanging scanner does not take down the main application or stall the scan indefinitely. Results are cached persistently per user by bundle size/mtime fingerprint; normal startup probes only new or changed binaries and an explicit full rescan ignores fingerprints.
- Project data stores the format, class/plugin ID, display metadata, and binary path. VST3 and CLAP devices therefore use the same ordered effect chain as built-in DSP.
- VST3 audio buses are enumerated when the plug-in is loaded. Every declared input and output bus is activated when possible; the chain signal feeds the main input and a selected MiniStudio track or return feeds every auxiliary input bus. VST3 instruments receive sample-offset MIDI note, controller, pressure, pitch-bend, and all-notes-off events.
- CLAP audio ports are enumerated through `CLAP_EXT_AUDIO_PORTS`. The main input receives the chain signal and non-main inputs receive the selected external-sidechain signal. CLAP instruments receive timestamped MIDI events. Event, port, and audio storage is reserved when the instance is created and reused by the callback.
- VST3 latency and tail values feed the existing graph latency/tail contract.
- VST3 initialization and native standalone editor operations share one
  message-pumped owner thread. The production path does not force a COM
  apartment on vendor code. The old WebView-child embedding experiment is
  disabled by default because its cross-thread parent-window lifetime is not a
  safe cross-vendor contract.
- Unchanged external instances survive structural graph rebuilds. Adding BBC
  Symphony Orchestra does not destroy/recreate an already loaded Serum
  instance or its editor.
- MiniStudio can re-execute its own signed binary with the private
  `--ministudio-vst3-host` argument. This avoids a separately located helper
  executable and keeps plug-in creation, state, native GUI message dispatch,
  editor teardown and final destruction on the Windows helper process main
  thread. The application checks this mode before Tauri/WebView/audio startup.
- Windows VST3 instances use the self-hosted process server by default.
  `MINISTUDIO_VST3_PROCESS_ISOLATION=0` is an explicit diagnostic escape hatch.
  CLAP and non-Windows realtime isolation remain separate follow-up work.

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

- MiniStudio's effect chain still consumes the plug-in's main output as stereo. Additional output buses are negotiated and processed, but independent stem routing for those outputs is a later mixer feature.
- Native standalone editor windows, parameter enumeration, automation and
  opaque state persistence are implemented. A host-chrome embedded editor is
  still experimental; compatibility takes priority over a toolbar frame.
- CLAP latency/tail extensions are not yet included in graph compensation.
- CLAP and non-Windows native plug-ins can still crash the application while
  actively processing. Windows VST3 processing is isolated; the remaining
  formats/platforms must reach the same boundary before release.

These limits should be treated as compatibility work, not as an ABI or project-model redesign: format, ID, path, effect-chain ordering, and the realtime adapter boundary are already persistent.

## Process-host milestone (required before release)

JSON stdin/stdout is now restricted to control/state operations. Windows VST3
realtime playback uses:

- a real process main thread for format lifecycle, callbacks and native GUI;
- a dedicated audio worker in that process;
- one versioned, preallocated shared-memory block per instance for up to 32
  buses, 64 channels, 4096 frames, 1024 timestamped MIDI events and 1024
  sample-accurate parameter events;
- auto-reset Windows request/response events, sequence validation and bounded
  5-50 ms block deadlines, with no JSON, pipe I/O or allocation in the host
  callback;
- an explicit realtime `Idle -> Requested -> Processing -> Done` state machine,
  sequence IDs, timeout latching and safe mapping reattachment after recovery;
- a higher-level heartbeat plus `Loading`, `Ready`, `EditorOpening`, `Running`,
  `Closing`, `Crashed` and `Recovering` states (still pending);
- selectable Together / By manufacturer / By plug-in / Individually isolation,
  with an individual override for heavy or unstable plug-ins;
- cached state checkpoints so a crashed instance can be reloaded without
  restarting the project.

Timeouts are watchdog diagnostics, not lifecycle state transitions. A slow
sample library may remain `Loading` while the DAW stays responsive; it is not
silently discarded merely because a fixed UI timer elapsed.

### Verified Windows control-plane baseline

The self-hosted helper has been physically tested against the locally
installed commercial plug-ins, rather than only a mock protocol:

- BBC Symphony Orchestra 1.12.1 loaded, created its 1083x917 native editor,
  closed it and exited cleanly.
- Serum remained open in one helper while BBC loaded and opened in another.
  Both processes stayed alive concurrently and both editors closed with exit
  code 0.

The realtime path was also physically rendered through Serum: repeated runs of
96 blocks of 256 frames completed in about 10-11 ms total (roughly 0.11 ms per
block) with a non-silent 0.124861 RMS result. A forced helper termination then recovered to a
new PID, reattached the same mapping and rendered audio again. This verifies
the Windows VST3 process boundary end to end, including timestamped MIDI and
audio return.
