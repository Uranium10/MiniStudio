// CPAL backend/device enumeration and configured output-stream construction.
use super::{
    engine::AudioCore,
    types::{AudioBackendInfo, AudioDeviceInfo, AudioSettings},
    MAX_BLOCK_SIZE,
};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, I24,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

pub fn list_backends() -> Vec<AudioBackendInfo> {
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
        .collect()
}
pub fn list_devices(backend: &str) -> Result<Vec<AudioDeviceInfo>, String> {
    let host = host_for(backend)?;
    let default_name = host.default_output_device().and_then(|v| v.name().ok());
    let mut out = Vec::new();
    for device in host.output_devices().map_err(|e| e.to_string())? {
        let name = device.name().unwrap_or_else(|_| "Unknown output".into());
        let mut rates = Vec::new();
        let mut buffers = vec![32, 64, 128, 256, 512, 1024, 2048];
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
                buffers.retain(|v| *v >= *min && *v <= *max)
            }
        }
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
) -> Result<Stream, String> {
    let host = host_for(&settings.backend_id)?;
    let device = if settings.device_id == "default" {
        host.default_output_device()
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|v| v.name().ok().as_deref() == Some(settings.device_id.as_str()))
    }
    .ok_or("output device not found")?;
    let supported = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| {
            settings.sample_rate >= c.min_sample_rate().0
                && settings.sample_rate <= c.max_sample_rate().0
        })
        .max_by_key(|config| sample_format_rank(config.sample_format()))
        .ok_or("sample rate is not supported")?
        .with_sample_rate(cpal::SampleRate(settings.sample_rate));
    let channels = supported.channels();
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(settings.sample_rate),
        buffer_size: BufferSize::Fixed(settings.buffer_size),
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::F64 => build::<f64>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::I8 => build::<i8>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::I16 => build::<i16>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::I24 => build::<I24>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::I32 => build::<i32>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::I64 => build::<i64>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::U8 => build::<u8>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::U16 => build::<u16>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::U32 => build::<u32>(&device, &config, channels, core, xruns, failed)?,
        SampleFormat::U64 => build::<u64>(&device, &config, channels, core, xruns, failed)?,
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
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
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
    f32: Sample,
{
    let mut scratch = vec![0.0_f32; MAX_BLOCK_SIZE * 2];
    let channel_count = channels as usize;
    let mut configured = false;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                if !configured {
                    enable_denormal_flush();
                    configured = true
                }
                for slice in data.chunks_mut(MAX_BLOCK_SIZE * channel_count) {
                    let frames = slice.len() / channel_count;
                    let stereo = &mut scratch[..frames * 2];
                    audio_core.render(stereo, frames);
                    for (frame, source) in
                        slice.chunks_mut(channel_count).zip(stereo.chunks_exact(2))
                    {
                        if let Some(v) = frame.get_mut(0) {
                            *v = T::from_sample(source[0])
                        }
                        if let Some(v) = frame.get_mut(1) {
                            *v = T::from_sample(source[1])
                        }
                        for value in frame.iter_mut().skip(2) {
                            *value = T::from_sample(0.0)
                        }
                    }
                }
            },
            move |_| {
                xruns.fetch_add(1, Ordering::Relaxed);
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| e.to_string())
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
        );
        assert!(result.is_ok(), "default stream failed: {:?}", result.err());
    }
}
