const ROBOTER_YIN_FRAME: usize = 1024;
const ROBOTER_YIN_MAX_LAG: usize = 320;
const ROBOTER_VOICE_COUNT: usize = 6;
const ROBOTER_HARMONY_COUNT: usize = 5;
const ROBOTER_ANALYSIS_HOP: usize = 256;

const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const MAJOR_SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR_SCALE: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
const ROBOTER_DETUNE_CENTS: [f32; ROBOTER_VOICE_COUNT] = [0.0, -4.0, 3.5, -6.5, 7.0, -3.0];
const ROBOTER_DRIFT_HZ: [f32; ROBOTER_VOICE_COUNT] = [0.0, 0.13, 0.17, 0.21, 0.25, 0.29];
const ROBOTER_EXTRA_DELAY_SEC: [f32; ROBOTER_VOICE_COUNT] =
    [0.0, 0.010, 0.006, 0.009, 0.013, 0.015];

#[derive(Clone, Copy, Debug, Default)]
struct PitchEstimate {
    frequency: f32,
    confidence: f32,
    aperiodicity: f32,
    voiced: bool,
}

#[derive(Clone, Copy)]
struct ChromaEvent {
    pitch_class: u8,
    weight: f32,
}
impl ChromaEvent {
    const EMPTY: Self = Self {
        pitch_class: u8::MAX,
        weight: 0.0,
    };
}

#[derive(Clone, Copy)]
struct RoboterVoice {
    phase: f32,
    drift_phase: f32,
    current_ratio: f32,
    target_ratio: f32,
    gain: f32,
    target_gain: f32,
    pan: f32,
    target_pan: f32,
    low_state: [f32; MAX_CHANNELS],
}
impl RoboterVoice {
    const fn new() -> Self {
        Self {
            phase: 0.0,
            drift_phase: 0.0,
            current_ratio: 1.0,
            target_ratio: 1.0,
            gain: 0.0,
            target_gain: 0.0,
            pan: 0.0,
            target_pan: 0.0,
            low_state: [0.0; MAX_CHANNELS],
        }
    }

    #[inline(always)]
    fn render(
        &mut self,
        delay: &[Vec<f32>; MAX_CHANNELS],
        write_position: usize,
        base_delay: usize,
        period: f32,
        sample_rate: f32,
        voice_index: usize,
        retime_seconds: f32,
    ) -> [f32; MAX_CHANNELS] {
        if retime_seconds <= 0.000_05 {
            self.current_ratio = self.target_ratio;
        } else {
            let coefficient = (-1.0 / (retime_seconds * sample_rate)).exp();
            self.current_ratio =
                self.target_ratio + (self.current_ratio - self.target_ratio) * coefficient;
        }
        self.drift_phase = (self.drift_phase + ROBOTER_DRIFT_HZ[voice_index] / sample_rate).fract();
        let drift_cents = (2.0 * PI * self.drift_phase).sin() * 1.5;
        let ratio = self.current_ratio
            * 2.0_f32.powf((ROBOTER_DETUNE_CENTS[voice_index] + drift_cents) / 1200.0);
        let window = (period * 2.0).clamp(sample_rate / 1000.0 * 2.0, sample_rate / 80.0 * 2.0);
        self.phase = (self.phase + (1.0 - ratio) / window).rem_euclid(1.0);
        let phase_b = (self.phase + 0.5).fract();
        let weight_a = 0.5 - 0.5 * (2.0 * PI * self.phase).cos();
        let weight_b = 1.0 - weight_a;
        let extra_delay = ROBOTER_EXTRA_DELAY_SEC[voice_index] * sample_rate;
        let delay_a = base_delay as f32 + extra_delay + self.phase * window;
        let delay_b = base_delay as f32 + extra_delay + phase_b * window;
        std::array::from_fn(|channel| {
            denormal(
                fractional_read(&delay[channel], write_position as f32 - delay_a) * weight_a
                    + fractional_read(&delay[channel], write_position as f32 - delay_b) * weight_b,
            )
        })
    }

    #[inline(always)]
    fn advance_mix(&mut self, sample_rate: f32) {
        let fade_step = 1.0 / (0.04 * sample_rate).max(1.0);
        if self.gain < self.target_gain {
            self.gain = (self.gain + fade_step).min(self.target_gain)
        } else {
            self.gain = (self.gain - fade_step).max(self.target_gain)
        }
        let pan_step = 1.0 / (0.03 * sample_rate).max(1.0);
        self.pan += (self.target_pan - self.pan).clamp(-pan_step, pan_step)
    }

