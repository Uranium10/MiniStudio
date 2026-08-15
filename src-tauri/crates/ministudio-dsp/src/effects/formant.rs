// Pitch shifter with independent formant control and two interchangeable engines.
//
// Mono engine: the original fixed-grain two-tap delay-line reader (no FFT), with formant
// correction via a periodic LPC envelope comparison driving a small Bell-biquad bank. Extremely
// light; excellent on a single voiced source, but it does not separate simultaneous pitches, so
// chords/mixes beat and smear through it.
//
// Poly engine: a short-time phase vocoder with identity phase locking (Laroche & Dolson) - the
// standard fix for the "phasy/robotic" artifact a naive per-bin vocoder produces, at negligible
// extra cost. Formant correction reuses the same spectrum via a smoothed log-magnitude envelope
// (no separate transform). This is the one that actually replaces a general-purpose pitch+formant
// plugin on chords, mixes, and ambience - the mono engine fundamentally cannot.
//
// `mode` picks Mono/Poly explicitly, or Auto: every analysis hop, `yin_pitch` (the same detector
// Roboter uses) checks whether the source is currently a single clear pitch and hands off to the
// cheap mono engine only then, falling back to the phase vocoder otherwise. A few hops of
// hysteresis avoid flapping between engines on borderline material.
//
// Both engines skip their expensive work entirely when there is nothing to do: pitch and formant
// both at unity is a plain delay-matched passthrough, and `formantLink` (formant follows pitch,
// the "natural" behavior) skips formant analysis/correction altogether rather than computing a
// correction that would only cancel back out.
const FORMANT_LPC_ORDER: usize = 20;
const FORMANT_ANALYSIS_FRAME: usize = 1024;
const FORMANT_ANALYSIS_HOP: usize = 256;
const FORMANT_BANDS: usize = 8;
// Log-spaced centers spanning the range typical vocal formants (F1-F5) occupy.
const FORMANT_BAND_HZ: [f32; FORMANT_BANDS] = [220.0, 380.0, 650.0, 1100.0, 1850.0, 3000.0, 4600.0, 6800.0];
const FORMANT_BAND_Q: f32 = 1.35;
/// Per-hop blend toward the freshly analyzed correction target; a few hops (~15-20 ms) to
/// settle avoids audible zipper noise between analyses without adding perceptible lag.
const FORMANT_GAIN_SMOOTHING: f32 = 0.3;

/// Consecutive analysis hops of agreement required before Auto actually swaps engines - about
/// 60 ms at the 256-sample hop. Long enough that a single ambiguous frame at a phrase boundary
/// does not flap the engine mid-note, short enough to feel responsive.
const FORMANT_AUTO_HYSTERESIS_HOPS: u8 = 10;
/// A source counts as "clearly monophonic" only when YIN is confident, low in aperiodicity, and
/// actually reports voiced - all three, so a quiet chord's strongest partial cannot masquerade
/// as a solo voice.
const FORMANT_AUTO_CONFIDENCE_MIN: f32 = 0.5;
const FORMANT_AUTO_APERIODICITY_MAX: f32 = 0.35;

const FORMANT_POLY_FFT_SIZE: usize = 2048;
const FORMANT_POLY_HOP: usize = FORMANT_POLY_FFT_SIZE / 4;
const FORMANT_POLY_BINS: usize = FORMANT_POLY_FFT_SIZE / 2 + 1;
/// Radius (in bins) of the log-magnitude moving average used to estimate the poly engine's
/// spectral envelope. Wide enough to average over harmonics rather than tracing them (which
/// would just reproduce the raw spectrum instead of a formant-scale envelope).
const FORMANT_POLY_ENVELOPE_RADIUS: usize = 24;

/// Declared latency whenever anything other than an explicitly-forced Mono is in play: the
/// phase vocoder's own OLA design (see Colorizer, which uses the identical single-ring-cursor
/// STFT/OLA pattern this reuses) needs a full FFT window of lookback before its first frame
/// closes. Auto reports this unconditionally - not "whatever the currently active engine costs" -
/// because the active engine can change mid-stream, and a latency contract that moves around at
/// runtime is worse than a host's PDC than a small, constant one. Users who need true low latency
/// pick Mono explicitly (see the `mode` docs on the frontend); that path alone reports 0.
const FORMANT_DECLARED_LATENCY: usize = FORMANT_POLY_FFT_SIZE;

