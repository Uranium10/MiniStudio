use super::*;

/// Not a correctness test: prints measured per-callback CPU cost for
/// `Colorizer`'s `Map` quality mode at both FFT sizes, for
/// `docs/colorizer-algorithm.md`'s F-5 performance record. Run explicitly
/// with `cargo test -p ministudio-dsp --release colorizer_map_bench --
/// --ignored --nocapture`; excluded from the normal suite because wall-clock
/// timing is not a deterministic pass/fail signal.
#[test]
#[ignore]
fn colorizer_map_bench_reports_callback_cost() {
    let sample_rate = 48_000.0_f32;
    let block = 256_usize; // representative low-latency live buffer
    for (label, map_quality) in [("Fast/512", 0.0_f32), ("Clean/1024", 1.0_f32)] {
        let mut effect = Colorizer::new();
        effect.set_param("quality", 1.0);
        effect.set_param("mapQuality", map_quality);
        effect.set_param("mix", 1.0);
        effect.set_param("depth", 0.8);
        effect.set_param("transient", 0.5);
        effect.set_param("gate", 0.3);
        for pitch in [0, 2, 4, 5, 7, 9, 11] {
            effect.set_param(&format!("pitch{pitch}"), 1.0);
        }
        effect.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);

        // Dense material: a chord plus noise, closer to the "polyphonic
        // pad/drum loop" material the design doc calls for than a bare tone.
        let mut buffers: Vec<AudioBuffer> = (0..64)
            .map(|block_index| {
                let mut buffer = AudioBuffer::new();
                for frame in 0..block {
                    let sample = (block_index * block + frame) as f32;
                    let t = sample / sample_rate;
                    let mut value = 0.0;
                    for freq in [220.0, 277.18, 329.63, 440.0] {
                        value += (2.0 * PI * freq * t).sin() * 0.12;
                    }
                    let noise = ((sample * 12.9898).sin() * 43758.5453).fract() * 0.02;
                    buffer.channels[0][frame] = value + noise;
                    buffer.channels[1][frame] = value - noise;
                }
                buffer
            })
            .collect();

        // Warm up (fills STFT rings, avoids measuring cold-start branches).
        for buffer in &mut buffers {
            effect.process(&[], buffer, block);
        }

        let iterations = 400;
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            for buffer in &mut buffers {
                effect.process(&[], buffer, block);
            }
        }
        let elapsed = started.elapsed();
        let callbacks = iterations * buffers.len();
        let per_callback = elapsed / callbacks as u32;
        let deadline = std::time::Duration::from_secs_f32(block as f32 / sample_rate);
        let load_percent = 100.0 * per_callback.as_secs_f64() / deadline.as_secs_f64();
        println!(
            "Colorizer Map/{label}: {per_callback:?}/callback, {block}-frame deadline {deadline:?}, {load_percent:.2}% of budget"
        );
    }
}

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
fn dynamics_processors_publish_fixed_realtime_io_history() {
    let mut effects: [Box<dyn DspEffect>; 2] = [
        Box::new(Compressor::new()),
        Box::new(UpwardCompressor::new()),
    ];
    for effect in &mut effects {
        effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
        for block in 0..4 {
            let mut buffer = AudioBuffer::new();
            for frame in 0..MAX_BLOCK_SIZE {
                let sample = block * MAX_BLOCK_SIZE + frame;
                let value = (2.0 * PI * 220.0 * sample as f32 / 48_000.0).sin() * 0.3;
                buffer.channels[0][frame] = value;
                buffer.channels[1][frame] = value;
            }
            effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        }
        let history = effect.effect_spectrum().expect("dynamics I/O history");
        assert!(history.iter().all(|value| value.is_finite()));
        assert!(history[..DYNAMICS_HISTORY_POINTS]
            .iter()
            .any(|value| *value > 0.1));
        assert!(history[DYNAMICS_HISTORY_POINTS..]
            .iter()
            .any(|value| *value > 0.01));
    }
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
        "builtin:transient-shaper",
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
        ("builtin:resonator", COLORIZER_CLEAN_FFT_SIZE),
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
fn transient_shaper_is_neutral_at_default_and_preserves_stereo_link() {
    let mut effect = TransientShaper::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let mut buffer = AudioBuffer::new();
    buffer.channels[0][0] = 0.5;
    buffer.channels[1][0] = -0.25;
    effect.process(&[], &mut buffer, 1);
    assert_eq!(buffer.channels[0][0], 0.5);
    assert_eq!(buffer.channels[1][0], -0.25);

    effect.set_param("attack", 1.0);
    for frame in 0..MAX_BLOCK_SIZE {
        let sample = if frame % 240 == 0 { 0.8 } else { 0.03 };
        buffer.channels[0][frame] = sample;
        buffer.channels[1][frame] = sample * 0.5;
    }
    effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    assert!(buffer
        .channels
        .iter()
        .flatten()
        .all(|sample| sample.is_finite()));
    for frame in 0..MAX_BLOCK_SIZE {
        assert!((buffer.channels[1][frame] * 2.0 - buffer.channels[0][frame]).abs() < 1e-5);
    }
}

#[test]
fn transient_shaper_clip_bounds_large_boosts() {
    let mut effect = TransientShaper::new();
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    effect.set_param("attack", 1.0);
    effect.set_param("sustain", 1.0);
    effect.set_param("thresholdDb", -72.0);
    effect.set_param("clip", 1.0);
    let mut buffer = AudioBuffer::new();
    buffer.channels[0][..MAX_BLOCK_SIZE].fill(4.0);
    buffer.channels[1][..MAX_BLOCK_SIZE].fill(-4.0);
    effect.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    assert!(buffer
        .channels
        .iter()
        .flatten()
        .all(|sample| sample.is_finite() && sample.abs() <= 1.000_001));
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
fn render_colorizer(
    quality: f32,
    color: f32,
    morph: f32,
    gate: f32,
    mask: u16,
    input: &[f32],
) -> (Vec<f32>, usize) {
    let mut effect = Colorizer::new();
    effect.set_param("quality", quality);
    effect.set_param("color", color);
    effect.set_param("morph", morph);
    effect.set_param("gate", gate);
    for pitch in 0..12 {
        effect.set_param(
            &format!("pitch{pitch}"),
            if mask & (1 << pitch) != 0 { 1.0 } else { 0.0 },
        );
    }
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let latency = effect.latency_samples();
    let mut output = Vec::with_capacity(input.len());
    for block in input.chunks(256) {
        let mut buffer = AudioBuffer::new();
        for (frame, sample) in block.iter().enumerate() {
            buffer.channels[0][frame] = *sample;
            buffer.channels[1][frame] = -*sample * 0.5;
        }
        effect.process(&[], &mut buffer, block.len());
        output.extend_from_slice(&buffer.channels[0][..block.len()]);
    }
    (output, latency)
}

fn sine_wave(frequency: f32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|sample| (2.0 * PI * frequency * sample as f32 / 48_000.0).sin() * 0.35)
        .collect()
}

fn tone_power(signal: &[f32], frequency: f32) -> f64 {
    let mut real = 0.0_f64;
    let mut imaginary = 0.0_f64;
    for (index, sample) in signal.iter().enumerate() {
        let phase = 2.0 * std::f64::consts::PI * frequency as f64 * index as f64 / 48_000.0;
        real += f64::from(*sample) * phase.cos();
        imaginary -= f64::from(*sample) * phase.sin();
    }
    real * real + imaginary * imaginary
}

#[test]
fn colorizer_i1_all_pitch_classes_is_sample_accurate_and_unity_gain() {
    let input: Vec<f32> = (0..16_384)
        .map(|sample| {
            (2.0 * PI * 997.0 * sample as f32 / 48_000.0).sin() * 0.31
                + (2.0 * PI * 3_113.0 * sample as f32 / 48_000.0).sin() * 0.09
        })
        .collect();
    for quality in [0.0, 1.0] {
        let (output, latency) = render_colorizer(quality, 1.0, 0.0, 0.0, 0x0fff, &input);
        let start = latency * 2;
        let mut reference_energy = 0.0_f64;
        let mut output_energy = 0.0_f64;
        let mut error_energy = 0.0_f64;
        for sample in start..output.len() {
            let reference = input[sample - latency];
            reference_energy += f64::from(reference * reference);
            output_energy += f64::from(output[sample] * output[sample]);
            error_energy += f64::from((output[sample] - reference).powi(2));
        }
        let error_db = 10.0 * (error_energy / reference_energy).max(1e-30).log10();
        let rms_db = 10.0 * (output_energy / reference_energy).log10();
        assert!(error_db <= -60.0, "quality={quality}, error={error_db} dB");
        assert!(
            rms_db.abs() <= 0.5,
            "quality={quality}, RMS delta={rms_db} dB"
        );
    }
}

#[test]
fn colorizer_i1b_has_no_tail_at_color_100_with_gate_off() {
    let burst_samples = 9_600;
    let mut input = sine_wave(440.0, burst_samples);
    input.resize(burst_samples + 48_000, 0.0);
    for quality in [0.0, 1.0] {
        let (output, latency) = render_colorizer(quality, 1.0, 0.0, 0.0, 1 << 0, &input);
        let hop = if quality < 0.5 { 128 } else { 256 };
        let start = burst_samples + latency + hop;
        let peak = output[start..]
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        assert!(peak <= 1e-4, "quality={quality}, residual tail={peak}");
    }
}

#[test]
fn colorizer_i2_moves_445hz_toward_440hz_without_moving_in_key_440hz() {
    let samples = 48_000;
    for (input_frequency, should_move) in [(440.0_f32, false), (445.0, true)] {
        let input = sine_wave(input_frequency, samples);
        let (output, latency) = render_colorizer(1.0, 1.0, 0.0, 0.0, 1 << 9, &input);
        let settled = &output[latency * 4..];
        let at_440 = tone_power(settled, 440.0);
        let at_input = tone_power(settled, input_frequency);
        if should_move {
            assert!(at_440 > at_input * 1.5, "440={at_440}, input={at_input}");
        } else {
            assert!(at_440 > 1.0);
        }
    }
}

#[test]
fn colorizer_i3_preserves_harmonic_family_integer_ratios() {
    let samples = 48_000;
    let mut input = vec![0.0_f32; samples];
    for (harmonic, gain) in [(1, 0.24), (2, 0.12), (3, 0.08), (4, 0.06), (5, 0.04)] {
        for (sample, value) in input.iter_mut().enumerate() {
            *value +=
                (2.0 * PI * 277.182_65 * harmonic as f32 * sample as f32 / 48_000.0).sin() * gain;
        }
    }
    let (output, latency) = render_colorizer(1.0, 1.0, 0.0, 0.0, 1 << 0, &input);
    let settled = &output[latency * 4..];
    for harmonic in 1..=5 {
        let target = tone_power(settled, 261.625_55 * harmonic as f32);
        let independently_snapped = tone_power(
            settled,
            440.0
                * 2.0_f32.powf(
                    ((69.0 + 12.0 * ((277.182_65 * harmonic as f32) / 440.0).log2()).round()
                        - 69.0)
                        / 12.0,
                ),
        );
        assert!(target > 1.0, "harmonic {harmonic} missing: {target}");
        if harmonic > 1 {
            assert!(target >= independently_snapped * 0.25);
        }
    }
}

#[test]
fn colorizer_i4_sustained_output_is_phase_coherent_between_frames() {
    let input = sine_wave(277.182_65, 48_000);
    let (output, latency) = render_colorizer(1.0, 1.0, 0.0, 0.0, 1 << 0, &input);
    let settled = &output[latency * 4..];
    let period = (48_000.0_f32 / 261.625_55).round() as usize;
    let mut cross = 0.0_f64;
    let mut left_energy = 0.0_f64;
    let mut right_energy = 0.0_f64;
    for index in period..settled.len() {
        let left = f64::from(settled[index]);
        let right = f64::from(settled[index - period]);
        cross += left * right;
        left_energy += left * left;
        right_energy += right * right;
    }
    let correlation = cross / (left_energy * right_energy).sqrt().max(1e-20);
    assert!(correlation >= 0.92, "period correlation={correlation}");
}

#[test]
fn colorizer_linked_decisions_preserve_stereo_polarity_and_level() {
    let mut effect = Colorizer::new();
    effect.set_param("quality", 1.0);
    effect.set_param("color", 1.0);
    effect.set_param("morph", 0.0);
    for pitch in 0..12 {
        effect.set_param(&format!("pitch{pitch}"), if pitch == 0 { 1.0 } else { 0.0 });
    }
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let mut maximum_error = 0.0_f32;
    let mut maximum_level = 0.0_f32;
    for block in 0..24 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..256 {
            let sample = block * 256 + frame;
            let left = (2.0 * PI * 277.182_65 * sample as f32 / 48_000.0).sin() * 0.3;
            buffer.channels[0][frame] = left;
            buffer.channels[1][frame] = -left * 0.5;
        }
        effect.process(&[], &mut buffer, 256);
        if block >= 8 {
            for frame in 0..256 {
                maximum_level = maximum_level.max(buffer.channels[0][frame].abs());
                maximum_error = maximum_error
                    .max((buffer.channels[1][frame] + buffer.channels[0][frame] * 0.5).abs());
            }
        }
    }
    assert!(maximum_level > 0.05);
    assert!(
        maximum_error <= maximum_level * 5e-4,
        "error={maximum_error}, level={maximum_level}"
    );
}

