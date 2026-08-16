# Spectral engine preservation and Colorizer 2.0 research

Status: Map/Fast implementation baseline, 2026-08-16

## Decision

MiniStudio will keep two complementary Colorizer backends instead of forcing one
algorithm to satisfy incompatible latency and quality goals.

- **Live / Resonate** keeps the current fixed-capacity modal bank. It is zero
  latency, allocation-free, and appropriate for tuned ringing and color-bass
  sound design.
- **Map / High Quality** will remap detected tonal components to a pitch grid
  while preserving transient and residual energy. It will use the reusable
  spectral layer and report a fixed latency to PDC.
- A future **Precision** backend may use a slice-wise invertible constant-Q /
  nonstationary Gabor transform if blind tests show that multi-resolution STFT
  cannot meet bass resolution and dense-mix quality targets.

The old 4096-point Colorizer implementation must not return as an effect-local
FFT/OLA block. Its useful mechanics are preserved as a format-neutral engine in
`ministudio-dsp/src/spectral`, while musical pitch mapping remains in the effect.

## Repository audit

Before this change, the repository held two coupled spectral implementations:

1. Git history contained Colorizer's 4096-point Hann WOLA resonator. It allocated
   outside `process`, but combined ring scheduling, mask policy, phase, decay,
   metering, and FFT mechanics in one effect.
2. `FormantShifter` contains a 2048-point phase vocoder with peak-region identity
   phase locking and a coarse spectral-envelope correction. Its phase wrapper
   was accidentally supplied by the later-included Colorizer source file.

The new `spectral/stft.rs` establishes the reusable base:

- shareable RustFFT forward/inverse plans;
- power-of-two sizes and validated hop factors;
- periodic sqrt-Hann weighted overlap-add;
- numerically derived overlap normalization rather than an effect-specific
  `1.5` or `2.0` constant;
- zero-padded causal startup after the first hop, avoiding loss of the first
  transient;
- preallocated stereo rings, spectra, and FFT scratch;
- positive-frequency-only mutation with automatic Hermitian reconstruction;
- an explicit `fft_size` latency contract for graph PDC;
- impulse and steady-state identity reconstruction tests.

It is now connected to Colorizer only through the explicit `Map` quality mode.
Existing sessions without a quality field remain on the original `Live` path;
new instances default to Map. Formant's phase wrapper now belongs to the
spectral module; moving its full phase-vocoder state is deferred until
sample-parity tests exist.

The first production slice includes linked-stereo true-frequency peak analysis,
a bounded harmonic-family mapper, 36-bin HPCP, per-bin transient preservation,
identity-style phase locking, exact 1024-sample PDC, and a latency-preserving
bypass. The callback path plans no FFTs and performs no allocation, locking,
logging, file I/O, or IPC. `Live` does not execute the STFT path at all.

## What the commercial references actually do

The products set two different targets.

### Xynth Audio Chroma

The official manual describes scale-conforming movement of input frequencies,
a zero-latency/high-quality switch, and optional spectral processing using 512-
or 1024-point FFTs. Its spectral morph replaces or blends wet magnitudes with dry
magnitudes to retain coloration while preserving dry transients and tails; its
spectral gate uses the dry input to shorten wet ringing. It also exposes a lower
internal sample-rate option to reduce CPU.

This makes Chroma the nearest product target for a low-latency creative
Colorizer. The current modal bank covers the resonant character and CPU goal,
but it **does not move out-of-scale energy into in-scale destinations**.

### Zynaptiq PITCHMAP / PITCHMAP::COLORS

Zynaptiq states that PITCHMAP separates a mixture into sounds including harmonic,
transient, and noise components, then processes and maps the separated sounds.
PITCHMAP::COLORS describes pitch-based separation followed by individual shifts
to pitch-grid slots, plus formant processing and transient bypass.

The exact MAP implementation is proprietary. Product parity therefore cannot be
claimed from an FFT mask or pitch-class resonator alone. MiniStudio needs a
tonal-component tracker, coherent remapping, and an explicitly preserved
transient/residual path. Dense masters remain the hardest case and must be judged
by blind listening, not by a unit-test tone.

## Algorithm assessment

| Candidate | Strength | Limitation | Decision |
|---|---|---|---|
| Fixed modal bank | Zero latency, very low CPU, stable resonant color | Filters/holds energy but does not retune sources | Keep as Live / Resonate |
| Uniform WOLA STFT mask | Simple, perfectly reconstructable, existing code | Fixed time/frequency resolution; a mask cannot separate overlapping notes | Keep as common engine, not final mapping algorithm |
| Peak-locked phase vocoder | Coherent frequency movement; existing Formant experience | Transient smearing and source collisions without decomposition | Core of first HQ prototype |
| Multi-resolution STFT | Better low-note frequency and high-frequency transient resolution | Band synchronization and phase continuity are more complex | Recommended HQ production path |
| Invertible CQT / NSGT (sliCQ) | Pitch-aligned resolution, perfect reconstruction, real-time slices | Largest implementation/validation cost | Precision fallback after measured need |
| NMF or neural de-mixing | Can model overlapping pitched sources | Iterative/model CPU, memory, training and failure opacity | Do not place in the audio path initially |