    #[inline(always)]
    fn mono_bass(&mut self, input: [f32; MAX_CHANNELS], sample_rate: f32) -> [f32; 2] {
        let coefficient = 1.0 - (-2.0 * PI * 200.0 / sample_rate).exp();
        for channel in 0..MAX_CHANNELS {
            self.low_state[channel] += (input[channel] - self.low_state[channel]) * coefficient
        }
        let mono_low = (self.low_state[0] + self.low_state[1]) * 0.5;
        [
            input[0] - self.low_state[0] + mono_low,
            input[1] - self.low_state[1] + mono_low,
        ]
    }
}

/// Monophonic vocal pitch correction and automatic diatonic harmonization.
/// Analysis uses a 1024-sample downsampled YIN frame at a 256-input-sample hop.
/// Six fixed PSOLA-style granular voices are allocated in `prepare`; processing
/// only mutates those buffers and never creates or destroys a voice.
pub struct Roboter {
    sample_rate: f32,
    amount: f32,
    number: usize,
    downsample_factor: usize,
    downsample_sum: f32,
    downsample_count: usize,
    samples_since_analysis: usize,
    pitch_ring: [f32; ROBOTER_YIN_FRAME],
    pitch_position: usize,
    pitch_filled: usize,
    raw_midi: f32,
    smoothed_midi: f32,
    detected_hz: f32,
    pitch_confidence: f32,
    aperiodicity: f32,
    voiced: bool,
    chroma_histogram: [f32; 12],
    chroma_history: Vec<ChromaEvent>,
    chroma_position: usize,
    chroma_weight: f32,
    key_root: usize,
    key_minor: bool,
    key_valid: bool,
    key_confidence: f32,
    active_intervals: [f32; ROBOTER_HARMONY_COUNT],
    voices: [RoboterVoice; ROBOTER_VOICE_COUNT],
    delay: [Vec<f32>; MAX_CHANNELS],
    write_position: usize,
    latency: usize,
    bypassed: bool,
}
impl Roboter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            amount: 0.72,
            number: 0,
            downsample_factor: 2,
            downsample_sum: 0.0,
            downsample_count: 0,
            samples_since_analysis: 0,
            pitch_ring: [0.0; ROBOTER_YIN_FRAME],
            pitch_position: 0,
            pitch_filled: 0,
            raw_midi: 69.0,
            smoothed_midi: 69.0,
            detected_hz: 440.0,
            pitch_confidence: 0.0,
            aperiodicity: 1.0,
            voiced: false,
            chroma_histogram: [0.0; 12],
            chroma_history: Vec::new(),
            chroma_position: 0,
            chroma_weight: 0.0,
            key_root: 0,
            key_minor: false,
            key_valid: false,
            key_confidence: 0.0,
            active_intervals: [0.0; ROBOTER_HARMONY_COUNT],
            voices: [RoboterVoice::new(); ROBOTER_VOICE_COUNT],
            delay: [Vec::new(), Vec::new()],
            write_position: 0,
            latency: 2_768,
            bypassed: false,
        }
    }

    fn analyse_pitch(&mut self) {
        let frame = std::array::from_fn(|index| {
            self.pitch_ring[(self.pitch_position + index) % ROBOTER_YIN_FRAME]
        });
        let analysis_rate = self.sample_rate / self.downsample_factor as f32;
        let estimate = yin_pitch(&frame, analysis_rate);
        self.pitch_confidence = estimate.confidence;
        self.aperiodicity = estimate.aperiodicity;
        self.voiced = estimate.voiced;
        if estimate.voiced {
            self.detected_hz = estimate.frequency;
            self.raw_midi = 69.0 + 12.0 * (estimate.frequency / 440.0).log2();
            if !self.smoothed_midi.is_finite() || (self.raw_midi - self.smoothed_midi).abs() >= 4.0
            {
                self.smoothed_midi = self.raw_midi
            } else {
                self.smoothed_midi += (self.raw_midi - self.smoothed_midi) * 0.35
            }
        }
        self.update_chroma();
        self.update_pitch_targets();
    }

    fn update_chroma(&mut self) {
        if self.chroma_history.is_empty() {
            return;
        }
        let old = self.chroma_history[self.chroma_position];
        if old.pitch_class < 12 {
            self.chroma_histogram[old.pitch_class as usize] =
                (self.chroma_histogram[old.pitch_class as usize] - old.weight).max(0.0);
            self.chroma_weight = (self.chroma_weight - old.weight).max(0.0)
        }
        let event = if self.voiced {
            let pitch_class = self.smoothed_midi.round().rem_euclid(12.0) as u8;
            let weight = self.pitch_confidence * (1.0 - self.aperiodicity).sqrt();
            self.chroma_histogram[pitch_class as usize] += weight;
            self.chroma_weight += weight;
            ChromaEvent {
                pitch_class,
                weight,
            }
        } else {
            ChromaEvent::EMPTY
        };
        self.chroma_history[self.chroma_position] = event;
        self.chroma_position = increment_wrap(self.chroma_position, self.chroma_history.len());

        self.refresh_key_estimate()
    }

    fn refresh_key_estimate(&mut self) {
        let distinct = self
            .chroma_histogram
            .iter()
            .filter(|weight| **weight > self.chroma_weight * 0.025)
            .count();
        let (root, minor, best_score, second_score) = match_key(&self.chroma_histogram);
        let evidence = (self.chroma_weight / 20.0).clamp(0.0, 1.0)
            * ((distinct as f32 - 2.0) / 3.0).clamp(0.0, 1.0);
        self.key_confidence = ((best_score - second_score) * 3.5).clamp(0.0, 1.0) * evidence;
        if !self.key_valid {
            if self.key_confidence >= 0.18 {
                self.key_root = root;
                self.key_minor = minor;
                self.key_valid = true
            }
        } else if root != self.key_root || minor != self.key_minor {
            let current_score =
                key_profile_score(&self.chroma_histogram, self.key_root, self.key_minor);
            if self.key_confidence >= 0.24 && best_score > current_score + 0.08 {
                self.key_root = root;
                self.key_minor = minor
            }
        }
    }

    fn update_pitch_targets(&mut self) {
        if !self.voiced {
            for voice in &mut self.voices {
                voice.target_ratio = 1.0
            }
            return;
        }
        let tonal = self.key_valid && self.key_confidence >= 0.18;
        let (lead_target, degree) = if tonal {
            nearest_scale_note(self.smoothed_midi, self.key_root, self.key_minor)
        } else {
            (self.smoothed_midi.round() as i32, 0)
        };
        let depth = if self.amount <= 0.5 {
            self.amount * 2.0
        } else {
            1.0
        };
        let lead_shift = (lead_target as f32 - self.smoothed_midi) * depth;
        self.voices[0].target_ratio = 2.0_f32.powf(lead_shift / 12.0).clamp(0.9, 1.1);

        for harmony in 0..ROBOTER_HARMONY_COUNT {
            let target = if tonal {
                diatonic_harmony_note(lead_target, degree, harmony, self.key_root, self.key_minor)
            } else {
                lead_target + chromatic_harmony_interval(harmony)
            };
            self.active_intervals[harmony] = (target - lead_target) as f32;
            self.voices[harmony + 1].target_ratio = 2.0_f32
                .powf((target as f32 - self.smoothed_midi) / 12.0)
                .clamp(0.45, 2.1)
        }
    }

    fn configure_voices(&mut self) {
        for harmony in 0..ROBOTER_HARMONY_COUNT {
            let (active, pan) = roboter_voice_layout(self.number, harmony);
            self.voices[harmony + 1].target_gain = if active { 1.0 } else { 0.0 };
            self.voices[harmony + 1].target_pan = pan
        }
    }
}
impl DspEffect for Roboter {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.downsample_factor = (sample_rate / 24_000.0).round().clamp(1.0, 8.0) as usize;
        let analysis_latency = ROBOTER_YIN_FRAME * self.downsample_factor;
        let maximum_period = (sample_rate / 80.0).ceil() as usize * 2;
        let maximum_extra_delay = (sample_rate * 0.015).ceil() as usize;
        self.latency = analysis_latency + maximum_period + maximum_extra_delay;
        let delay_length = self.latency + maximum_period + maximum_extra_delay + MAX_BLOCK_SIZE + 8;
        self.delay = [vec![0.0; delay_length], vec![0.0; delay_length]];
        let history_length = ((sample_rate * 6.0) / ROBOTER_ANALYSIS_HOP as f32)
            .round()
            .max(1.0) as usize;
        self.chroma_history = vec![ChromaEvent::EMPTY; history_length];
        self.reset();
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        let retime_seconds = if self.amount <= 0.5 {
            0.04
        } else {
            0.04 * (1.0 - (self.amount - 0.5) * 2.0).max(0.0)
        };
        let harmony_normalization = if self.number == 0 {
            0.0
        } else {
            1.0 / (self.number as f32).sqrt()
        };
        for frame in 0..frames {
            let input = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let mono = (input[0] + input[1]) * 0.5;
            self.delay[0][self.write_position] = input[0];
            self.delay[1][self.write_position] = input[1];

            self.downsample_sum += mono;
            self.downsample_count += 1;
            self.samples_since_analysis += 1;
            if self.downsample_count >= self.downsample_factor {
                self.pitch_ring[self.pitch_position] =
                    self.downsample_sum / self.downsample_count as f32;
                self.pitch_position = increment_wrap(self.pitch_position, ROBOTER_YIN_FRAME);
                self.pitch_filled = (self.pitch_filled + 1).min(ROBOTER_YIN_FRAME);
                self.downsample_sum = 0.0;
                self.downsample_count = 0
            }
            if self.samples_since_analysis >= ROBOTER_ANALYSIS_HOP
                && self.pitch_filled == ROBOTER_YIN_FRAME
            {
                self.samples_since_analysis = 0;
                self.analyse_pitch()
            }

            let delayed = std::array::from_fn(|channel| {
                fractional_read(
                    &self.delay[channel],
                    self.write_position as f32 - self.latency as f32,
                )
            });
            let period = if self.voiced {
                self.sample_rate / self.detected_hz.clamp(80.0, 1_000.0)
            } else {
                self.sample_rate / 220.0
            };
            let mut output = if self.voiced && self.amount > 0.0001 {
                self.voices[0].render(
                    &self.delay,
                    self.write_position,
                    self.latency,
                    period,
                    self.sample_rate,
                    0,
                    retime_seconds,
                )
            } else {
                delayed
            };

            for harmony in 0..ROBOTER_HARMONY_COUNT {
                let voice_index = harmony + 1;
                self.voices[voice_index].advance_mix(self.sample_rate);
                if !self.voiced || self.voices[voice_index].gain <= 0.000_01 {
                    continue;
                }
                let mut shifted = self.voices[voice_index].render(
                    &self.delay,
                    self.write_position,
                    self.latency,
                    period,
                    self.sample_rate,
                    voice_index,
                    0.004,
                );
                if harmony == 0 {
                    shifted = self.voices[voice_index].mono_bass(shifted, self.sample_rate)
                }
                let mut pan = self.voices[voice_index].pan;
                if harmony == 3 && self.detected_hz < 180.0 {
                    pan *= (self.detected_hz / 180.0).clamp(0.25, 1.0)
                }
                let left_pan = if pan > 0.0 { 1.0 - pan } else { 1.0 };
                let right_pan = if pan < 0.0 { 1.0 + pan } else { 1.0 };
                let gain = self.voices[voice_index].gain * harmony_normalization;
                output[0] += shifted[0] * left_pan * gain;
                output[1] += shifted[1] * right_pan * gain
            }
            buffer.channels[0][frame] = denormal(output[0]);
            buffer.channels[1][frame] = denormal(output[1]);
            self.write_position = increment_wrap(self.write_position, self.delay[0].len())
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "amount" | "depth" | "robot" => self.amount = value.clamp(0.0, 1.0),
            "number" | "voices" => {
                self.number = value.round().clamp(0.0, 5.0) as usize;
                self.configure_voices()
            }
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.downsample_sum = 0.0;
        self.downsample_count = 0;
        self.samples_since_analysis = 0;
        self.pitch_ring = [0.0; ROBOTER_YIN_FRAME];
        self.pitch_position = 0;
        self.pitch_filled = 0;
        self.raw_midi = 69.0;
        self.smoothed_midi = 69.0;
        self.detected_hz = 440.0;
        self.pitch_confidence = 0.0;
        self.aperiodicity = 1.0;
        self.voiced = false;
        self.chroma_histogram = [0.0; 12];
        self.chroma_history.fill(ChromaEvent::EMPTY);
        self.chroma_position = 0;
        self.chroma_weight = 0.0;
        self.key_root = 0;
        self.key_minor = false;
        self.key_valid = false;
        self.key_confidence = 0.0;
        self.active_intervals = [0.0; ROBOTER_HARMONY_COUNT];
        self.voices = [RoboterVoice::new(); ROBOTER_VOICE_COUNT];
        self.voices[0].gain = 1.0;
        self.voices[0].target_gain = 1.0;
        self.configure_voices();
        for channel in &mut self.delay {
            channel.fill(0.0)
        }
        self.write_position = 0
    }
    fn tail_samples(&self) -> usize {
        (self.sample_rate * (0.015 + 0.05)).ceil() as usize
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        let mut metrics = [0.0; DISTORTION_SPECTRUM_BINS];
        metrics[0] = if self.voiced {
            self.smoothed_midi
        } else {
            -1.0
        };
        metrics[1] = self.key_root as f32;
        metrics[2] = if self.key_valid && self.key_confidence >= 0.18 {
            if self.key_minor {
                1.0
            } else {
                0.0
            }
        } else {
            2.0
        };
        metrics[3] = self.key_confidence;
        metrics[4] = if self.voiced { 1.0 } else { 0.0 };
        metrics[5] = self.pitch_confidence;
        metrics[6] = self.number as f32;
        metrics[7..(7 + ROBOTER_HARMONY_COUNT)].copy_from_slice(&self.active_intervals);
        Some(metrics)
    }
}