#[test]
fn colorizer_midi_notes_drive_the_active_pitch_class_mask() {
    let mut effect = Colorizer::new();
    effect.set_param("quality", 1.0);
    effect.set_param("color", 1.0);
    effect.set_param("morph", 0.0);
    effect.set_param("midi", 1.0);
    effect.prepare(48_000.0, MAX_BLOCK_SIZE, MAX_CHANNELS);
    let note = NoteEvent {
        sample_offset: 0,
        kind: NoteEventKind::NoteOn {
            note_id: 12,
            pitch: 60,
            velocity: 0.8,
            tuning_cents: 0.0,
        },
    };
    let note_events = [note];
    let mut output = Vec::with_capacity(12_288);
    for block in 0..48 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..256 {
            let sample = block * 256 + frame;
            let value = (2.0 * PI * 277.182_65 * sample as f32 / 48_000.0).sin() * 0.3;
            buffer.channels[0][frame] = value;
            buffer.channels[1][frame] = value;
        }
        effect.process(
            if block == 0 { &note_events } else { &[] },
            &mut buffer,
            256,
        );
        if block >= 12 {
            output.extend_from_slice(&buffer.channels[0][..256]);
        }
    }
    assert_eq!(effect.midi_mask, 1 << 0);
    let target = tone_power(&output, 261.625_55);
    let source = tone_power(&output, 277.182_65);
    assert!(target > source * 1.5, "target={target}, source={source}");
}

