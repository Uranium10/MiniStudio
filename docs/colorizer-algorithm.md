# Colorizer algorithm notes

Status: `Map` quality-mode extensions (GATE, QUALITY Fast/Clean, real-FFT, shift
guards) landed on top of the Codex spectral-engine rewrite recorded in
`docs/architecture/spectral-engine-colorizer-plan.md`. Read that document
first — it explains why `Colorizer` is two backends (`Live`/`Map`) instead of
one, and the product research (Xynth Chroma, Zynaptiq PITCHMAP) behind the
design. This document covers only the `Map` signal path in more implementation
detail, and the specific additions made in this pass.

## Signal path (`Map` quality, per hop)

```
dry L/R
  -> StftEngine (real FFT, sqrt-Hann WOLA)
  -> SpectralAnalyzer.analyze
       linked-stereo magnitude/phase, spectral flux -> transient mask,
       detect_spectral_peaks (log-parabolic + phase-derivative true frequency)
  -> PitchMapProcessor.process
       1. build_peak_map: group peaks into harmonic families, assign each
          family (or lone peak) a pitch ratio via nearest_allowed_ratio,
          clamped by constrained_ratio (60 Hz floor, 400-cent shift cap)
       2. per-bin scatter: residual (untouched, weighted 1-mapped) +
          mapped magnitude scattered to the shifted destination bin
       3. apply_spectral_gate: broadband dry-envelope gate on the mapped
          magnitude only (GATE)
       4. assign_mapped_phase_regions + render_phase_locked_component:
          identity-style phase locking per region, using each region's
          nearest surviving peak as the phase anchor
  -> StftEngine inverse (real FFT) -> wet
dry_delay (plain fft_size-sample line) -> `delayed` reference for mix/bypass
```

`Live` quality does not touch any of this; it stays the existing fixed modal
resonator bank.

## Real FFT (`spectral/stft.rs`)

The previous implementation fed a complex FFT a zero imaginary half and
manually rebuilt the negative-frequency bins from Hermitian symmetry before
the inverse transform. `spectral::stft` is the only consumer of this module in
the codebase (verified before changing it — `rg spectral::stft` matches only
`colorizer.rs` and `spectral/analysis.rs`), so it was safe to convert to
`realfft`'s real-to-complex/complex-to-real transform without touching any
other effect:

- forward: `size` real samples in, `size/2+1` complex bins out — exactly the
  `positive_bins` the rest of the module already assumed.
- inverse: the reverse, unconditionally real output; no manual conjugate
  fill-in is needed or possible.
- `realfft`'s inverse is unnormalized like `rustfft`'s, so the synthesis
  window's existing `1 / (size * overlap_sum)` term is unchanged.
- DC and Nyquist bins are still forced to zero imaginary part before the
  inverse call, since a processor that writes arbitrary gains into every bin
  could otherwise leave a non-real residual there.

This roughly halves the transform's arithmetic. `spectral/stft.rs`'s own
reconstruction tests (impulse/steady-state identity, positive-bin symmetry)
were extended with an explicit `(16, 4)` and `(4096, 512)` size round-trip and
all continued to pass unchanged after the swap — the `SpectralFrame` API
(`positive_bins`, `channel`, `channel_mut`) did not change shape, so
`analysis.rs` and `pitch_map.rs` needed no changes for this part.

## Shift guards (`pitch_map.rs`)

Two guards were added to `nearest_allowed_ratio`'s result before it is stored
as a peak's or fundamental's ratio (`constrained_ratio`):

- **`MIN_MAPPABLE_HZ = 60`**: pitch judgment from a single analysis window is
  unreliable this low, and pulling bass content onto the wrong octave is far
  more audible than leaving it alone. Peaks below this are always left at
  ratio 1.0, regardless of what the fundamental-family grouping would
  otherwise pick.
- **`MAX_SHIFT_CENTS = 400`**: a peak that needs to move more than this to
  reach an enabled pitch class most likely *is* a different note, not an
  out-of-tune one. Forcing it onto the grid anyway is a jump, not a
  correction, so it is left unshifted instead.

Both are applied uniformly to fundamentals and to individual peaks that
didn't join a harmonic family, so a family's shared ratio and a lone peak's
own ratio go through the identical check.

## GATE

Modeled on the documented Chroma/PITCHMAP-class "spectral gate" behavior of
shortening wet ringing using the dry signal as the key — **not** a per-bin
comparison of dry against wet magnitude at the same frequency. That distinction
matters here specifically because the whole point of `Map` mode is that the
wet content moves to a *different* bin than the dry content came from;
gating bin-for-bin against dry would gate away the correction itself, not
just its tail.

Instead, `apply_spectral_gate` tracks one broadband envelope per hop
(`gate_reference`, a slow max/decay follower on summed dry magnitude across
all bins) and derives a single gain from how far the current frame's dry
energy has fallen below that reference. That gain is applied only to
`mapped_magnitude` — the phase-locked/shifted component — before phase-region
assignment; the residual pass-through (already an exact, un-shifted copy of
dry) is left alone. Attack is fast (0.35/hop) so ringing does not linger after
a note stops; release is slow (0.05/hop) so a legitimately decaying note is
not chattered by the gate. `GATE = 0` skips the stage entirely and resets the
gain to unity so a later re-enable doesn't resume from a stale value.

