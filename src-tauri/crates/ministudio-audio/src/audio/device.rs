// CPAL backend/device enumeration and configured output-stream construction.
use super::{
    engine::AudioCore,
    metrics::RealtimeMetrics,
    types::{AudioBackendInfo, AudioDeviceInfo, AudioSettings},
    MAX_BLOCK_SIZE,
};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, I24,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

pub fn list_backends() -> Vec<AudioBackendInfo> {
    let mut backends = vec![AudioBackendInfo {
        id: "default".into(),
        name: "System Default".into(),
        available: true,
        asio: false,
    }];
    backends.extend(
        cpal::available_hosts()
            .into_iter()
            .filter_map(|id| {
                let name = id.name().to_owned();
                let asio = name.eq_ignore_ascii_case("asio");
                if asio && !cfg!(feature = "asio") {
                    return None;
                }
                Some(AudioBackendInfo {
                    id: name.to_lowercase(),
                    name,
                    available: true,
                    asio,
                })
            })
            .collect::<Vec<_>>(),
    );
    backends
}
pub fn list_devices(backend: &str) -> Result<Vec<AudioDeviceInfo>, String> {
    let host = host_for(backend)?;
    let default_name = host.default_output_device().and_then(|v| v.name().ok());
    let mut out = Vec::new();
    for device in host.output_devices().map_err(|e| e.to_string())? {
        let name = device.name().unwrap_or_else(|_| "Unknown output".into());
        let mut rates = Vec::new();
        let mut buffers = Vec::new();
        let mut channels = 2;
        for range in device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
        {
            channels = channels.max(range.channels());
            for rate in [44100, 48000, 88200, 96000, 192000] {
                if rate >= range.min_sample_rate().0
                    && rate <= range.max_sample_rate().0
                    && !rates.contains(&rate)
                {
                    rates.push(rate)
                }
            }
            if let cpal::SupportedBufferSize::Range { min, max } = range.buffer_size() {
                for value in [32, 64, 128, 256, 512, 1024, 2048] {
                    if value >= *min && value <= *max && !buffers.contains(&value) {
                        buffers.push(value)
                    }
                }
            }
        }
        if let Ok(default) = device.default_output_config() {
            let rate = default.sample_rate().0;
            if !rates.contains(&rate) {
                rates.push(rate)
            }
        }
        rates.sort_unstable();
        buffers.sort_unstable();
        out.push(AudioDeviceInfo {
            id: name.clone(),
            is_default: default_name.as_ref() == Some(&name),
            name,
            sample_rates: rates,
            buffer_sizes: buffers,
            channels,
        })
    }
    Ok(out)
}
pub fn open_stream(
    settings: &AudioSettings,
    core: AudioCore,
    xruns: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    cpu_load: Arc<AtomicU32>,
    cpu_peak: Arc<AtomicU32>,
    metrics: Arc<RealtimeMetrics>,
) -> Result<Stream, String> {
    let host = host_for(&settings.backend_id)?;
    let device = if settings.device_id == "default" {
        host.default_output_device()
            .or_else(|| host.output_devices().ok()?.next())
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|v| v.name().ok().as_deref() == Some(settings.device_id.as_str()))
    }
    .ok_or("output device not found")?;
    let ranges = device
        .supported_output_configs()
        .map_err(|e| format!("could not query output formats: {e}"))?
        .collect::<Vec<_>>();
    let default_rate = device
        .default_output_config()
        .ok()
        .map(|config| config.sample_rate().0);
    let sample_rate = if ranges.iter().any(|range| {
        settings.sample_rate >= range.min_sample_rate().0
            && settings.sample_rate <= range.max_sample_rate().0
    }) {
        settings.sample_rate
    } else if let Some(rate) = default_rate {
        rate
    } else {
        ranges
            .iter()
            .map(|range| range.min_sample_rate().0)
            .min_by_key(|rate| rate.abs_diff(settings.sample_rate))
            .ok_or("output device reports no supported stream formats")?
    };
    let range = ranges
        .into_iter()
        .filter(|candidate| {
            sample_rate >= candidate.min_sample_rate().0
                && sample_rate <= candidate.max_sample_rate().0
        })
        .max_by_key(|candidate| {
            (
                channel_rank(candidate.channels()),
                sample_format_rank(candidate.sample_format()),
            )
        })
        .ok_or_else(|| format!("output device does not support {sample_rate} Hz"))?;
    let buffer_size = match range.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max }
            if settings.buffer_size >= *min && settings.buffer_size <= *max =>
        {
            BufferSize::Fixed(settings.buffer_size)
        }
        _ => BufferSize::Default,
    };
    let supported = range.with_sample_rate(cpal::SampleRate(sample_rate));
    let channels = supported.channels();
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size,
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::F64 => build::<f64>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::I8 => build::<i8>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::I16 => build::<i16>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::I24 => build::<I24>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::I32 => build::<i32>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::I64 => build::<i64>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::U8 => build::<u8>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::U16 => build::<u16>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::U32 => build::<u32>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        SampleFormat::U64 => build::<u64>(
            &device, &config, channels, core, xruns, failed, cpu_load, cpu_peak, metrics,
        )?,
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };
    stream
        .play()
        .map_err(|e| format!("could not start output stream: {e}"))?;
    Ok(stream)
}