#[test]
fn colorizer_i5_energy_is_preserved_within_three_db() {
    let input: Vec<f32> = (0..48_000)
        .map(|sample| {
            (2.0 * PI * 277.182_65 * sample as f32 / 48_000.0).sin() * 0.24
                + (2.0 * PI * 554.365_3 * sample as f32 / 48_000.0).sin() * 0.11
        })
        .collect();
    let (output, latency) = render_colorizer(1.0, 1.0, 0.0, 0.0, 1 << 0, &input);
    let start = latency * 4;
    let input_energy = input[..input.len() - start]
        .iter()
        .map(|sample| f64::from(sample * sample))
        .sum::<f64>();
    let output_energy = output[start..]
        .iter()
        .map(|sample| f64::from(sample * sample))
        .sum::<f64>();
    let delta_db = 10.0 * (output_energy / input_energy).log10();
    assert!(delta_db.abs() <= 3.0, "energy delta={delta_db} dB");
}

#[test]
fn colorizer_i6_morph_preserves_at_least_ninety_percent_of_attack_peak() {
    let mut input = vec![0.0_f32; 8_192];
    input[0] = 1.0;
    let (dry, latency) = render_colorizer(1.0, 0.0, 1.0, 0.0, 1 << 0, &input);
    let (morphed, _) = render_colorizer(1.0, 1.0, 1.0, 0.0, 1 << 0, &input);
    let dry_peak = dry[latency..latency + 1024]
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max);
    let wet_peak = morphed[latency..latency + 1024]
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max);
    assert!(wet_peak >= dry_peak * 0.9, "dry={dry_peak}, wet={wet_peak}");
}