fn yin_pitch(frame: &[f32; ROBOTER_YIN_FRAME], sample_rate: f32) -> PitchEstimate {
    let minimum_lag = (sample_rate / 1_000.0).floor().max(2.0) as usize;
    let maximum_lag = ((sample_rate / 80.0).ceil() as usize)
        .min(ROBOTER_YIN_MAX_LAG)
        .min(ROBOTER_YIN_FRAME / 2 - 1);
    let comparison_samples = ROBOTER_YIN_FRAME - maximum_lag;
    let rms =
        (frame.iter().map(|sample| sample * sample).sum::<f32>() / ROBOTER_YIN_FRAME as f32).sqrt();
    if rms < 0.0015 || maximum_lag <= minimum_lag + 2 {
        return PitchEstimate::default();
    }
    let mut raw_difference = [0.0_f32; ROBOTER_YIN_MAX_LAG + 1];
    for lag in 1..=maximum_lag {
        let mut sum = 0.0;
        for index in 0..comparison_samples {
            let delta = frame[index] - frame[index + lag];
            sum += delta * delta
        }
        raw_difference[lag] = sum
    }
    let mut difference = raw_difference;
    let mut running_sum = 0.0;
    difference[0] = 1.0;
    for lag in 1..=maximum_lag {
        running_sum += difference[lag];
        difference[lag] = difference[lag] * lag as f32 / running_sum.max(1e-12)
    }
    let mut selected = 0;
    for lag in minimum_lag..maximum_lag {
        if difference[lag] < 0.15 && difference[lag] <= difference[lag + 1] {
            selected = lag;
            break;
        }
    }
    if selected == 0 {
        selected = (minimum_lag..=maximum_lag)
            .min_by(|left, right| difference[*left].total_cmp(&difference[*right]))
            .unwrap_or(0)
    }
    if selected == 0 {
        return PitchEstimate::default();
    }
    let aperiodicity = difference[selected].clamp(0.0, 1.0);
    let mut lower = (selected as f32 - 1.0).max(minimum_lag as f32);
    let mut upper = (selected as f32 + 1.0).min(maximum_lag as f32);
    const GOLDEN: f32 = 0.618_034;
    let mut left = upper - (upper - lower) * GOLDEN;
    let mut right = lower + (upper - lower) * GOLDEN;
    let mut left_value = yin_fractional_difference(frame, left, comparison_samples);
    let mut right_value = yin_fractional_difference(frame, right, comparison_samples);
    for _ in 0..12 {
        if left_value <= right_value {
            upper = right;
            right = left;
            right_value = left_value;
            left = upper - (upper - lower) * GOLDEN;
            left_value = yin_fractional_difference(frame, left, comparison_samples)
        } else {
            lower = left;
            left = right;
            left_value = right_value;
            right = lower + (upper - lower) * GOLDEN;
            right_value = yin_fractional_difference(frame, right, comparison_samples)
        }
    }
    let lag = (lower + upper) * 0.5;
    let frequency = sample_rate / lag.max(1.0);
    let confidence = (1.0 - aperiodicity).clamp(0.0, 1.0);
    PitchEstimate {
        frequency,
        confidence,
        aperiodicity,
        voiced: (80.0..=1_000.0).contains(&frequency) && confidence >= 0.68,
    }
}