## QUALITY: Fast (512) / Clean (1024)

Two complete `ColorizerMap` engines (`map_fast`, `map_clean`) are constructed
in `prepare()`, so a `QUALITY` switch at runtime is never an audio-thread
allocation — both are already fully warmed-up-capable, just idle when not
selected. `Clean` (1024/256 hop) is the default and matches the previous fixed
behavior exactly.

Switching feeds *both* engines and linearly crossfades their `mapped` output
over `MAP_QUALITY_CROSSFADE_MS` (12 ms); once the fade completes the
now-inactive engine goes idle again (no ongoing double CPU cost outside the
transition window). A second switch requested mid-crossfade is ignored until
the current one settles, rather than trying to resolve a three-way blend.

**Known limitation, shared with Formant Shifter's Mono/Poly toggle**
(`effects/formant.rs::latency_samples`): `latency_samples()` changes with
`QUALITY` through an ordinary `set_param` call, which does not itself trigger
a PDC graph rebuild (`NativeEngine::set_effect` pushes straight to the live
effect instance over the bounded command queue — see `engine.rs`). The
crossfade makes the *audible* transition click-free, but other tracks' delay
compensation is not re-aligned to the new declared latency until the graph is
otherwise rebuilt. Properly closing this gap project-wide (making a
latency-changing `set_param` trigger a PDC-aware rebuild) is Runtime
Scheduler/PDC-validation scope, not something to special-case per effect.

## Deliberately not done in this pass

- **COLOR 100–200% resonance-boost layer** (from the original rewrite prompt):
  `Live`/`Resonate` already exists specifically for zero-latency ringing/color
  character, per Codex's own architecture decision. Adding a second resonance
  concept inside `Map` mode would duplicate that role rather than move toward
  Chroma parity — Chroma's own documented feature set (dry/wet, quality
  switch, spectral morph, spectral gate) doesn't have an analogous knob.
- **Mono-channel optimization**: `DspEffect::prepare`'s `channels` argument is
  accepted but unused by every effect in this codebase, because
  `create_builtin_effect` always calls `prepare(sample_rate, MAX_BLOCK_SIZE,
  MAX_CHANNELS)` — the graph's internal buffers are always stereo, so there is
  currently no code path where `channels == 1`. Building a mono fast path
  would be dead code today.
- **Parameter renaming to a literal `COLOR`/`QUALITY`/`MORPH`/`GATE` id
  scheme**: the underlying `set_param` ids (`resonance`, `transient`, `depth`,
  `mix`, …) are unchanged to keep old project files loading without a
  migration; the UI labels now read `COLOR`/`MORPH`/`MAP`/`GATE` in `Map` mode
  (`LowerPanel.tsx`), which is the only place these names are user-visible.
- **Corpus-based quality gate against licensed Chroma/PITCHMAP renders**
  (Codex's own plan, step 4): requires the actual commercial plugins and a
  listening-test corpus outside this repository; not something this pass can
  execute.

## Performance record (F-5)

Measured with `cargo test -p ministudio-dsp --release colorizer_map_bench_reports_callback_cost -- --ignored --nocapture`
(`tests.rs`), 256-frame callbacks at 48 kHz, dense material (4-note chord +
noise floor), release build, single core:

| QUALITY | FFT / hop | Latency | Cost/callback | % of 5.33 ms budget |
|---|---|---:|---:|---:|
| Fast | 512 / 128 | ~11 ms | 158 µs | 2.97% |
| Clean | 1024 / 256 | ~21 ms | 120 µs | 2.24% |

Both stay under the 3%-per-instance target, but **Fast is not cheaper than
Clean** — it is close to the target's edge, not comfortably under it like
Clean is. The reason is hop rate, not transform size: at a fixed callback
size, Fast's 128-sample hop means the STFT frame (peak detection, harmonic
grouping, phase-locked resynthesis — not just the FFT itself) runs twice per
callback, where Clean's 256-sample hop runs it once. The smaller FFT does not
make up for running the whole per-hop pipeline twice as often. `QUALITY`
should be understood as trading *latency* (matching how Chroma's own manual
frames its equivalent switch), not CPU — the UI's ms labels reflect this
rather than implying Fast is the "light" option.

## Tests added this pass (`ministudio-dsp/src/tests.rs`)

- `colorizer_map_gate_shortens_ringing_after_the_source_releases`
- `colorizer_map_leaves_sub_60hz_content_unshifted`
- `colorizer_map_quality_switch_has_no_audible_click`
- `spectral::stft::tests::small_and_large_sizes_round_trip_without_scratch_overrun`

All pre-existing `colorizer_*` and `spectral::*` tests continued to pass
unchanged.