#[test]
fn colorizer_i7_impulse_latency_matches_reported_quality() {
    for quality in [0.0, 1.0] {
        let mut input = vec![0.0_f32; 4_096];
        input[0] = 1.0;
        let (output, latency) = render_colorizer(quality, 1.0, 0.0, 0.0, 0x0fff, &input);
        let peak = output
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.abs().total_cmp(&right.1.abs()))
            .unwrap();
        assert_eq!(peak.0, latency);
        assert!((peak.1 - 1.0).abs() < 1e-5);
    }
}

#[test]
fn colorizer_stays_finite_during_long_polyphonic_processing() {
    let input: Vec<f32> = (0..96_000)
        .map(|sample| {
            [220.0_f32, 277.182_65, 329.627_56, 392.0]
                .iter()
                .map(|frequency| (2.0 * PI * frequency * sample as f32 / 48_000.0).sin() * 0.06)
                .sum()
        })
        .collect();
    let (output, _) = render_colorizer(1.0, 1.0, 0.72, 0.3, 0b1010_1101_0101, &input);
    assert!(output.iter().all(|sample| sample.is_finite()));
}

fn vowel_formants(sample_rate: f32) -> [Biquad; 2] {
    let mut bands = [Biquad::new(), Biquad::new()];
    bands[0].configure(FilterKind::Bell, 700.0, 18.0, 4.0, sample_rate);
    bands[1].configure(FilterKind::Bell, 2200.0, 18.0, 4.0, sample_rate);
    bands
}