#[derive(Clone, Copy, PartialEq)]
enum FormantMode {
    Auto,
    Mono,
    Poly,
}
impl FormantMode {
    fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::Mono,
            2 => Self::Poly,
            _ => Self::Auto,
        }
    }
}

/// Evaluate the LPC all-pole envelope's magnitude, in dB, at `freq`. The overall gain term
/// from Levinson-Durbin is deliberately dropped: callers compare two envelopes' *shapes* (one
/// mean-removed against the other), so a consistent per-curve normalization matters more than
/// an absolute level, and skipping it avoids replicating the recursion's error-power bookkeeping
/// just to cancel it out again.
fn lpc_envelope_db(coeffs: &[f32; FORMANT_LPC_ORDER + 1], freq: f32, sample_rate: f32) -> f32 {
    let w = 2.0 * PI * freq.clamp(20.0, sample_rate * 0.49) / sample_rate;
    let mut real = 1.0_f32;
    let mut imag = 0.0_f32;
    for k in 1..=FORMANT_LPC_ORDER {
        let angle = k as f32 * w;
        real += coeffs[k] * angle.cos();
        imag -= coeffs[k] * angle.sin();
    }
    let magnitude_sq = (real * real + imag * imag).max(1e-12);
    -10.0 * magnitude_sq.log10()
}

fn autocorrelate(frame: &[f32; FORMANT_ANALYSIS_FRAME]) -> [f32; FORMANT_LPC_ORDER + 1] {
    let mut r = [0.0_f32; FORMANT_LPC_ORDER + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        let mut sum = 0.0_f32;
        for n in lag..FORMANT_ANALYSIS_FRAME {
            sum += frame[n] * frame[n - lag];
        }
        *slot = sum;
    }
    r
}

/// Standard Levinson-Durbin recursion. Returns `a` with `a[0] == 1.0` so the all-pole filter is
/// `A(z) = 1 + sum_{k=1..order} a[k] z^-k`.
fn levinson_durbin(autocorr: &[f32; FORMANT_LPC_ORDER + 1]) -> [f32; FORMANT_LPC_ORDER + 1] {
    let mut a = [0.0_f32; FORMANT_LPC_ORDER + 1];
    a[0] = 1.0;
    let mut error = autocorr[0].max(1e-9);
    for i in 1..=FORMANT_LPC_ORDER {
        let mut acc = autocorr[i];
        for j in 1..i {
            acc += a[j] * autocorr[i - j];
        }
        let reflection = -acc / error;
        let previous = a;
        a[i] = reflection;
        for j in 1..i {
            a[j] = previous[j] + reflection * previous[i - j];
        }
        error = (error * (1.0 - reflection * reflection)).max(1e-9);
    }
    a
}

/// Smoothed log-magnitude envelope: a wide moving average in the log domain traces the coarse
/// formant shape without needing a second transform (cepstral liftering) or an LPC solve - cheap
/// enough to run every poly analysis hop only when formant correction is actually requested.
fn spectral_envelope(magnitude: &[f32], envelope: &mut [f32], bins: usize) {
    let radius = FORMANT_POLY_ENVELOPE_RADIUS;
    for k in 0..bins {
        let lo = k.saturating_sub(radius);
        let hi = (k + radius).min(bins - 1);
        let mut sum = 0.0_f32;
        for value in &magnitude[lo..=hi] {
            sum += value.max(1e-9).ln();
        }
        envelope[k] = (sum / (hi - lo + 1) as f32).exp();
    }
}

/// Identity phase locking regions: `region[k]` is the bin index of the nearest local-maximum
/// ("peak") to `k`. Locking every bin's synthesis phase to its region's peak (instead of letting
/// each bin drift independently) is what keeps a phase-vocoder shift from turning transients and
/// formants into the classic smeared/"phasy" wash - Laroche & Dolson's identity phase locking.
fn assign_phase_lock_regions(magnitude: &[f32], peaks: &mut Vec<usize>, region: &mut [u16], bins: usize) {
    peaks.clear();
    for k in 1..bins.saturating_sub(1) {
        if magnitude[k] > magnitude[k - 1] && magnitude[k] > magnitude[k + 1] && magnitude[k] > 1e-8 {
            peaks.push(k);
        }
    }
    if peaks.is_empty() {
        for (k, slot) in region.iter_mut().enumerate().take(bins) {
            *slot = k as u16;
        }
        return;
    }
    let mut peak_index = 0;
    for k in 0..bins {
        while peak_index + 1 < peaks.len()
            && (peaks[peak_index + 1] as i64 - k as i64).abs() <= (peaks[peak_index] as i64 - k as i64).abs()
        {
            peak_index += 1;
        }
        region[k] = peaks[peak_index] as u16;
    }
}

