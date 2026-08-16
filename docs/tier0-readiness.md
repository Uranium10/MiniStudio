# Tier 0 completion

The build-structure prerequisite is complete. Tier 0 work can now proceed
without returning every DSP or audio edit through the Tauri app crate.

## Current source locations

| Tier 0 area | Primary location |
|---|---|
| Piano-roll render subscriptions/layers | `src/components/PianoRoll.tsx` |
| Autosave and crash recovery | frontend project store plus `ministudio-app` persistence commands |
| Distortion oversampling/band controls | `src-tauri/crates/ministudio-dsp/src/effects/distortion.rs` and device UI |
| DSP latency audit | `src-tauri/crates/ministudio-dsp/src/effects/` |
| DSP module structure | complete under `ministudio-dsp/src/common` and `effects` |
| Plug-in scan crash isolation | app probe process plus `ministudio-plugin` scanner/host |

## Completed Tier 0 work

1. Piano-roll rendering is split into grid, note/event, and realtime overlay
   passes. The playhead is read from the store inside the overlay animation and
   no longer causes React rerenders; project extent is memoized.
2. Projects are debounced into a crash-recovery snapshot. A session that did
   not close cleanly appears as a dated recovery choice in the startup helper,
   and audio assets are rehydrated from their source paths.
3. Distortion exposes 1x/2x/4x oversampling plus explicit low/mid/high enable
   switches. Its latency report follows the selected oversampling factor.
4. All 17 native effect kinds are covered by an explicit latency contract test.
   Colorizer reports 512/1024 samples for Fast/Clean harmonic mapping; the
   graph PDC regression also covers the mastering limiter's 240-sample path.
5. Out-of-process plug-in discovery is covered for missing executables and
   hung probes; the latter is killed at its deadline.

Realtime processing isolation for a plug-in that crashes after it has loaded is
a separate hosting milestone; discovery itself is isolated and bounded.