/// A buzzy sawtooth carved by two Bell peaks stands in for a vowel with formants at 700 Hz
/// and 2200 Hz - simple enough for a 20th-order LPC to resolve cleanly in tests.
fn synthesize_vowel_block(
    phase: &mut f32,
    fundamental: f32,
    sample_rate: f32,
    formants: &mut [Biquad; 2],
    frames: usize,
) -> Vec<f32> {
    let mut out = vec![0.0_f32; frames];
    for sample in &mut out {
        *phase = (*phase + fundamental / sample_rate).fract();
        let mut value = (*phase * 2.0 - 1.0) * 0.2;
        for band in formants.iter_mut() {
            value = band.process(0, value);
        }
        *sample = value;
    }
    out
}

#[test]
fn formant_shifter_stays_finite_and_bounded_under_shift() {
    let sample_rate = 48_000.0;
    let mut shifter = FormantShifter::new();
    shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    shifter.set_param("mode", 1.0); // Mono
    shifter.set_param("pitchSemitones", -7.0);
    shifter.set_param("formantSemitones", 5.0);
    shifter.set_param("formantLink", 0.0);
    shifter.set_param("mix", 1.0);
    shifter.set_param("outputDb", 0.0);
    let mut phase = 0.0_f32;
    for _block in 0..40 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            phase = (phase + 130.0 / sample_rate).fract();
            let sample = (phase * 2.0 - 1.0) * 0.3;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        shifter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
        for sample in buffer
            .channels
            .iter()
            .flat_map(|channel| &channel[..MAX_BLOCK_SIZE])
        {
            assert!(sample.is_finite());
            assert!(sample.abs() < 4.0, "unexpectedly large sample {sample}")
        }
    }
}