struct FormantPolyChannel {
    input_ring: Vec<f32>,
    ola_ring: Vec<f32>,
    dry_delay: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    last_phase: Vec<f32>,
    synthesis_phase: Vec<f32>,
    magnitude: Vec<f32>,
    true_omega: Vec<f32>,
    shifted_magnitude: Vec<f32>,
    shifted_true_omega: Vec<f32>,
    shifted_relative_phase: Vec<f32>,
    envelope_dry: Vec<f32>,
    envelope_shifted: Vec<f32>,
    region_peak: Vec<u16>,
    /// For each *shifted* bin, which analysis bin's phase it was scattered from (bins no
    /// analysis source landed on keep whatever they last held; their magnitude is 0 so it does
    /// not matter). Used to look up the analysis-frame phase for identity phase locking.
    source_bin: Vec<u16>,
    peaks: Vec<usize>,
}
impl FormantPolyChannel {
    fn new(scratch_len: usize) -> Self {
        Self {
            input_ring: vec![0.0; FORMANT_POLY_FFT_SIZE],
            ola_ring: vec![0.0; FORMANT_POLY_FFT_SIZE],
            dry_delay: vec![0.0; FORMANT_POLY_FFT_SIZE],
            spectrum: vec![Complex32::new(0.0, 0.0); FORMANT_POLY_FFT_SIZE],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            last_phase: vec![0.0; FORMANT_POLY_BINS],
            synthesis_phase: vec![0.0; FORMANT_POLY_BINS],
            magnitude: vec![0.0; FORMANT_POLY_BINS],
            true_omega: vec![0.0; FORMANT_POLY_BINS],
            shifted_magnitude: vec![0.0; FORMANT_POLY_BINS],
            shifted_true_omega: vec![0.0; FORMANT_POLY_BINS],
            shifted_relative_phase: vec![0.0; FORMANT_POLY_BINS],
            envelope_dry: vec![0.0; FORMANT_POLY_BINS],
            envelope_shifted: vec![0.0; FORMANT_POLY_BINS],
            region_peak: vec![0; FORMANT_POLY_BINS],
            source_bin: vec![0; FORMANT_POLY_BINS],
            peaks: Vec::with_capacity(FORMANT_POLY_BINS / 4),
        }
    }
    fn clear(&mut self) {
        self.input_ring.fill(0.0);
        self.ola_ring.fill(0.0);
        self.dry_delay.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
        self.last_phase.fill(0.0);
        self.synthesis_phase.fill(0.0);
    }
}

pub struct FormantShifter {
    sample_rate: f32,
    mode: FormantMode,
    pitch_semitones: f32,
    formant_semitones: f32,
    formant_link: bool,
    mix: Smoother,
    output: Smoother,
    bypassed: bool,

    // Shared analysis ring feeding both Auto's periodicity check and the mono engine's LPC
    // formant comparison - one pass over the input regardless of how many consumers need it.
    dry_ring: [f32; FORMANT_ANALYSIS_FRAME],
    shifted_ring: [f32; FORMANT_ANALYSIS_FRAME],
    ring_position: usize,
    ring_filled: usize,
    samples_since_analysis: usize,
    auto_engine: FormantMode, // Auto's current pick (Mono or Poly); meaningless outside FormantMode::Auto
    auto_streak: u8,
    auto_candidate: FormantMode,

    // Mono engine (delay-line pitch shift + LPC formant correction).
    delay: [Vec<f32>; MAX_CHANNELS],
    write_position: usize,
    grain_phase: f32,
    /// The delay-line's raw output *before* formant correction, refreshed every `mono_step`
    /// call. The analysis ring must compare dry against this, not against `mono_step`'s
    /// returned (corrected) signal - feeding the corrected signal back into the comparison
    /// would make each hop underestimate the correction still owed, self-limiting it.
    last_raw_shift: [f32; MAX_CHANNELS],
    target_band_db: [f32; FORMANT_BANDS],
    current_band_db: [f32; FORMANT_BANDS],
    bands: [Biquad; FORMANT_BANDS],

    // Poly engine (phase vocoder, identity phase locking, optional envelope correction).
    poly: [FormantPolyChannel; MAX_CHANNELS],
    poly_forward: Arc<dyn Fft<f32>>,
    poly_inverse: Arc<dyn Fft<f32>>,
    poly_window: Vec<f32>,
    poly_ring_position: usize,
    poly_samples_until_frame: usize,