Research on nonstationary Gabor frames supports the long-term option: invertible
constant-Q transforms can provide logarithmic musical resolution and perfect
reconstruction, while slice-wise processing makes runtime independent of total
signal length. Adaptive nonstationary phase-vocoder work also reports better
time resolution at attacks and better frequency resolution for stationary
sinusoids, with adaptive phase locking reducing phasiness and transient smear.

## Colorizer 2.0 processing design

### 1. One analysis, three signal classes

For every spectral frame:

1. derive a stereo-linked energy spectrum without collapsing the actual L/R
   complex spectra;
2. detect interpolated spectral peaks and estimate true frequency from phase
   advance;
3. classify stable peak regions as tonal;
4. classify broadband flux/onset regions as transient;
5. treat the remainder as residual/noise.

Only the tonal component is pitch-mapped. Transients pass through or blend by a
single `Transient` amount, and residual energy remains at its original frequency
unless the user deliberately selects a synthetic mode. This follows the
deterministic-plus-stochastic principle of spectral modeling and avoids turning
drums, breath, and room tone into delayed pitched haze. A short causal
time/frequency median or stability filter is suitable for the first tonal mask;
it is much cheaper than iterative source separation.

### 2. Map fundamentals, not independent bins

Mapping every FFT bin to its nearest allowed pitch breaks timbre because a
source's partials can land on different notes. Instead:

- compute a high-resolution harmonic pitch-class profile from spectral peaks;
- maintain a bounded set of fundamental candidates and persistent peak tracks;
- attach harmonically related peaks to the same candidate;
- select the nearest allowed target with direction, range, and hysteresis;
- apply one frequency ratio to the candidate's entire partial family, retaining
  measured inharmonic offsets;
- preserve relative phase around each dominant peak (identity phase locking).

HPCP is an analysis/control feature, not the resynthesizer. It supplies automatic
key/chord confidence and detuning estimation; MIDI/manual pitch masks remain
sample-accurate authoritative targets.

### 3. Preserve envelope, stereo, and level

- Estimate a smooth log-magnitude envelope before remapping and reapply or shift
  it according to the future Formant control.
- Derive one set of pitch assignments from linked stereo energy, then transform
  the original L and R complex values with the same mapping. This preserves
  inter-channel differences instead of synthesizing dual mono.
- Use equal-power accumulation when multiple sources land on one destination,
  followed by slow loudness compensation. Do not normalize each frame; that
  pumps ambience and destroys dynamics.
- Crossfade pitch-assignment changes over several hops and require hysteresis so
  a partial near a note boundary cannot chatter.

### 4. Resolution and mode policy

- **Live / Resonate:** current modal bank, zero latency.
- **Map / Fast:** 1024-point, 1/4-hop WOLA; intended for synths and upper-band
  sound design where latency matters more than bass separation.
- **Map / HQ:** multi-resolution analysis. Start with 4096 low, 2048 mid, and
  512 high windows, align all output to the longest fixed latency, and use
  complementary band masks. The exact crossovers are chosen by listening and
  reconstruction tests rather than frozen in project data.
- **Precision (conditional):** sliCQ/NSGT with 24 or 36 bins per octave if HQ
  still confuses low fundamentals or smears attacks.

Changing modes changes declared latency and therefore must go through a control-
plane graph/PDC update. It must not occur as an unannounced parameter mutation in
the callback.

## Performance design

The built-in advantage should come from integration, not lower quality:

- create/share `StftPlan` objects on the control thread; never invoke an FFT
  planner from `process`;
- keep all rings, spectra, peak tracks, masks, and region arrays fixed-capacity;
- use RustFFT's planner so AVX/FMA, SSE4.1, and AArch64 NEON are selected
  automatically;
- compute peak detection, HPCP, transient flux, wet spectrum, and UI analyzer
  from the same transform;
- calculate mapping policy once per hop and reuse it for both audio channels;
- process only active peak regions for expensive phase/envelope work;
- precompute bin frequency, log-frequency, pitch-class contribution, window,
  and display lookup tables;
- coalesce scale/MIDI target changes and smooth the resulting map at hop rate;
- publish a decimated fixed-size analyzer snapshot from DSP state, never a new
  allocation or a React render per frame;
- bypass/silence can skip analysis only after the graph's tail/sleep contract
  says it is safe; local level guessing must not change PDC or lose MIDI wakeups.

Performance gates at 48 kHz / 256 stereo on the reference machine:

