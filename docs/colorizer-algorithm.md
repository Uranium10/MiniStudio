# Colorizer harmonic-family pitch mapper

Status: implemented 2026-08-17. This document supersedes the former dual
Live/Map design. Colorizer now has one spectral signal path with Fast and Clean
quality plans; the zero-latency modal resonator was removed.

## Signal path

```text
stereo input
  -> linked-stereo analysis (RealFFT, Hann/Hann WOLA)
  -> peak detection and source-region partition
  -> at most four tracked harmonic families
  -> one smoothed pitch ratio per family
  -> source-region phase-preserving translation
  -> untouched non-peak residual
  -> energy compensation
  -> optional transient morph and broadband gate
  -> optional COLOR > 100 resonance layer
  -> inverse RealFFT and overlap-add
```

All plans, spectra, rings, peak/group tracks and scratch buffers are allocated
in `prepare`. The audio callback performs no allocation, deallocation, lock,
I/O or logging.

## STFT and gain contract

- Fast: FFT 512, hop 128, reported latency 512 samples.
- Clean: FFT 1024, hop 256, reported latency 1024 samples.
- Analysis and synthesis both use a periodic Hann window.
- With hop `N/4`, the Hann-squared overlap sum is 1.5. `StftPlan` derives the
  phase-dependent sum and folds both COLA and RealFFT inverse normalization
  into the synthesis window.
- Consumed overlap-add samples are cleared immediately.

Identity tests for both qualities require sample-aligned error below -60 dB
and an explicit input/output RMS difference no greater than 0.5 dB.

## Linked analysis and peak regions

Peak decisions use linked L/R energy and one shared phase/frequency estimate,
so the channels cannot choose different targets. A peak must exceed its two
neighbors on each side and the larger of a -60 dB frame-relative floor and a
dynamic mean floor. Log-parabolic interpolation refines the bin; phase advance
provides the instantaneous frequency.

Peaks remain in ascending source-bin order. Each source region extends to the
local minimum between neighboring peaks. Bins outside all regions are residual
and pass through unchanged. A moved region is moved in full; no tapered source
copy remains behind.

## Harmonic-family estimation

Up to four simultaneous families are tracked. Previous fundamentals seed the
next frame before new candidates are considered. For an unassigned strong
peak, candidates assume it is harmonic 1 through 4. Other peaks near integer
multiples contribute magnitude-weighted support, with a slightly wider cents
tolerance for high partials. A family needs at least two supported partials.

Only the fundamental is snapped to an enabled pitch class. Every member then
uses that one ratio, preserving integer harmonic relationships. Unassigned
peaks use the same nearest-class rule individually. Shifts below 5 cents and
above 150 cents are bypassed; accepted ratios are smoothed with a 25 ms time
constant. The smoothing deadband is applied to the target, not repeatedly to
the intermediate value, so small corrections can converge.

## Phase-preserving translation

For source peak `p`, source bin `k_p`, target frequency `f_t`, and hop `H`:

```text
delta = round((f_t - f_source) * N / sample_rate)
theta_p <- wrap(theta_p + 2*pi*f_t*H/sample_rate)
dest = k + delta
Y_c[dest] += abs(X_c[k]) * exp(i * (theta_p + angle(X_c[k]) - angle(X_c[k_p])))
```

Peak tracks match source frequencies within 60 cents. New tracks initialize
`theta_p` from the source peak; unmatched old tracks are discarded every
frame. There is no destination-spectrum peak detection and no per-bin
independent phase rotation.

## Morph, gate, energy and resonance

- MORPH uses global spectral flux. Between flux 0.10 and 0.25 it crossfades
  the mapped frame toward the original frame, preserving transient output
  rather than reducing the pitch ratio.
- GATE follows the dry broadband envelope. It applies only to shifted energy,
  never compares dry and wet bin-by-bin, and is completely skipped at zero.
- Frame energy compensation is smoothed and limited to +/-3 dB.
- COLOR 0..100 is delayed dry/wet. COLOR 100..200 keeps wet at 100% and enables
  a separate bounded resonance hold. The hold branch is cleared and skipped at
  COLOR <= 100, so the normal path has no recursive tail.

## Parameters and compatibility

The active parameter set is `color`, `quality`, `morph`, `gate`, `midi`, and
`pitch0` through `pitch11`. Old `mix`, `transient`, and `mapQuality` values are
accepted as migration aliases. Retired Live-engine `resonance`, `decay`, and
`depth` values are harmless no-ops. Old sessions apply `mapQuality` after the
unordered parameter map so their original FFT size is deterministic.

## Verification

The native test suite covers:

- I-1 identity waveform and RMS gain at both FFT sizes;
- I-1b no tail after `N + hop` at COLOR 100 and GATE 0;
- I-2 445 Hz to 440 Hz correction and in-key 440 Hz bypass;
- I-3 shared-ratio harmonic-family preservation;
- I-4 sustained-frame autocorrelation;
- I-5 energy within 3 dB;
- I-6 transient peak at least 90% of dry;
- I-7 measured impulse latency equals `latency_samples()`;
- I-8 zero allocations and deallocations through the complete `AudioCore`
  render call graph with Colorizer active;
- own-track MIDI pitch-class following, linked-stereo polarity/level
  preservation and long-run finite output.

Measured performance is recorded in `docs/colorizer-bench.md`.