    // Cheap fixed delay used to keep output timing constant at the declared latency whenever
    // the expensive engine is skipped (unity shift, or Auto currently on the near-zero-latency
    // mono engine) - see FORMANT_DECLARED_LATENCY.
    align_wet: [Vec<f32>; MAX_CHANNELS],
    align_dry: [Vec<f32>; MAX_CHANNELS],
    align_position: usize,
}

impl FormantShifter {
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let poly_forward = planner.plan_fft_forward(FORMANT_POLY_FFT_SIZE);
        let poly_inverse = planner.plan_fft_inverse(FORMANT_POLY_FFT_SIZE);
        let scratch_len = poly_forward
            .get_inplace_scratch_len()
            .max(poly_inverse.get_inplace_scratch_len());
        Self {
            sample_rate: 48_000.0,
            mode: FormantMode::Auto,
            pitch_semitones: 0.0,
            formant_semitones: 0.0,
            formant_link: true,
            mix: Smoother::new(1.0, 48_000.0, 0.01),
            output: Smoother::new(1.0, 48_000.0, 0.01),
            bypassed: false,
            dry_ring: [0.0; FORMANT_ANALYSIS_FRAME],
            shifted_ring: [0.0; FORMANT_ANALYSIS_FRAME],
            ring_position: 0,
            ring_filled: 0,
            samples_since_analysis: 0,
            auto_engine: FormantMode::Mono,
            auto_streak: 0,
            auto_candidate: FormantMode::Mono,
            delay: [Vec::new(), Vec::new()],
            write_position: 0,
            grain_phase: 0.0,
            last_raw_shift: [0.0; MAX_CHANNELS],
            target_band_db: [0.0; FORMANT_BANDS],
            current_band_db: [0.0; FORMANT_BANDS],
            bands: std::array::from_fn(|_| Biquad::new()),
            poly: [
                FormantPolyChannel::new(scratch_len),
                FormantPolyChannel::new(scratch_len),
            ],
            poly_forward,
            poly_inverse,
            poly_window: vec![0.0; FORMANT_POLY_FFT_SIZE],
            poly_ring_position: 0,
            poly_samples_until_frame: FORMANT_POLY_FFT_SIZE,
            align_wet: [Vec::new(), Vec::new()],
            align_dry: [Vec::new(), Vec::new()],
            align_position: 0,
        }
    }

    fn pitch_ratio(&self) -> f32 {
        2.0_f32.powf(self.pitch_semitones / 12.0)
    }
    fn formant_ratio(&self) -> f32 {
        if self.formant_link {
            self.pitch_ratio()
        } else {
            2.0_f32.powf(self.formant_semitones / 12.0)
        }
    }
    /// Nothing to shift and formants are already following pitch (or explicitly matched to it):
    /// both engines degenerate to identity, so skip their work and just delay-match the dry
    /// signal instead of paying for an FFT or an LPC solve that would only cancel back out.
    fn is_unity(&self) -> bool {
        (self.pitch_ratio() - 1.0).abs() < 1e-3 && (self.formant_ratio() / self.pitch_ratio() - 1.0).abs() < 1e-3
    }

    /// Feeds the shared analysis ring and, on the LPC/YIN hop cadence, updates whichever of
    /// Auto's periodicity vote or the mono engine's formant correction target is relevant.
    fn tick_analysis(&mut self, dry_mono: f32, shifted_mono: f32) {
        self.dry_ring[self.ring_position] = dry_mono;
        self.shifted_ring[self.ring_position] = shifted_mono;
        self.ring_position = increment_wrap(self.ring_position, FORMANT_ANALYSIS_FRAME);
        self.ring_filled = (self.ring_filled + 1).min(FORMANT_ANALYSIS_FRAME);
        self.samples_since_analysis += 1;
        if self.samples_since_analysis < FORMANT_ANALYSIS_HOP || self.ring_filled < FORMANT_ANALYSIS_FRAME {
            return;
        }
        self.samples_since_analysis = 0;
        if self.mode == FormantMode::Auto {
            self.update_auto_vote();
        }
        if !self.formant_link {
            self.update_mono_formant_target();
        }
    }

    fn ordered_ring(ring: &[f32; FORMANT_ANALYSIS_FRAME], position: usize) -> [f32; FORMANT_ANALYSIS_FRAME] {
        std::array::from_fn(|index| ring[(position + index) % FORMANT_ANALYSIS_FRAME])
    }

    fn update_auto_vote(&mut self) {
        let frame = Self::ordered_ring(&self.dry_ring, self.ring_position);
        let estimate = yin_pitch(&frame, self.sample_rate);
        let candidate = if estimate.voiced
            && estimate.confidence >= FORMANT_AUTO_CONFIDENCE_MIN
            && estimate.aperiodicity <= FORMANT_AUTO_APERIODICITY_MAX
        {
            FormantMode::Mono
        } else {
            FormantMode::Poly
        };
        if candidate == self.auto_candidate {
            self.auto_streak = self.auto_streak.saturating_add(1);
        } else {
            self.auto_candidate = candidate;
            self.auto_streak = 1;
        }
        if self.auto_streak >= FORMANT_AUTO_HYSTERESIS_HOPS {
            self.auto_engine = candidate;
        }
    }

    fn update_mono_formant_target(&mut self) {
        let mut dry_frame = [0.0_f32; FORMANT_ANALYSIS_FRAME];
        let mut shifted_frame = [0.0_f32; FORMANT_ANALYSIS_FRAME];
        let mut energy = 0.0_f32;
        for index in 0..FORMANT_ANALYSIS_FRAME {
            let window = 0.5 - 0.5 * (2.0 * PI * index as f32 / (FORMANT_ANALYSIS_FRAME - 1) as f32).cos();
            let slot = (self.ring_position + index) % FORMANT_ANALYSIS_FRAME;
            let dry = self.dry_ring[slot];
            dry_frame[index] = dry * window;
            shifted_frame[index] = self.shifted_ring[slot] * window;
            energy += dry * dry;
        }
        if energy < 1e-8 {
            self.target_band_db = [0.0; FORMANT_BANDS];
            return;
        }
        let dry_lpc = levinson_durbin(&autocorrelate(&dry_frame));
        let shifted_lpc = levinson_durbin(&autocorrelate(&shifted_frame));
        let formant_ratio = self.formant_ratio();
        let mut raw = [0.0_f32; FORMANT_BANDS];
        let mut mean = 0.0_f32;
        for (band, &freq) in FORMANT_BAND_HZ.iter().enumerate() {
            let source_freq = (freq / formant_ratio).clamp(60.0, self.sample_rate * 0.45);
            let target_db = lpc_envelope_db(&dry_lpc, source_freq, self.sample_rate);
            let shifted_db = lpc_envelope_db(&shifted_lpc, freq, self.sample_rate);
            raw[band] = target_db - shifted_db;
            mean += raw[band];
        }
        mean /= FORMANT_BANDS as f32;
        for band in 0..FORMANT_BANDS {
            self.target_band_db[band] = (raw[band] - mean).clamp(-18.0, 18.0);
        }
        self.settle_mono_formant_bands();
    }

    /// Blends toward the freshly analyzed target and reconfigures the correction biquads. Runs
    /// once per analysis hop (~5 ms), not per sample: the target is otherwise constant between
    /// hops, so recomputing Bell coefficients every sample would be pure waste.
    fn settle_mono_formant_bands(&mut self) {
        for band in 0..FORMANT_BANDS {
            self.current_band_db[band] += (self.target_band_db[band] - self.current_band_db[band]) * FORMANT_GAIN_SMOOTHING;
            self.bands[band].configure(FilterKind::Bell, FORMANT_BAND_HZ[band], self.current_band_db[band], FORMANT_BAND_Q, self.sample_rate);
        }
    }

    fn mono_step(&mut self, dry: [f32; MAX_CHANNELS]) -> [f32; MAX_CHANNELS] {
        let pitch_ratio = self.pitch_ratio();
        const GRAIN_MS: f32 = 55.0;
        let grain_samples = (GRAIN_MS * 0.001 * self.sample_rate).max(8.0);
        let delay_len = self.delay[0].len();
        self.delay[0][self.write_position] = dry[0];
        self.delay[1][self.write_position] = dry[1];

        self.grain_phase = (self.grain_phase + (1.0 - pitch_ratio) / grain_samples).rem_euclid(1.0);
        let phase_b = (self.grain_phase + 0.5).fract();
        let weight_a = 0.5 - 0.5 * (2.0 * PI * self.grain_phase).cos();
        let weight_b = 1.0 - weight_a;
        let delay_a = grain_samples * self.grain_phase;
        let delay_b = grain_samples * phase_b;
        let shifted: [f32; MAX_CHANNELS] = std::array::from_fn(|channel| {
            denormal(
                fractional_read(&self.delay[channel], self.write_position as f32 - delay_a) * weight_a
                    + fractional_read(&self.delay[channel], self.write_position as f32 - delay_b) * weight_b,
            )
        });
        self.write_position = increment_wrap(self.write_position, delay_len);
        self.last_raw_shift = shifted;

        if self.formant_link {
            return shifted;
        }
        let mut corrected = shifted;
        for channel in 0..MAX_CHANNELS {
            for band in &mut self.bands {
                corrected[channel] = band.process(channel, corrected[channel]);
            }
        }
        corrected
    }

    fn poly_prepare_window(&mut self) {
        for (index, value) in self.poly_window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * PI * index as f32 / FORMANT_POLY_FFT_SIZE as f32).cos();
        }
    }

    fn poly_step(&mut self, dry: [f32; MAX_CHANNELS]) -> [f32; MAX_CHANNELS] {
        let position = self.poly_ring_position;
        let mut wet = [0.0_f32; MAX_CHANNELS];
        for channel in 0..MAX_CHANNELS {
            let state = &mut self.poly[channel];
            state.input_ring[position] = dry[channel];
            let delayed_dry = state.dry_delay[position];
            state.dry_delay[position] = dry[channel];
            wet[channel] = state.ola_ring[position];
            state.ola_ring[position] = 0.0;
            let _ = delayed_dry; // dry/wet alignment is handled by the caller's align buffers
        }
        self.poly_ring_position = increment_wrap(self.poly_ring_position, FORMANT_POLY_FFT_SIZE);
        self.poly_samples_until_frame -= 1;
        if self.poly_samples_until_frame == 0 {
            self.poly_samples_until_frame = FORMANT_POLY_HOP;
            self.poly_render_frame();
        }
        wet
    }

    fn poly_render_frame(&mut self) {
        let pitch_ratio = self.pitch_ratio();
        let formant_ratio = self.formant_ratio();
        let correct_formant = !self.formant_link;
        let position = self.poly_ring_position;
        let bins = FORMANT_POLY_BINS;
        let hop = FORMANT_POLY_HOP as f32;
        for channel_index in 0..MAX_CHANNELS {
            let (forward, inverse) = (&self.poly_forward, &self.poly_inverse);
            let window = &self.poly_window;
            let state = &mut self.poly[channel_index];

            for index in 0..FORMANT_POLY_FFT_SIZE {
                let input_index = (position + index) % FORMANT_POLY_FFT_SIZE;
                state.spectrum[index] = Complex32::new(state.input_ring[input_index] * window[index], 0.0);
            }
            forward.process_with_scratch(&mut state.spectrum, &mut state.scratch);

            for k in 0..bins {
                let bin = state.spectrum[k];
                let magnitude = bin.norm();
                let phase = bin.arg();
                let expected = 2.0 * PI * k as f32 / FORMANT_POLY_FFT_SIZE as f32 * hop;
                let deviation = wrap_phase(phase - state.last_phase[k] - expected);
                state.last_phase[k] = phase;
                let bin_omega = 2.0 * PI * k as f32 / FORMANT_POLY_FFT_SIZE as f32;
                state.true_omega[k] = bin_omega + deviation / hop;
                state.magnitude[k] = magnitude;
            }

            // Scatter (push), not gather: walking analysis bins and writing `round(k*ratio)` is
            // the standard smbPitchShift structure. Gathering the other way (`source =
            // round(k/ratio)`) looks equivalent but is not - on an integer upshift it copies the
            // same source into *every* adjacent output bin along the way, producing magnitude
            // plateaus that defeat the strict local-maximum peak test below and scramble phase
            // locking. Scatter leaves real gaps (zero magnitude) between sparse harmonics
            // instead, which keeps peaks isolated and reconstruction coherent.
            state.shifted_magnitude[..bins].fill(0.0);
            for k in 0..bins {
                let dest = (k as f32 * pitch_ratio).round();
                if dest >= 0.0 && (dest as usize) < bins {
                    let dest = dest as usize;
                    state.shifted_magnitude[dest] += state.magnitude[k];
                    state.shifted_true_omega[dest] = state.true_omega[k] * pitch_ratio;
                    state.source_bin[dest] = k as u16;
                }
            }

            if correct_formant {
                spectral_envelope(&state.magnitude, &mut state.envelope_dry, bins);
                spectral_envelope(&state.shifted_magnitude, &mut state.envelope_shifted, bins);
                for k in 0..bins {
                    let source = (k as f32 / formant_ratio).round();
                    let target = if source >= 0.0 && (source as usize) < bins {
                        state.envelope_dry[source as usize]
                    } else {
                        state.envelope_shifted[k]
                    };
                    let gain = (target / state.envelope_shifted[k].max(1e-9)).clamp(0.1, 10.0);
                    state.shifted_magnitude[k] *= gain;
                }
            }

            assign_phase_lock_regions(&state.shifted_magnitude, &mut state.peaks, &mut state.region_peak, bins);
            // Relative-to-peak phase, measured on the *analysis* frame at each bin's scattered
            // source - this is what identity phase locking preserves into synthesis.
            for k in 0..bins {
                let source = state.source_bin[k] as usize;
                let peak = state.region_peak[k] as usize;
                let peak_source = state.source_bin[peak] as usize;
                state.shifted_relative_phase[k] = wrap_phase(state.last_phase[source] - state.last_phase[peak_source]);
            }
            // Peaks accumulate their own phase; every other bin locks to its region's peak.
            for &peak in &state.peaks.clone() {
                state.synthesis_phase[peak] = wrap_phase(state.synthesis_phase[peak] + state.shifted_true_omega[peak] * hop);
            }
            if state.peaks.is_empty() && bins > 0 {
                state.synthesis_phase[0] = wrap_phase(state.synthesis_phase[0] + state.shifted_true_omega[0] * hop);
            }
            for k in 0..bins {
                let peak = state.region_peak[k] as usize;
                if peak != k {
                    state.synthesis_phase[k] = wrap_phase(state.synthesis_phase[peak] + state.shifted_relative_phase[k]);
                }
            }
            state.synthesis_phase[0] = 0.0;
            if bins > 1 {
                state.synthesis_phase[bins - 1] = 0.0;
            }

            for k in 0..bins {
                state.spectrum[k] = Complex32::from_polar(state.shifted_magnitude[k], state.synthesis_phase[k]);
                if k > 0 && k < FORMANT_POLY_FFT_SIZE / 2 {
                    state.spectrum[FORMANT_POLY_FFT_SIZE - k] = state.spectrum[k].conj();
                }
            }
            inverse.process_with_scratch(&mut state.spectrum, &mut state.scratch);
            let normalization = 1.0 / (FORMANT_POLY_FFT_SIZE as f32 * 1.5);
            for index in 0..FORMANT_POLY_FFT_SIZE {
                let output_index = (position + index) % FORMANT_POLY_FFT_SIZE;
                state.ola_ring[output_index] += state.spectrum[index].re * window[index] * normalization;
            }
        }
    }
}