fn yin_fractional_difference(frame: &[f32; ROBOTER_YIN_FRAME], lag: f32, samples: usize) -> f32 {
    let mut sum = 0.0;
    let safe_end = samples
        .saturating_sub(2)
        .min(ROBOTER_YIN_FRAME.saturating_sub(lag.ceil() as usize + 2));
    for index in 1..safe_end {
        let position = index as f32 + lag;
        let base = position.floor() as usize;
        let fraction = position - base as f32;
        let shifted = frame[base] + (frame[base + 1] - frame[base]) * fraction;
        let delta = frame[index] - shifted;
        sum += delta * delta
    }
    sum
}

fn key_profile_score(histogram: &[f32; 12], root: usize, minor: bool) -> f32 {
    let profile = if minor {
        &MINOR_PROFILE
    } else {
        &MAJOR_PROFILE
    };
    let histogram_mean = histogram.iter().sum::<f32>() / 12.0;
    let profile_mean = profile.iter().sum::<f32>() / 12.0;
    let mut numerator = 0.0;
    let mut histogram_energy = 0.0;
    let mut profile_energy = 0.0;
    for pitch_class in 0..12 {
        let observed = histogram[(root + pitch_class) % 12] - histogram_mean;
        let expected = profile[pitch_class] - profile_mean;
        numerator += observed * expected;
        histogram_energy += observed * observed;
        profile_energy += expected * expected
    }
    numerator / (histogram_energy * profile_energy).sqrt().max(1e-9)
}

