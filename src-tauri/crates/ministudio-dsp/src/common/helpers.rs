fn parse_band_param(id: &str, band_count: usize) -> (usize, &str) {
    if let Some(rest) = id.strip_prefix("band") {
        let mut p = rest.split('.');
        let idx = p.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        return (idx, p.next().unwrap_or("gain"));
    }
    let idx = if id.starts_with("low") {
        0
    } else if id.starts_with("high") {
        band_count.saturating_sub(1)
    } else {
        1
    };
    let p = if id.ends_with("Freq") {
        "freq"
    } else if id.ends_with("Gain") {
        "gain"
    } else {
        "q"
    };
    (idx, p)
}
fn kind_from_value(v: f32) -> FilterKind {
    match v as usize {
        1 => FilterKind::LowShelf,
        2 => FilterKind::HighShelf,
        3 => FilterKind::HighPass,
        4 => FilterKind::LowPass,
        5 => FilterKind::Notch,
        _ => FilterKind::Bell,
    }
}
#[inline(always)]
fn amplitude_to_db(value: f32) -> f32 {
    20.0 * value.max(1e-6).log10()
}
#[inline(always)]
fn power_to_lufs(power: f64) -> f32 {
    if power <= 1e-12 {
        -120.0
    } else {
        (-0.691 + 10.0 * power.log10()) as f32
    }
}
fn integrated_lufs(blocks: &[f32], count: usize) -> f32 {
    let absolute_energy = 10.0_f32.powf((-70.0 + 0.691) / 10.0);
    let mut absolute_sum = 0.0_f64;
    let mut absolute_count = 0_usize;
    for &energy in blocks.iter().take(count) {
        if energy >= absolute_energy {
            absolute_sum += f64::from(energy);
            absolute_count += 1;
        }
    }
    if absolute_count == 0 {
        return -120.0;
    }
    let ungated = absolute_sum / absolute_count as f64;
    let relative_energy = (ungated * 0.1) as f32;
    let mut gated_sum = 0.0_f64;
    let mut gated_count = 0_usize;
    for &energy in blocks.iter().take(count) {
        if energy >= absolute_energy.max(relative_energy) {
            gated_sum += f64::from(energy);
            gated_count += 1;
        }
    }
    if gated_count == 0 {
        -120.0
    } else {
        power_to_lufs(gated_sum / gated_count as f64)
    }
}
#[inline(always)]
fn interpolated_peak(history: [f32; 4], factor: usize) -> f32 {
    let mut peak = history[1].abs().max(history[2].abs());
    for phase in 1..factor {
        peak = peak.max(
            catmull(
                history[0],
                history[1],
                history[2],
                history[3],
                phase as f32 / factor as f32,
            )
            .abs(),
        )
    }
    peak
}
fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}
#[inline(always)]
fn cubic_delay_read(line: &[f32], write: usize, delay: f32) -> f32 {
    let length = line.len();
    let mut position = write as f32 - delay;
    if position < 0.0 {
        position += length as f32
    }
    let base = position.floor() as usize;
    let fraction = position - base as f32;
    let previous = if base == 0 { length - 1 } else { base - 1 };
    let next = if base + 1 == length { 0 } else { base + 1 };
    let next_two = if next + 1 == length { 0 } else { next + 1 };
    catmull(
        line[previous],
        line[base],
        line[next],
        line[next_two],
        fraction,
    )
}
#[inline(always)]
fn fractional_read(line: &[f32], position: f32) -> f32 {
    let length = line.len();
    let mut wrapped = position;
    if wrapped < 0.0 {
        wrapped += length as f32
    } else if wrapped >= length as f32 {
        wrapped -= length as f32
    }
    let base = wrapped.floor() as usize;
    let fraction = wrapped - base as f32;
    let previous = if base == 0 { length - 1 } else { base - 1 };
    let next = if base + 1 == length { 0 } else { base + 1 };
    let next_two = if next + 1 == length { 0 } else { next + 1 };
    catmull(
        line[previous],
        line[base],
        line[next],
        line[next_two],
        fraction,
    )
}
#[inline(always)]
fn hadamard(v: &mut [f32; 8]) {
    let mut h = 1;
    while h < 8 {
        let step = h * 2;
        let mut i = 0;
        while i < 8 {
            for j in i..i + h {
                let a = v[j];
                let b = v[j + h];
                v[j] = a + b;
                v[j + h] = a - b
            }
            i += step
        }
        h *= 2
    }
    for value in v {
        *value *= 0.35355338
    }
}
#[inline(always)]
fn denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}
#[inline(always)]
fn increment_wrap(index: usize, length: usize) -> usize {
    let next = index + 1;
    if next == length {
        0
    } else {
        next
    }
}
#[inline(always)]
fn shape(curve: Curve, x: f32) -> f32 {
    match curve {
        Curve::SoftClip => x.tanh(),
        Curve::HardClip => x.clamp(-1.0, 1.0),
        // A sine soft clip is linear around zero and reaches the rails with a
        // zero derivative. Clamping its phase avoids foldback above the rail.
        Curve::Sine => (x.clamp(-1.0, 1.0) * PI * 0.5).sin(),
    }
}
/// Soft-knee saturation: transparent below `1.0 - knee`, then bends smoothly (with a
/// continuous first derivative at the knee) toward the ceiling instead of hard-clipping.
/// Public so the graph's master-output stage can reuse it as a safety limiter.
#[inline(always)]
pub fn clipper_curve(input: f32, knee: f32) -> f32 {
    let knee = knee.clamp(0.0, 1.0);
    if knee <= 1e-5 {
        return input.clamp(-1.0, 1.0);
    }
    let sign = input.signum();
    let magnitude = input.abs();
    let start = 1.0 - knee;
    if magnitude <= start {
        input
    } else {
        sign * (start + knee * ((magnitude - start) / knee).tanh())
    }
}
#[inline(always)]
fn fir_read(history: &[f32; 15], position: usize) -> f32 {
    const TAPS: [(usize, f32); 9] = [
        (0, -0.001682),
        (2, 0.010703),
        (4, -0.049012),
        (6, 0.289991),
        (7, 0.499999),
        (8, 0.289991),
        (10, -0.049012),
        (12, 0.010703),
        (14, -0.001682),
    ];
    let mut sum = 0.0;
    for (offset, coefficient) in TAPS {
        sum += history[(position + 15 - offset) % 15] * coefficient
    }
    sum
}