impl DspEffect for FormantShifter {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        let len = (sample_rate * 0.25).ceil() as usize + 16;
        self.delay = [vec![0.0; len], vec![0.0; len]];
        self.align_wet = [vec![0.0; FORMANT_DECLARED_LATENCY], vec![0.0; FORMANT_DECLARED_LATENCY]];
        self.align_dry = [vec![0.0; FORMANT_DECLARED_LATENCY], vec![0.0; FORMANT_DECLARED_LATENCY]];
        self.align_position = 0;
        self.mix = Smoother::new(self.mix.target, sample_rate, 0.01);
        self.output = Smoother::new(self.output.target, sample_rate, 0.01);
        for band in &mut self.bands {
            band.reset();
        }
        let mut planner = FftPlanner::<f32>::new();
        self.poly_forward = planner.plan_fft_forward(FORMANT_POLY_FFT_SIZE);
        self.poly_inverse = planner.plan_fft_inverse(FORMANT_POLY_FFT_SIZE);
        let scratch_len = self.poly_forward.get_inplace_scratch_len().max(self.poly_inverse.get_inplace_scratch_len());
        self.poly = [FormantPolyChannel::new(scratch_len), FormantPolyChannel::new(scratch_len)];
        self.poly_prepare_window();
        self.poly_ring_position = 0;
        self.poly_samples_until_frame = FORMANT_POLY_HOP;
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed || self.delay[0].is_empty() {
            return;
        }
        for frame in 0..frames {
            let dry = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let unity = self.is_unity();
            let want_poly = match self.mode {
                FormantMode::Poly => true,
                FormantMode::Mono => false,
                FormantMode::Auto => self.auto_engine == FormantMode::Poly,
            };
            let explicit_mono = self.mode == FormantMode::Mono;

            // Only run the mono engine when it is actually producing this sample's output (or
            // there is nothing to shift): if the poly engine is driving output right now, the
            // mono engine's delay-line/LPC state would just be discarded, so skip it entirely.
            // Its formant target goes briefly stale across a switch back to Mono - one analysis
            // hop (~5 ms) to refresh - which is a cheaper trade than running two engines at once.
            let (raw_wet, raw_latency_samples, shifted_probe) = if unity {
                (dry, 0usize, dry)
            } else if want_poly {
                (self.poly_step(dry), FORMANT_DECLARED_LATENCY, dry)
            } else {
                let mono_probe = self.mono_step(dry);
                // Feed the analysis ring the *raw* (pre-correction) shift, not `mono_probe`
                // itself - see `last_raw_shift`'s doc comment.
                (mono_probe, 0usize, self.last_raw_shift)
            };
            self.tick_analysis((dry[0] + dry[1]) * 0.5, (shifted_probe[0] + shifted_probe[1]) * 0.5);

            let (wet, mix_dry) = if explicit_mono || raw_latency_samples == FORMANT_DECLARED_LATENCY {
                // Already at the declared latency (poly ran for real), or the user forced Mono
                // (0 declared latency, no alignment needed either way).
                (raw_wet, dry)
            } else {
                // Skipped the expensive engine this sample (unity, or Auto currently on mono) but
                // still owe FORMANT_DECLARED_LATENCY of delay so output timing does not jump
                // around as Auto swaps engines or the pitch knob crosses zero.
                let mut delayed_wet = [0.0; MAX_CHANNELS];
                let mut delayed_dry = [0.0; MAX_CHANNELS];
                for channel in 0..MAX_CHANNELS {
                    delayed_wet[channel] = self.align_wet[channel][self.align_position];
                    self.align_wet[channel][self.align_position] = raw_wet[channel];
                    delayed_dry[channel] = self.align_dry[channel][self.align_position];
                    self.align_dry[channel][self.align_position] = dry[channel];
                }
                (delayed_wet, delayed_dry)
            };
            if !explicit_mono {
                self.align_position = increment_wrap(self.align_position, FORMANT_DECLARED_LATENCY.max(1));
            }

            let mix = self.mix.next().clamp(0.0, 1.0);
            let output = self.output.next();
            for channel in 0..MAX_CHANNELS {
                buffer.channels[channel][frame] = denormal((mix_dry[channel] * (1.0 - mix) + wet[channel] * mix) * output);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "mode" => self.mode = FormantMode::from_param(value),
            "pitchSemitones" | "pitch" => self.pitch_semitones = value.clamp(-24.0, 24.0),
            "formantSemitones" | "formant" => self.formant_semitones = value.clamp(-24.0, 24.0),
            "formantLink" | "link" => self.formant_link = value >= 0.5,
            "mix" => self.mix.set_target(value.clamp(0.0, 1.0)),
            "outputDb" => self.output.set_target(db_to_gain(value.clamp(-24.0, 24.0))),
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        for channel in &mut self.delay {
            channel.fill(0.0);
        }
        self.write_position = 0;
        self.grain_phase = 0.0;
        self.last_raw_shift = [0.0; MAX_CHANNELS];
        self.dry_ring = [0.0; FORMANT_ANALYSIS_FRAME];
        self.shifted_ring = [0.0; FORMANT_ANALYSIS_FRAME];
        self.ring_position = 0;
        self.ring_filled = 0;
        self.samples_since_analysis = 0;
        self.auto_engine = FormantMode::Mono;
        self.auto_streak = 0;
        self.auto_candidate = FormantMode::Mono;
        self.target_band_db = [0.0; FORMANT_BANDS];
        self.current_band_db = [0.0; FORMANT_BANDS];
        for band in &mut self.bands {
            band.reset();
        }
        for channel in &mut self.poly {
            channel.clear();
        }
        self.poly_ring_position = 0;
        self.poly_samples_until_frame = FORMANT_POLY_HOP;
        for channel in &mut self.align_wet {
            channel.fill(0.0);
        }
        for channel in &mut self.align_dry {
            channel.fill(0.0);
        }
        self.align_position = 0;
    }
    fn latency_samples(&self) -> usize {
        if self.mode == FormantMode::Mono {
            0
        } else {
            FORMANT_DECLARED_LATENCY
        }
    }
}