fn match_key(histogram: &[f32; 12]) -> (usize, bool, f32, f32) {
    let mut best = (0, false, f32::NEG_INFINITY);
    let mut second = f32::NEG_INFINITY;
    for minor in [false, true] {
        for root in 0..12 {
            let score = key_profile_score(histogram, root, minor);
            if score > best.2 {
                second = best.2;
                best = (root, minor, score)
            } else if score > second {
                second = score
            }
        }
    }
    (best.0, best.1, best.2, second)
}

fn nearest_scale_note(midi: f32, root: usize, minor: bool) -> (i32, usize) {
    let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    let center = midi.round() as i32;
    let mut best_note = center;
    let mut best_degree = 0;
    let mut best_distance = f32::MAX;
    for note in (center - 7)..=(center + 7) {
        let relative = (note - root as i32).rem_euclid(12);
        if let Some(degree) = scale.iter().position(|pitch| *pitch == relative) {
            let distance = (note as f32 - midi).abs();
            if distance < best_distance {
                best_note = note;
                best_degree = degree;
                best_distance = distance
            }
        }
    }
    (best_note, best_degree)
}

fn diatonic_harmony_note(
    lead_note: i32,
    lead_degree: usize,
    harmony: usize,
    root: usize,
    minor: bool,
) -> i32 {
    if harmony == 0 {
        return lead_note - 12;
    }
    let mut steps = match harmony {
        1 => -2,
        2 => 2,
        3 => -5,
        _ => 4,
    };
    if !minor && lead_degree == 6 && harmony == 4 {
        steps = 5
    }
    let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    let tonic = lead_note - scale[lead_degree] - root as i32 + root as i32;
    let target_degree = lead_degree as i32 + steps;
    let octave = target_degree.div_euclid(7);
    let degree = target_degree.rem_euclid(7) as usize;
    tonic + octave * 12 + scale[degree]
}

fn chromatic_harmony_interval(harmony: usize) -> i32 {
    [-12, -4, 4, -9, 7][harmony.min(ROBOTER_HARMONY_COUNT - 1)]
}

fn roboter_voice_layout(number: usize, harmony: usize) -> (bool, f32) {
    const ACTIVE: [[bool; ROBOTER_HARMONY_COUNT]; 6] = [
        [false, false, false, false, false],
        [true, false, false, false, false],
        [false, true, true, false, false],
        [true, true, true, false, false],
        [false, true, true, true, true],
        [true, true, true, true, true],
    ];
    const PAN: [[f32; ROBOTER_HARMONY_COUNT]; 6] = [
        [0.0; 5],
        [0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, -0.60, 0.60, 0.0, 0.0],
        [0.0, -0.75, 0.75, 0.0, 0.0],
        [0.0, -0.40, 0.40, -0.85, 0.85],
        [0.0, -0.45, 0.45, -0.90, 0.90],
    ];
    let number = number.min(5);
    (ACTIVE[number][harmony], PAN[number][harmony])
}