#[test]
fn formant_shifter_correction_pulls_envelope_toward_original_formants() {
    let sample_rate = 48_000.0;
    let total_frames = MAX_BLOCK_SIZE * 6;

    let mut dry_phase = 0.0_f32;
    let mut dry_formants = vowel_formants(sample_rate);
    let dry = synthesize_vowel_block(
        &mut dry_phase,
        110.0,
        sample_rate,
        &mut dry_formants,
        total_frames,
    );

    let run = |link: f32| -> Vec<f32> {
        let mut shifter = FormantShifter::new();
        shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
        shifter.set_param("mode", 1.0); // Mono, deterministic regardless of Auto's YIN vote
        shifter.set_param("pitchSemitones", 12.0);
        shifter.set_param("formantSemitones", 0.0);
        shifter.set_param("formantLink", link);
        shifter.set_param("mix", 1.0);
        let mut output = Vec::with_capacity(total_frames);
        let mut cursor = 0;
        while cursor < total_frames {
            let block_frames = MAX_BLOCK_SIZE.min(total_frames - cursor);
            let mut buffer = AudioBuffer::new();
            for frame in 0..block_frames {
                let sample = dry[cursor + frame];
                buffer.channels[0][frame] = sample;
                buffer.channels[1][frame] = sample;
            }
            shifter.process(&[], &mut buffer, block_frames);
            for frame in 0..block_frames {
                assert!(buffer.channels[0][frame].is_finite());
                output.push(buffer.channels[0][frame]);
            }
            cursor += block_frames;
        }
        output
    };

    let uncorrected = run(1.0); // linked: formants follow pitch, no correction
    let corrected = run(0.0); // unlinked: independent formant, correction active

    let target_freqs = [700.0_f32, 2200.0];
    let windowed_envelope = |signal: &[f32]| -> [f32; 2] {
        let mut frame = [0.0_f32; FORMANT_ANALYSIS_FRAME];
        let start = signal.len() - FORMANT_ANALYSIS_FRAME;
        for (index, slot) in frame.iter_mut().enumerate() {
            let window =
                0.5 - 0.5 * (2.0 * PI * index as f32 / (FORMANT_ANALYSIS_FRAME - 1) as f32).cos();
            *slot = signal[start + index] * window;
        }
        let lpc = levinson_durbin(&autocorrelate(&frame));
        std::array::from_fn(|k| lpc_envelope_db(&lpc, target_freqs[k], sample_rate))
    };

    let dry_env = windowed_envelope(&dry);
    let uncorrected_env = windowed_envelope(&uncorrected);
    let corrected_env = windowed_envelope(&corrected);
    // Absolute per-band distance to the dry target, summed - not the difference *between* the
    // two bands. A correction that pulls each band closer individually, even unevenly, should
    // pass; a "shape" (band0 - band1) metric can get worse from uneven-but-real improvement.
    let total_deviation = |env: [f32; 2]| (env[0] - dry_env[0]).abs() + (env[1] - dry_env[1]).abs();
    let uncorrected_error = total_deviation(uncorrected_env);
    let corrected_error = total_deviation(corrected_env);

    assert!(
        corrected_error < uncorrected_error * 0.6,
        "correction did not meaningfully pull formants back: corrected={corrected_error:.2}dB \
         uncorrected={uncorrected_error:.2}dB"
    );
}