- no allocation, mutex, logging, file I/O, or synchronous IPC in processing;
- exact, stable PDC latency for every mode;
- identity reconstruction peak error below `2e-5` in development tests and a
  release null target below -110 dBFS after startup;
- one HQ Colorizer callback p99 below 15% of the 5.33 ms block deadline and max
  below 25%; no xrun in a 10-minute dense-mix run;
- compare CPU to Chroma and PITCHMAP at matched sample rate, buffer, latency,
  and approximately matched audible quality. “Faster” is not accepted from an
  unmatched zero-latency resonator versus a full pitch mapper.

## Quality gates

Product-level musical quality is an empirical requirement.

1. Build a versioned corpus: mono bass, detuned bass, pads, piano chords,
   distorted synths, guitar, vocal stems, drums plus harmony, dense masters,
   inharmonic/noise sources, and wide stereo ambience.
2. Render the same target scales/chords through MiniStudio, Chroma, PITCHMAP,
   the unprocessed reference, and deliberately degraded anchors.
3. Run hidden-reference MUSHRA-style listening tests. Release parity requires
   MiniStudio's median score to remain within five points of the better relevant
   reference for its declared mode, with no recurring catastrophic source class.
4. Track supporting metrics: target pitch-class energy, off-grid suppression,
   onset-energy error, log-spectral distance, LUFS delta, L/R correlation,
   inter-channel level/phase deviation, CPU p50/p95/p99/max, and allocations.
5. Keep failure clips as permanent regression fixtures. A change that improves
   a sine sweep but damages drums or stereo masters does not pass.

ITU-R BS.1534 supplies the formal multi-stimulus hidden-reference methodology;
objective metrics are diagnostic and cannot replace listening scores.

## Implementation sequence

1. **Completed:** preserve the generic WOLA STFT plan/streamer and phase
   primitive with reconstruction tests. Keep current Colorizer audible behavior.
2. **Completed:** add true-frequency peak interpolation, harmonic-family
   grouping, transient masks, and HPCP as independently tested primitives.
3. **Completed:** connect realtime Map/Fast mode, fixed PDC, MIDI/manual target
   snapshots, linked-stereo mapping decisions, analyzer reuse, and a stable
   latency-preserving bypass.
4. **Next quality gate:** render the versioned music corpus through MiniStudio,
   installed Chroma, and installed PITCHMAP with loudness/latency matching; keep
   the licensed reference renders outside the repository.
5. Add multi-resolution HQ only for source classes where blind tests show a
   repeatable Map/Fast gap, then add formant/envelope preservation controls.
6. Implement sliCQ/NSGT only if the measured HQ gap justifies its complexity.

No project format should persist FFT sizes or backend internals at this stage.
Persist semantic controls and a stable quality-mode identifier so algorithms can
improve without invalidating sessions.

## Primary sources and official product references

- [Xynth Audio Chroma manual](https://www.xynth.audio/docs/plugins/chroma)
- [Zynaptiq PITCHMAP overview](https://www.zynaptiq.com/pitchmap/)
- [Zynaptiq PITCHMAP operation FAQ](https://www.zynaptiq.com/pitchmap/pitchmap-faq/85cdc826bf74b9e851c44920d35fdae3/?tx_irfaq_pi1%5BshowUid%5D=6)
- [Zynaptiq PITCHMAP::COLORS details](https://www.zynaptiq.com/pitchmapcolors/pitchmapcolors-details/)
- [Laroche and Dolson, Phase-Vocoder: About this phasiness business](https://www.ee.columbia.edu/~dpwe/papers/LaroD97-phasiness.pdf)
- [Röbel, A new approach to transient processing in the phase vocoder](https://www.dafx.de/paper-archive/search?author%5B%5D=R%C3%B6bel%2C+A.&p=1&s=oldest&years%5B%5D=2003)
- [Ottosen and Dörfler, A Phase Vocoder based on Nonstationary Gabor Frames](https://arxiv.org/abs/1612.05156)
- [Holighaus et al., A framework for invertible, real-time constant-Q transforms](https://arxiv.org/abs/1210.0084)
- [Velasco et al., Constructing an invertible constant-Q transform with nonstationary Gabor frames](https://dav.grrrr.org/public/pub/velasco-2011-dafx.pdf)
- [FitzGerald, Harmonic/Percussive Separation Using Median Filtering](https://dafx.de/paper-archive/2010/DAFx10/DerryFitzGerald_DAFx10_P15.pdf)
- [Serra and Smith, Spectral Modeling Synthesis](https://mtg.upf.edu/node/251)
- [Essentia HPCP algorithm reference](https://essentia.upf.edu/reference/streaming_HPCP.html)
- [RustFFT official documentation](https://docs.rs/rustfft/latest/rustfft/)
- [ITU-R BS.1534 MUSHRA recommendation](https://www.itu.int/rec/r-rec-bs.1534/en)