/// Resolve the device clock before building the graph. Some shared-mode
/// drivers expose only their current mix rate, so DSP and output must agree on
/// the negotiated value rather than continuing at the requested rate.
pub fn negotiated_sample_rate(settings: &AudioSettings) -> Result<u32, String> {
    let host = host_for(&settings.backend_id)?;
    let device = if settings.device_id == "default" {
        host.default_output_device()
            .or_else(|| host.output_devices().ok()?.next())
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|device| device.name().ok().as_deref() == Some(settings.device_id.as_str()))
    }
    .ok_or("output device not found")?;
    let ranges = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .collect::<Vec<_>>();
    if ranges.iter().any(|range| {
        settings.sample_rate >= range.min_sample_rate().0
            && settings.sample_rate <= range.max_sample_rate().0
    }) {
        return Ok(settings.sample_rate);
    }
    if let Ok(default) = device.default_output_config() {
        return Ok(default.sample_rate().0);
    }
    ranges
        .iter()
        .map(|range| range.min_sample_rate().0)
        .min_by_key(|rate| rate.abs_diff(settings.sample_rate))
        .ok_or_else(|| "output device reports no supported stream formats".into())
}
fn channel_rank(channels: u16) -> u8 {
    match channels {
        2 => 3,
        1 => 2,
        _ => 1,
    }
}
fn sample_format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 11,
        SampleFormat::F64 => 10,
        SampleFormat::I32 => 9,
        SampleFormat::I24 => 8,
        SampleFormat::I16 => 7,
        SampleFormat::U32 => 6,
        SampleFormat::U16 => 5,
        SampleFormat::I8 => 4,
        SampleFormat::U8 => 3,
        SampleFormat::I64 => 2,
        SampleFormat::U64 => 1,
        _ => 0,
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    mut audio_core: AudioCore,
    xruns: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    cpu_load: Arc<AtomicU32>,
    cpu_peak: Arc<AtomicU32>,
    metrics: Arc<RealtimeMetrics>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
    f32: Sample,
{
    let mut scratch = vec![0.0_f32; MAX_BLOCK_SIZE * 2];
    let channel_count = channels as usize;
    let sample_rate = config.sample_rate.0.max(1) as f64;
    let mut configured = false;
    let mut load_ema = 0.0_f32;
    let mut peak_hold = 0.0_f32;
    let mut peak_hold_blocks = 0_u32;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                if !configured {
                    enable_denormal_flush();
                    configured = true
                }
                for slice in data.chunks_mut(MAX_BLOCK_SIZE * channel_count) {
                    let started = Instant::now();
                    let frames = slice.len() / channel_count;
                    let stereo = &mut scratch[..frames * 2];
                    audio_core.render(stereo, frames);
                    for (frame, source) in
                        slice.chunks_mut(channel_count).zip(stereo.chunks_exact(2))
                    {
                        if channel_count == 1 {
                            frame[0] = T::from_sample((source[0] + source[1]) * 0.5)
                        } else {
                            frame[0] = T::from_sample(source[0]);
                            frame[1] = T::from_sample(source[1]);
                        }
                        for value in frame.iter_mut().skip(2) {
                            *value = T::from_sample(0.0)
                        }
                    }
                    let budget = frames as f64 / sample_rate;
                    let elapsed = started.elapsed();
                    metrics.observe_callback(elapsed.as_nanos().min(u128::from(u64::MAX)) as u64);
                    let load = if budget > 0.0 {
                        (elapsed.as_secs_f64() / budget).clamp(0.0, 8.0) as f32
                    } else {
                        0.0
                    };
                    let coefficient = if load > load_ema { 0.32 } else { 0.075 };
                    load_ema += (load - load_ema) * coefficient;
                    if load >= peak_hold {
                        peak_hold = load;
                        peak_hold_blocks = 90;
                    } else if peak_hold_blocks > 0 {
                        peak_hold_blocks -= 1;
                    } else {
                        peak_hold += (load - peak_hold) * 0.025;
                    }
                    cpu_load.store(load_ema.to_bits(), Ordering::Relaxed);
                    cpu_peak.store(peak_hold.max(load_ema).to_bits(), Ordering::Relaxed);
                }
            },
            move |_| {
                xruns.fetch_add(1, Ordering::Relaxed);
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| {
            format!(
                "could not build output stream ({:?}, {} Hz, {} ch): {e}",
                config.buffer_size, config.sample_rate.0, config.channels
            )
        })
}
fn host_for(id: &str) -> Result<cpal::Host, String> {
    if id == "default" {
        return Ok(cpal::default_host());
    }
    let host_id = cpal::available_hosts()
        .into_iter()
        .find(|v| v.name().eq_ignore_ascii_case(id))
        .ok_or("audio backend is unavailable")?;
    cpal::host_from_id(host_id).map_err(|e| e.to_string())
}
#[inline]
#[allow(deprecated)]
fn enable_denormal_flush() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | (1 << 15) | (1 << 6));
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        use std::arch::x86::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | (1 << 15) | (1 << 6));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opens the machine's real default audio output"]
    fn probe_default_output() {
        eprintln!("backends: {:#?}", list_backends());
        eprintln!("devices: {:#?}", list_devices("default"));
        let settings = AudioSettings::default();
        let result = open_stream(
            &settings,
            AudioCore::placeholder(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            Arc::new(RealtimeMetrics::default()),
        );
        assert!(result.is_ok(), "default stream failed: {:?}", result.err());
    }
}