#[test]
fn formant_shifter_settles_near_unity_correction_when_pitch_is_unchanged() {
    let sample_rate = 48_000.0;
    let total_frames = MAX_BLOCK_SIZE * 6;
    let mut shifter = FormantShifter::new();
    shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    shifter.set_param("mode", 1.0); // Mono
    shifter.set_param("pitchSemitones", 0.0);
    shifter.set_param("formantSemitones", 0.0);
    shifter.set_param("formantLink", 0.0);
    shifter.set_param("mix", 1.0);

    let mut phase = 0.0_f32;
    let mut formants = vowel_formants(sample_rate);
    let dry = synthesize_vowel_block(&mut phase, 110.0, sample_rate, &mut formants, total_frames);
    let mut cursor = 0;
    while cursor < total_frames {
        let block_frames = MAX_BLOCK_SIZE.min(total_frames - cursor);
        let mut buffer = AudioBuffer::new();
        for frame in 0..block_frames {
            let sample = dry[cursor + frame];
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        shifter.process(&[], &mut buffer, block_frames);
        cursor += block_frames;
    }
    for &gain in &shifter.current_band_db {
        assert!(
            gain.abs() < 2.0,
            "unexpected correction with no pitch shift: {gain:.2}dB"
        )
    }
}

#[test]
fn formant_shifter_poly_shifts_a_pure_tone_to_the_expected_frequency() {
    let sample_rate = 48_000.0;
    let mut shifter = FormantShifter::new();
    shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    shifter.set_param("mode", 2.0); // Poly, forced regardless of what Auto would pick
    shifter.set_param("pitchSemitones", 12.0);
    shifter.set_param("formantLink", 1.0);
    shifter.set_param("mix", 1.0);

    let input_freq = 220.0_f32;
    let total_frames = MAX_BLOCK_SIZE * 12;
    let mut phase = 0.0_f32;
    let mut output = Vec::with_capacity(total_frames);
    let mut cursor = 0;
    while cursor < total_frames {
        let block_frames = MAX_BLOCK_SIZE.min(total_frames - cursor);
        let mut buffer = AudioBuffer::new();
        for frame in 0..block_frames {
            phase = (phase + input_freq / sample_rate).fract();
            let sample = (2.0 * PI * phase).sin() * 0.3;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        shifter.process(&[], &mut buffer, block_frames);
        for frame in 0..block_frames {
            assert!(buffer.channels[0][frame].is_finite());
            output.push(buffer.channels[0][frame]);
        }
        cursor += block_frames;
    }
    let tail: [f32; ROBOTER_YIN_FRAME] =
        std::array::from_fn(|index| output[output.len() - ROBOTER_YIN_FRAME + index]);
    let estimate = yin_pitch(&tail, sample_rate);
    assert!(
        estimate.voiced,
        "expected the shifted tone to still read as voiced (confidence={})",
        estimate.confidence
    );
    let expected = input_freq * 2.0; // +12 semitones
    let error_cents = 1200.0 * (estimate.frequency / expected).log2();
    assert!(
        error_cents.abs() < 50.0,
        "poly shift landed at {}Hz, expected ~{expected}Hz ({error_cents:.1} cents off)",
        estimate.frequency
    );
}

#[test]
fn formant_shifter_auto_picks_mono_for_a_clean_tone_and_poly_for_noise() {
    let sample_rate = 48_000.0;

    let mut tone_shifter = FormantShifter::new();
    tone_shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    tone_shifter.set_param("pitchSemitones", 3.0);
    let mut phase = 0.0_f32;
    for _block in 0..8 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            phase = (phase + 220.0 / sample_rate).fract();
            let sample = (2.0 * PI * phase).sin() * 0.3;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        tone_shifter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    }
    assert!(
        matches!(tone_shifter.auto_engine, FormantMode::Mono),
        "expected Auto to settle on Mono for a clean tone"
    );

    let mut noise_shifter = FormantShifter::new();
    noise_shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    noise_shifter.set_param("pitchSemitones", 3.0);
    let mut seed = 12_345_u32;
    for _block in 0..8 {
        let mut buffer = AudioBuffer::new();
        for frame in 0..MAX_BLOCK_SIZE {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let sample = ((seed >> 8) as f32 / (1_u32 << 24) as f32 * 2.0 - 1.0) * 0.3;
            buffer.channels[0][frame] = sample;
            buffer.channels[1][frame] = sample;
        }
        noise_shifter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    }
    assert!(
        matches!(noise_shifter.auto_engine, FormantMode::Poly),
        "expected Auto to settle on Poly for noise"
    );
}

#[test]
fn formant_shifter_reports_latency_by_mode() {
    let mut mono = FormantShifter::new();
    mono.set_param("mode", 1.0);
    assert_eq!(mono.latency_samples(), 0);

    let mut poly = FormantShifter::new();
    poly.set_param("mode", 2.0);
    assert_eq!(poly.latency_samples(), FORMANT_DECLARED_LATENCY);

    let auto = FormantShifter::new();
    assert_eq!(auto.latency_samples(), FORMANT_DECLARED_LATENCY);
}

#[test]
fn formant_shifter_passes_through_unchanged_at_unity_pitch_in_mono_mode() {
    let sample_rate = 48_000.0;
    let mut shifter = FormantShifter::new();
    shifter.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    shifter.set_param("mode", 1.0); // Mono: unity means zero declared latency, so this should be bit-exact.
    shifter.set_param("pitchSemitones", 0.0);
    shifter.set_param("mix", 1.0);
    let mut buffer = AudioBuffer::new();
    let mut phase = 0.0_f32;
    for frame in 0..MAX_BLOCK_SIZE {
        phase = (phase + 220.0 / sample_rate).fract();
        let sample = (2.0 * PI * phase).sin() * 0.3;
        buffer.channels[0][frame] = sample;
        buffer.channels[1][frame] = sample;
    }
    let dry: Vec<f32> = buffer.channels[0][..MAX_BLOCK_SIZE].to_vec();
    shifter.process(&[], &mut buffer, MAX_BLOCK_SIZE);
    for (index, (&input, &output)) in dry
        .iter()
        .zip(buffer.channels[0][..MAX_BLOCK_SIZE].iter())
        .enumerate()
    {
        assert!(
            (input - output).abs() < 1e-5,
            "sample {index} was altered at unity: {input} -> {output}"
        );
    }
}
