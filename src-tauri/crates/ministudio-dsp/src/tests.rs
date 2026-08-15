use super::*;
#[test]
fn smoother_converges() {
    let mut s = Smoother::new(0.0, 48000.0, 0.01);
    s.set_target(1.0);
    for _ in 0..480 {
        s.next();
    }
    assert!((s.current - 0.632).abs() < 0.01)
}
#[test]
fn waveshaper_reports_latency() {
    let mut w = Waveshaper::new();
    w.oversample = 4;
    assert_eq!(w.latency_samples(), 2)
}
#[test]
fn waveshaper_modes_are_finite_bounded_and_odd() {
    for curve in [Curve::SoftClip, Curve::HardClip, Curve::Sine] {
        for x in [-12.0_f32, -2.0, -1.0, -0.25, 0.0, 0.25, 1.0, 2.0, 12.0] {
            let y = shape(curve, x);
            assert!(y.is_finite());
            assert!(y.abs() <= 1.000_001);
            assert!((y + shape(curve, -x)).abs() < 1e-6)
        }
    }
}
#[test]
fn sine_shaper_reaches_rails_without_foldback() {
    assert!((shape(Curve::Sine, 1.0) - 1.0).abs() < 1e-6);
    assert!((shape(Curve::Sine, 8.0) - 1.0).abs() < 1e-6);
    assert!((shape(Curve::Sine, -8.0) + 1.0).abs() < 1e-6)
}
#[test]
fn eq_response_is_finite() {
    let mut e = ParametricEq::new();
    e.prepare(48000.0, 256, 2);
    assert!(e
        .response(256)
        .unwrap()
        .combined_db
        .iter()
        .all(|v| v.is_finite()))
}
#[test]
fn every_eq_starts_with_an_enabled_low_cut() {
    for effect in [ParametricEq::new(), ParametricEq::new_eight()] {
        assert!(effect.bands[0].enabled);
        assert!(matches!(effect.bands[0].kind, FilterKind::HighPass));
    }
}
#[test]
fn eight_band_eq_response_is_finite() {
    let mut effect = ParametricEq::new_eight();
    effect.prepare(48_000.0, 256, 2);
    effect.set_param("band7.enabled", 1.0);
    effect.set_param("band7.type", 4.0);
    assert_eq!(effect.bands.len(), 8);
    assert!(effect
        .response(256)
        .unwrap()
        .combined_db
        .iter()
        .all(|value| value.is_finite()))
}
#[test]
fn utility_mono_removes_pure_side_signal() {
    let mut effect = Utility::new();
    effect.set_param("mono", 1.0);
    let mut buffer = AudioBuffer::new();
    buffer.channels[0][0] = 1.0;
    buffer.channels[1][0] = -1.0;
    effect.process(&[], &mut buffer, 1);
    assert!(buffer.channels[0][0].abs() < 1e-6);
    assert!(buffer.channels[1][0].abs() < 1e-6)
}
#[test]
fn compressor_uses_external_sidechain_detector() {
    let mut effect = Compressor::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, 2);
    effect.set_param("threshold", -30.0);
    effect.set_param("ratio", 20.0);
    effect.set_param("attackMs", 1.0);
    let mut buffer = AudioBuffer::new();
    let mut detector = AudioBuffer::new();
    buffer.channels[0][..MAX_BLOCK_SIZE].fill(0.25);
    buffer.channels[1][..MAX_BLOCK_SIZE].fill(0.25);
    detector.channels[0][..MAX_BLOCK_SIZE].fill(1.0);
    detector.channels[1][..MAX_BLOCK_SIZE].fill(1.0);
    effect.process_with_sidechain(&[], &mut buffer, Some(&detector), MAX_BLOCK_SIZE);
    assert!(buffer.channels[0][MAX_BLOCK_SIZE - 1] < 0.1)
}
#[test]
fn multiband_compressor_processes_finite_audio() {
    let mut effect = MultibandCompressor::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let mut buffer = AudioBuffer::new();
    for frame in 0..MAX_BLOCK_SIZE {
        let sample = (2.0 * PI * 440.0 * frame as f32 / 48_000.0).sin() * 0.25;
        buffer.channels[0][frame] = sample;
        buffer.channels[1][frame] = sample;
    }
    effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    assert!(buffer
        .channels
        .iter()
        .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        .all(|sample| sample.is_finite()));
    let levels = effect.multiband_levels().expect("multiband meter");
    assert!(levels.iter().flatten().all(|level| level.is_finite()));
    assert!(levels.iter().flatten().any(|level| *level > 0.0))
}
#[test]
fn multiband_low_split_respects_requested_range() {
    let mut effect = MultibandCompressor::new();
    effect.set_param("splitLow", 500.0);
    assert_eq!(effect.split_low, 150.0);
    effect.set_param("splitLow", 1.0);
    assert_eq!(effect.split_low, 20.0)
}
#[test]
fn distortion_modes_process_finite_audio_and_publish_fft() {
    for mode in 0..=4 {
        let mut effect = Distortion::new();
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        for band in ["lowMode", "midMode", "highMode"] {
            effect.set_param(band, mode as f32)
        }
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let sample = (2.0 * PI * 440.0 * frame as f32 / 48_000.0).sin() * 0.25;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        assert!(buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            .all(|sample| sample.is_finite()));
        let spectrum = effect.effect_spectrum().expect("distortion FFT");
        assert!(spectrum.iter().all(|value| value.is_finite()));
        assert!(spectrum.iter().any(|value| *value > 0.0));
    }
}
#[test]
fn distortion_crossovers_clamp_and_disable_at_edges() {
    let mut effect = Distortion::new();
    effect.set_param("splitLow", -100.0);
    effect.set_param("splitHigh", 40_000.0);
    assert_eq!(effect.split_low, 20.0);
    assert_eq!(effect.split_high, 20_000.0);
    effect.set_param("splitLow", 10_000.0);
    assert!(effect.split_low <= effect.split_high / 1.05)
}
#[test]
fn distortion_exposes_band_bypass_and_oversampling_latency() {
    let mut effect = Distortion::new();
    assert_eq!(effect.latency_samples(), 2);
    effect.set_param("oversample", 2.0);
    assert_eq!(effect.latency_samples(), 4);
    effect.set_param("oversample", 1.0);
    assert_eq!(effect.latency_samples(), 0);
    effect.set_param("midEnabled", 0.0);
    assert!(!effect.band_enabled[1]);
    effect.set_param("midEnabled", 1.0);
    assert!(effect.band_enabled[1]);
}
#[test]
fn every_builtin_effect_has_an_audited_latency_contract() {
    let zero_latency = [
        "builtin:eq",
        "builtin:eq8",
        "builtin:compressor",
        "builtin:multiband-compressor",
        "builtin:utility",
        "builtin:delay",
        "builtin:reverb",
        "builtin:disperser",
        "builtin:vocoder",
        "builtin:lfo-tremolo",
        "builtin:upward-compressor",
    ];
    for kind in zero_latency {
        let effect = create_builtin_effect(kind, &HashMap::new(), false, 48_000.0).expect(kind);
        assert_eq!(
            effect.latency_samples(),
            0,
            "{kind} must remain truly zero-latency"
        );
    }
    for (kind, expected) in [
        ("builtin:waveshaper", 2),
        ("builtin:distortion", 2),
        ("builtin:clipper", 2),
        ("builtin:mastering-limiter", 240),
        ("builtin:resonator", COLORIZER_FFT_SIZE),
    ] {
        let effect = create_builtin_effect(kind, &HashMap::new(), false, 48_000.0).expect(kind);
        assert_eq!(
            effect.latency_samples(),
            expected,
            "latency contract changed for {kind}"
        );
    }
    let roboter = create_builtin_effect("builtin:roboter", &HashMap::new(), false, 48_000.0)
        .expect("roboter");
    assert!(
        roboter.latency_samples() > 0,
        "pitch analysis must report its lookahead"
    );
}
#[test]
fn disperser_allpass_section_has_unity_magnitude() {
    let mut section = AllpassSection::new();
    section.configure(3_050.0, 1.2, 48_000.0);
    for hz in [20.0_f64, 100.0, 1_000.0, 3_050.0, 10_000.0, 20_000.0] {
        let omega = 2.0 * std::f64::consts::PI * hz / 48_000.0;
        let (sin_1, cos_1) = omega.sin_cos();
        let (sin_2, cos_2) = (2.0 * omega).sin_cos();
        let numerator_real = section.b0 + section.b1 * cos_1 + section.b2 * cos_2;
        let numerator_imag = -section.b1 * sin_1 - section.b2 * sin_2;
        let denominator_real = 1.0 + section.a1 * cos_1 + section.a2 * cos_2;
        let denominator_imag = -section.a1 * sin_1 - section.a2 * sin_2;
        let magnitude = (numerator_real * numerator_real + numerator_imag * numerator_imag).sqrt()
            / (denominator_real * denominator_real + denominator_imag * denominator_imag).sqrt();
        assert!(
            (magnitude - 1.0).abs() < 1e-10,
            "{hz} Hz magnitude was {magnitude}"
        )
    }
}
#[test]
fn disperser_amount_controls_order_and_output_stays_finite() {
    let mut effect = Disperser::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    effect.set_param("amount", 1.0);
    effect.set_param("frequency", 80.0);
    effect.set_param("pinch", 1.0);
    assert_eq!(effect.stages, MAX_DISPERSER_STAGES);
    let mut buffer = AudioBuffer::new();
    buffer.channels[0][0] = 1.0;
    buffer.channels[1][0] = 1.0;
    effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    assert!(buffer
        .channels
        .iter()
        .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        .all(|sample| sample.is_finite()));

    effect.set_param("amount", 0.0);
    let mut dry = AudioBuffer::new();
    dry.channels[0][0] = 0.25;
    dry.channels[1][0] = -0.25;
    effect.process(&[], &mut dry, 1);
    assert_eq!(dry.channels[0][0], 0.25);
    assert_eq!(dry.channels[1][0], -0.25)
}
#[test]
fn eq_and_disperser_publish_finite_spectra() {
    let mut effects: [Box<dyn DspEffect>; 2] = [
        Box::new(ParametricEq::new_eight()),
        Box::new(Disperser::new()),
    ];
    for effect in &mut effects {
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let sample = (2.0 * PI * 880.0 * frame as f32 / 48_000.0).sin() * 0.2;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        let spectrum = effect.effect_spectrum().expect("effect spectrum");
        assert!(spectrum.iter().all(|value| value.is_finite()));
        assert!(spectrum.iter().any(|value| *value > 0.0));
    }
}
#[test]
fn optimized_delay_reader_matches_wrapped_reference() {
    let line = (0..17)
        .map(|index| (index as f32 * 0.37).sin())
        .collect::<Vec<_>>();
    for write in [0, 1, 8, 16] {
        for delay in [1.0_f32, 2.25, 8.5, 13.0] {
            let position = (write as f32 - delay).rem_euclid(line.len() as f32);
            let base = position.floor() as usize;
            let fraction = position - base as f32;
            let reference = catmull(
                line[(base + line.len() - 1) % line.len()],
                line[base],
                line[(base + 1) % line.len()],
                line[(base + 2) % line.len()],
                fraction,
            );
            assert!((cubic_delay_read(&line, write, delay) - reference).abs() < 1e-6)
        }
    }
}
#[test]
fn sparse_oversampling_fir_matches_full_reference() {
    const COEFFICIENTS: [f32; 15] = [
        -0.001682, 0.0, 0.010703, 0.0, -0.049012, 0.0, 0.289991, 0.499999, 0.289991, 0.0,
        -0.049012, 0.0, 0.010703, 0.0, -0.001682,
    ];
    let history = std::array::from_fn(|index| (index as f32 * 0.61).cos());
    for position in 0..15 {
        let reference = COEFFICIENTS
            .iter()
            .enumerate()
            .map(|(offset, coefficient)| history[(position + 15 - offset) % 15] * coefficient)
            .sum::<f32>();
        assert!((fir_read(&history, position) - reference).abs() < 1e-6)
    }
}
#[test]
fn clean_limiter_respects_sample_ceiling_and_reports_loudness() {
    let mut limiter = MasteringLimiter::new();
    limiter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    limiter.set_param("outputDb", -1.0);
    let ceiling = db_to_gain(-1.0) + 1e-5;
    let mut saw_audio = false;
    let mut reconstruction = [[0.0_f32; 4]; MAX_CHANNELS];
    for block in 0..220 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let sample_index = block * MAX_BLOCK_SIZE + frame;
            let sample = (2.0 * PI * 997.0 * sample_index as f32 / 48_000.0).sin() * 1.8;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        limiter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for frame in 0..MAX_BLOCK_SIZE {
            for channel in 0..MAX_CHANNELS {
                let sample = buffer.channels[channel][frame];
                assert!(sample.is_finite());
                assert!(sample.abs() <= ceiling);
                reconstruction[channel].rotate_left(1);
                reconstruction[channel][3] = sample;
                let reconstructed = interpolated_peak(reconstruction[channel], 4);
                assert!(
                    reconstructed <= ceiling + 0.001,
                    "true peak {reconstructed} exceeded ceiling {ceiling}"
                );
                saw_audio |= sample.abs() > 0.1;
            }
        }
    }
    assert!(saw_audio);
    let metrics = limiter.limiter_metrics().expect("limiter meters");
    assert!(metrics.iter().all(|value| value.is_finite()));
    assert!(metrics[2] > 0.0);
    assert!(metrics[4] > -120.0)
}
#[test]
fn vocoder_uses_external_modulator_and_stays_finite() {
    let mut vocoder = Vocoder::new();
    vocoder.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    vocoder.set_param("source", 0.0);
    let mut energy = 0.0_f32;
    for block in 0..24 {
        let mut carrier = AudioBuffer::new();
        let mut modulator = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let index = block * MAX_BLOCK_SIZE + frame;
            let c = (2.0 * PI * 220.0 * index as f32 / 48_000.0).sin() * 0.7;
            let m = (2.0 * PI * 440.0 * index as f32 / 48_000.0).sin() * 0.8;
            carrier.channels[0][frame] = c;
            carrier.channels[1][frame] = c;
            modulator.channels[0][frame] = m;
            modulator.channels[1][frame] = m;
        }
        vocoder.process_with_sidechain(&[], &mut carrier, Some(&modulator), MAX_BLOCK_SIZE);
        for sample in carrier
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        {
            assert!(sample.is_finite());
            energy += sample * sample;
        }
    }
    assert!(energy > 1e-6)
}
#[test]
fn tremolo_modulates_volume_and_pan_without_non_finite_samples() {
    let mut tremolo = LfoTremolo::new();
    tremolo.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    tremolo.set_param("volumeDepth", 1.0);
    tremolo.set_param("panDepth", 1.0);
    tremolo.set_param("rateHz", 10.0);
    let mut minimum = f32::MAX;
    let mut maximum = f32::MIN;
    for _ in 0..32 {
        let mut buffer = AudioBuffer::new();
        buffer.channels[0][..MAX_BLOCK_SIZE].fill(0.5);
        buffer.channels[1][..MAX_BLOCK_SIZE].fill(0.5);
        tremolo.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for sample in buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        {
            assert!(sample.is_finite());
            minimum = minimum.min(*sample);
            maximum = maximum.max(*sample);
        }
    }
    assert!(maximum - minimum > 0.2)
}
#[test]
fn clipper_limits_peaks_and_publishes_realtime_flow() {
    let mut clipper = Clipper::new();
    clipper.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    clipper.set_param("inputDb", 0.0);
    clipper.set_param("outputDb", -1.0);
    let mut maximum = 0.0_f32;
    for block in 0..8 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let index = block * MAX_BLOCK_SIZE + frame;
            let sample = (2.0 * PI * 997.0 * index as f32 / 48_000.0).sin() * 2.0;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        clipper.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for sample in buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        {
            assert!(sample.is_finite());
            maximum = maximum.max(sample.abs())
        }
    }
    assert!(maximum <= 1.0);
    let flow = clipper.effect_spectrum().expect("clipper peak flow");
    assert!(flow.iter().all(|value| value.is_finite()));
    assert!(flow.iter().any(|value| *value > 1.0));
    assert_eq!(clipper_curve(4.0, 0.0), 1.0)
}
#[test]
fn upward_compressor_lifts_quiet_material_without_non_finite_samples() {
    let mut compressor = UpwardCompressor::new();
    compressor.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    compressor.set_param("threshold", -24.0);
    compressor.set_param("rangeDb", 18.0);
    compressor.set_param("attackMs", 1.0);
    let mut input_energy = 0.0_f32;
    let mut output_energy = 0.0_f32;
    for block in 0..40 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let index = block * MAX_BLOCK_SIZE + frame;
            let sample = (2.0 * PI * 220.0 * index as f32 / 48_000.0).sin() * 0.01;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
            if block > 20 {
                input_energy += sample * sample * 2.0
            }
        }
        compressor.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        if block > 20 {
            for sample in buffer
                .channels
                .iter()
                .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
            {
                assert!(sample.is_finite());
                output_energy += sample * sample
            }
        }
    }
    assert!(output_energy > input_energy * 2.0)
}
#[test]
fn roboter_yin_detects_sine_and_saw_within_one_cent() {
    let sample_rate = 24_000.0;
    for frequency in [82.41_f32, 440.0, 987.77] {
        for saw in [false, true] {
            let frame = std::array::from_fn(|index| {
                let phase = frequency * index as f32 / sample_rate;
                if saw {
                    (phase.fract() * 2.0 - 1.0) * 0.35
                } else {
                    (2.0 * PI * phase).sin() * 0.35
                }
            });
            let estimate = yin_pitch(&frame, sample_rate);
            let error_cents = 1200.0 * (estimate.frequency / frequency).log2();
            assert!(
                estimate.voiced,
                "frequency={frequency} saw={saw} confidence={}",
                estimate.confidence
            );
            let tolerance = if (frequency - 440.0).abs() < 0.01 {
                1.0
            } else {
                3.0
            };
            assert!(
                error_cents.abs() <= tolerance,
                "frequency={frequency} saw={saw} pitch={} error={error_cents} cents",
                estimate.frequency
            )
        }
    }
}
#[test]
fn roboter_key_profiles_identify_transposed_major_and_minor() {
    for (root, minor) in [(0, false), (7, false), (9, true), (3, true)] {
        let profile = if minor { MINOR_PROFILE } else { MAJOR_PROFILE };
        let mut histogram = [0.0_f32; 12];
        for pitch_class in 0..12 {
            histogram[(root + pitch_class) % 12] = profile[pitch_class]
        }
        let (detected_root, detected_minor, best, second) = match_key(&histogram);
        assert_eq!((detected_root, detected_minor), (root, minor));
        assert!(best > second)
    }
}
#[test]
fn roboter_key_hysteresis_rejects_marginal_mid_phrase_change() {
    let mut roboter = Roboter::new();
    roboter.key_valid = true;
    roboter.key_root = 0;
    roboter.key_minor = false;
    roboter.chroma_weight = 100.0;
    let mut marginal = None;
    for step in 50..100 {
        let blend = step as f32 / 100.0;
        let histogram = std::array::from_fn(|pitch_class| {
            MAJOR_PROFILE[pitch_class] * (1.0 - blend)
                + MAJOR_PROFILE[(pitch_class + 12 - 7) % 12] * blend
        });
        let (root, minor, best, _) = match_key(&histogram);
        let current = key_profile_score(&histogram, 0, false);
        if root == 7 && !minor && best <= current + 0.08 {
            marginal = Some(histogram);
            break;
        }
    }
    let histogram = marginal.expect("a marginal C-to-G candidate");
    roboter.chroma_histogram = histogram;
    roboter.refresh_key_estimate();
    assert_eq!((roboter.key_root, roboter.key_minor), (0, false));

    roboter.chroma_histogram =
        std::array::from_fn(|pitch_class| MAJOR_PROFILE[(pitch_class + 12 - 7) % 12]);
    roboter.refresh_key_estimate();
    assert_eq!((roboter.key_root, roboter.key_minor), (7, false))
}
#[test]
fn roboter_diatonic_intervals_follow_scale_and_leading_tone_exception() {
    for root in 0..12 {
        for minor in [false, true] {
            let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
            let tonic = 60 + root as i32;
            for degree in 0..7 {
                let lead = tonic + scale[degree];
                let down_third = diatonic_harmony_note(lead, degree, 1, root, minor);
                let up_third = diatonic_harmony_note(lead, degree, 2, root, minor);
                assert!([3, 4].contains(&(lead - down_third)));
                assert!([3, 4].contains(&(up_third - lead)));
                for harmony in 1..ROBOTER_HARMONY_COUNT {
                    let note = diatonic_harmony_note(lead, degree, harmony, root, minor);
                    assert!(scale.contains(&((note - root as i32).rem_euclid(12))))
                }
            }
            if !minor {
                let leading = tonic + MAJOR_SCALE[6];
                let upper = diatonic_harmony_note(leading, 6, 4, root, false);
                assert_eq!(upper - leading, 8)
            }
        }
    }
}
#[test]
fn roboter_reports_and_realizes_common_dry_latency() {
    let mut roboter = Roboter::new();
    roboter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    roboter.set_param("amount", 0.0);
    let expected = roboter.latency_samples();
    let mut observed = None;
    let blocks = expected.div_ceil(MAX_BLOCK_SIZE) + 2;
    for block in 0..blocks {
        let mut buffer = AudioBuffer::new();
        if block == 0 {
            buffer.channels[0][0] = 1.0;
            buffer.channels[1][0] = 1.0
        }
        roboter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for frame in 0..MAX_BLOCK_SIZE {
            if buffer.channels[0][frame].abs() > 0.9 {
                observed = Some(block * MAX_BLOCK_SIZE + frame);
                break;
            }
        }
    }
    assert_eq!(observed, Some(expected));
    assert!(roboter.tail_samples() >= (48_000.0 * 0.06) as usize)
}
#[test]
fn roboter_harmony_changes_fade_and_processing_stays_finite() {
    let mut roboter = Roboter::new();
    roboter.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    roboter.set_param("amount", 1.0);
    let mut energy = 0.0_f32;
    let mut maximum_jump = 0.0_f32;
    let mut previous = [0.0_f32; MAX_CHANNELS];
    let delay_capacity = roboter.delay[0].capacity();
    let history_capacity = roboter.chroma_history.capacity();
    for block in 0..72 {
        if block == 32 {
            roboter.set_param("number", 5.0)
        }
        if block == 56 {
            roboter.set_param("number", 0.0)
        }
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            let index = block * MAX_BLOCK_SIZE + frame;
            let sample = (2.0 * PI * 445.0 * index as f32 / 48_000.0).sin() * 0.3;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        roboter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for channel in 0..MAX_CHANNELS {
            for sample in &buffer.channels[channel][..MAX_BLOCK_SIZE] {
                assert!(sample.is_finite());
                energy += sample * sample;
                maximum_jump = maximum_jump.max((*sample - previous[channel]).abs());
                previous[channel] = *sample
            }
        }
    }
    assert!(energy > 1.0);
    assert!(roboter.voiced);
    assert!((roboter.voices[0].target_ratio - 1.0).abs() > 0.001);
    assert!(maximum_jump < 0.7, "sample discontinuity {maximum_jump}");
    assert_eq!(roboter.delay[0].capacity(), delay_capacity);
    assert_eq!(roboter.chroma_history.capacity(), history_capacity)
}
#[test]
fn colorizer_reports_and_realizes_exact_latency() {
    let mut effect = Colorizer::new();
    effect.set_param("mix", 0.0);
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let mut observed = None;
    for block in 0..6 {
        let mut buffer = AudioBuffer::new();
        if block == 0 {
            buffer.channels[0][0] = 1.0;
            buffer.channels[1][0] = 1.0;
        }
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for frame in 0..MAX_BLOCK_SIZE {
            if buffer.channels[0][frame].abs() > 0.99 {
                observed = Some(block * MAX_BLOCK_SIZE + frame);
                break;
            }
        }
    }
    assert_eq!(effect.latency_samples(), COLORIZER_FFT_SIZE);
    assert_eq!(observed, Some(COLORIZER_FFT_SIZE))
}
#[test]
fn colorizer_hann_cola_reconstructs_below_minus_60_db() {
    let mut effect = Colorizer::new();
    effect.set_param("depth", 0.0);
    effect.set_param("decay", 0.0);
    effect.set_param("mix", 1.0);
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let total = COLORIZER_FFT_SIZE + COLORIZER_HOP_SIZE * 12;
    let input: Vec<f32> = (0..total)
        .map(|sample| {
            let time = sample as f32 / 48_000.0;
            (2.0 * PI * 173.0 * time).sin() * 0.23
                + (2.0 * PI * 997.0 * time).sin() * 0.17
                + (2.0 * PI * 4_123.0 * time).sin() * 0.09
        })
        .collect();
    let mut output = Vec::with_capacity(total);
    for start in (0..total).step_by(MAX_BLOCK_SIZE) {
        let frames = (total - start).min(MAX_BLOCK_SIZE);
        let mut buffer = AudioBuffer::new();
        buffer.channels[0][..frames].copy_from_slice(&input[start..start + frames]);
        buffer.channels[1][..frames].copy_from_slice(&input[start..start + frames]);
        effect.process(&[], &mut buffer, frames);
        output.extend_from_slice(&buffer.channels[0][..frames]);
    }
    let start = COLORIZER_FFT_SIZE + COLORIZER_HOP_SIZE * 3;
    let mut signal = 0.0_f64;
    let mut error = 0.0_f64;
    for index in start..total {
        let expected = input[index - COLORIZER_FFT_SIZE];
        signal += f64::from(expected * expected);
        let delta = output[index] - expected;
        error += f64::from(delta * delta);
    }
    let relative_db = 10.0 * (error.max(1e-30) / signal.max(1e-30)).log10();
    assert!(
        relative_db < -60.0,
        "COLA reconstruction error {relative_db:.1} dB"
    )
}
#[test]
fn colorizer_mask_tracks_pitch_classes_across_octaves() {
    let mut effect = Colorizer::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    effect.set_param("depth", 1.0);
    effect.set_param("resonance", 0.9);
    for pitch in 0..12 {
        effect.set_param(&format!("pitch{pitch}"), if pitch == 0 { 1.0 } else { 0.0 });
    }
    effect.rebuild_mask();
    for frequency in [65.406_f32, 130.813, 261.626, 523.251, 1046.502] {
        let bin = (frequency * COLORIZER_FFT_SIZE as f32 / 48_000.0).round() as usize;
        assert!(
            effect.mask_target[bin] > 0.72,
            "C octave {frequency} Hz was closed"
        )
    }
    let off_bin = (369.994 * COLORIZER_FFT_SIZE as f32 / 48_000.0).round() as usize;
    assert!(effect.mask_target[off_bin] < 0.2)
}
#[test]
fn colorizer_decay_is_finite_and_midi_gate_closes_without_notes() {
    let mut effect = Colorizer::new();
    effect.set_param("decay", 1.0);
    effect.set_param("depth", 1.0);
    effect.set_param("mix", 1.0);
    effect.set_param("midi", 1.0);
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    assert!(effect.wants_midi());
    effect.rebuild_mask();
    assert!(effect.mask_target.iter().all(|gain| *gain == 0.0));
    let event = NoteEvent {
        sample_offset: 0,
        kind: NoteEventKind::NoteOn {
            note_id: 1,
            pitch: 60,
            velocity: 1.0,
            tuning_cents: 0.0,
        },
    };
    let mut energy = 0.0_f64;
    for block in 0..20 {
        let mut buffer = AudioBuffer::new();
        if block == 0 {
            buffer.channels[0][COLORIZER_HOP_SIZE / 2] = 1.0;
            buffer.channels[1][COLORIZER_HOP_SIZE / 2] = 1.0;
            effect.process(&[event], &mut buffer, MAX_BLOCK_SIZE);
        } else {
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        }
        for sample in &buffer.channels[0][..MAX_BLOCK_SIZE] {
            assert!(sample.is_finite());
            energy += f64::from(sample * sample);
        }
    }
    assert_eq!(effect.midi_mask, 1);
    assert!(energy.is_finite());
    assert!(effect.decay_coefficient() < 1.0)
}
#[test]
fn colorizer_held_phase_is_stable_and_realtime_buffers_do_not_grow() {
    let mut effect = Colorizer::new();
    effect.set_param("decay", 0.88);
    effect.set_param("depth", 0.0);
    effect.set_param("mix", 1.0);
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let capacities: [(usize, usize, usize, usize, usize); MAX_CHANNELS] =
        std::array::from_fn(|channel| {
            (
                effect.channels[channel].input_ring.capacity(),
                effect.channels[channel].ola_ring.capacity(),
                effect.channels[channel].spectrum.capacity(),
                effect.channels[channel].scratch.capacity(),
                effect.channels[channel].held.capacity(),
            )
        });
    let mut output = Vec::new();
    for block in 0..20 {
        let mut buffer = AudioBuffer::new();
        if block < 8 {
            for frame in 0..MAX_BLOCK_SIZE {
                let sample = block * MAX_BLOCK_SIZE + frame;
                let value = (2.0 * PI * 440.0 * sample as f32 / 48_000.0).sin() * 0.3;
                buffer.channels[0][frame] = value;
                buffer.channels[1][frame] = value;
            }
        }
        effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        output.extend_from_slice(&buffer.channels[0][..MAX_BLOCK_SIZE]);
    }
    let tail_start = COLORIZER_FFT_SIZE + 8 * MAX_BLOCK_SIZE + COLORIZER_HOP_SIZE;
    let period = (48_000.0_f32 / 445.3125).round() as usize;
    let mut correlation = 0.0_f64;
    let mut left_energy = 0.0_f64;
    let mut right_energy = 0.0_f64;
    for index in tail_start..output.len() - period {
        correlation += f64::from(output[index] * output[index + period]);
        left_energy += f64::from(output[index] * output[index]);
        right_energy += f64::from(output[index + period] * output[index + period]);
    }
    let normalized = correlation / (left_energy * right_energy).sqrt().max(1e-20);
    assert!(
        normalized > 0.9,
        "unstable held phase correlation {normalized:.3}"
    );
    for channel in 0..MAX_CHANNELS {
        assert_eq!(
            capacities[channel],
            (
                effect.channels[channel].input_ring.capacity(),
                effect.channels[channel].ola_ring.capacity(),
                effect.channels[channel].spectrum.capacity(),
                effect.channels[channel].scratch.capacity(),
                effect.channels[channel].held.capacity(),
            )
        )
    }
}
