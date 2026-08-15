// Control-thread audio decoding, project-rate resampling, and binary peak generation.
use super::types::{DecodeProgress, NativeAssetInfo};
use rubato::{FftFixedInOut, Resampler};
use std::{
    fs::File,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};

static NEXT_ASSET_ID: AtomicU64 = AtomicU64::new(1);

pub struct AudioAsset {
    pub id: String,
    pub path: String,
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    pub frames: usize,
    pub lods: Vec<Vec<f32>>,
}

impl AudioAsset {
    pub fn info(&self) -> NativeAssetInfo {
        NativeAssetInfo {
            id: self.id.clone(),
            path: self.path.clone(),
            name: Path::new(&self.path)
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("Audio")
                .to_owned(),
            duration_sec: self.frames as f64 / f64::from(self.sample_rate),
            sample_rate: self.sample_rate,
            num_channels: self.channels.len(),
        }
    }

    pub fn peaks(&self, lod: u8) -> Option<&[f32]> {
        self.lods.get(usize::from(lod)).map(Vec::as_slice)
    }
}

pub fn decode_asset(
    path: &str,
    project_rate: u32,
    mut on_progress: impl FnMut(DecodeProgress),
) -> Result<Arc<AudioAsset>, String> {
    on_progress(DecodeProgress {
        stage: "decode".into(),
        fraction: 0.0,
    });
    let file = File::open(path).map_err(|e| e.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = Path::new(path).extension().and_then(|v| v.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or("audio file has no default track")?;
    let track_id = track.id;
    let source_rate = track
        .codec_params
        .sample_rate
        .ok_or("audio sample rate is unknown")?;
    let estimated_frames = track.codec_params.n_frames.unwrap_or(0);
    let channel_count = track
        .codec_params
        .channels
        .map(|v| v.count())
        .unwrap_or(2)
        .clamp(1, 2);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut channels = vec![Vec::<f32>::new(); channel_count];
    let mut decoded_frames = 0_u64;
    loop {
        let packet = match format.next_packet() {
            Ok(v) => v,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "cannot decode {}: {error}",
                    Path::new(path).display()
                ))
            }
        };
        let spec = *decoded.spec();
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        let packet_frames = samples.samples().len() / spec.channels.count();
        for frame in samples.samples().chunks(spec.channels.count()) {
            for ch in 0..channel_count {
                if let Some(v) = frame.get(ch) {
                    channels[ch].push(*v)
                }
            }
        }
        decoded_frames += packet_frames as u64;
        if estimated_frames > 0 {
            on_progress(DecodeProgress {
                stage: "decode".into(),
                fraction: (decoded_frames as f32 / estimated_frames as f32).clamp(0.0, 0.9),
            })
        }
    }
    if source_rate != project_rate {
        on_progress(DecodeProgress {
            stage: "resample".into(),
            fraction: 0.92,
        });
        channels = resample(channels, source_rate, project_rate)?;
    }
    let frames = channels.first().map(Vec::len).unwrap_or(0);
    if frames == 0 {
        return Err(format!(
            "audio file contains no decodable samples: {}",
            Path::new(path).display()
        ));
    }
    let lods = [256, 1024, 4096, 16384]
        .iter()
        .map(|size| peaks(&channels, *size))
        .collect();
    on_progress(DecodeProgress {
        stage: "peaks".into(),
        fraction: 1.0,
    });
    Ok(Arc::new(AudioAsset {
        id: format!(
            "native-asset-{}",
            NEXT_ASSET_ID.fetch_add(1, Ordering::Relaxed)
        ),
        path: path.into(),
        sample_rate: project_rate,
        channels,
        frames,
        lods,
    }))
}

fn resample(input: Vec<Vec<f32>>, from: u32, to: u32) -> Result<Vec<Vec<f32>>, String> {
    let count = input.len();
    let chunk = 1024;
    let mut r = FftFixedInOut::<f32>::new(from as usize, to as usize, chunk, count)
        .map_err(|e| e.to_string())?;
    let in_frames = r.input_frames_next();
    let out_frames = r.output_frames_next();
    let mut output = vec![Vec::new(); count];
    let total = input.first().map(Vec::len).unwrap_or(0);
    let mut offset = 0;
    while offset < total {
        let mut block = vec![vec![0.0; in_frames]; count];
        for ch in 0..count {
            let end = (offset + in_frames).min(total);
            let len = end - offset;
            block[ch][..len].copy_from_slice(&input[ch][offset..end])
        }
        let converted = r.process(&block, None).map_err(|e| e.to_string())?;
        for ch in 0..count {
            output[ch].extend_from_slice(&converted[ch][..out_frames.min(converted[ch].len())])
        }
        offset += in_frames
    }
    let expected = (total as f64 * f64::from(to) / f64::from(from)).round() as usize;
    for ch in &mut output {
        ch.truncate(expected)
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_pcm_wav_decodes_and_resamples() {
        let path =
            std::env::temp_dir().join(format!("ministudio-audio-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create fixture");
        for frame in 0..4_410 {
            let sample = ((2.0 * std::f32::consts::PI * 440.0 * frame as f32 / 44_100.0).sin()
                * i16::MAX as f32
                * 0.25) as i16;
            writer.write_sample(sample).unwrap();
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let asset =
            decode_asset(path.to_str().unwrap(), 48_000, |_| {}).expect("decode external wav");
        assert_eq!(asset.sample_rate, 48_000);
        assert_eq!(asset.channels.len(), 2);
        assert!(asset.frames >= 4_700);
        assert!(asset.peaks(0).is_some_and(|peaks| !peaks.is_empty()));
        std::fs::remove_file(path).ok();
    }
}
fn peaks(channels: &[Vec<f32>], bucket: usize) -> Vec<f32> {
    let frames = channels.first().map(Vec::len).unwrap_or(0);
    let mut out = Vec::with_capacity((frames / bucket + 1) * 2);
    let mut start = 0;
    while start < frames {
        let end = (start + bucket).min(frames);
        let (mut lo, mut hi) = (1.0_f32, -1.0_f32);
        for channel in channels {
            for sample in &channel[start..end] {
                lo = lo.min(*sample);
                hi = hi.max(*sample)
            }
        }
        out.push(if lo == 1.0 { 0.0 } else { lo });
        out.push(if hi == -1.0 { 0.0 } else { hi });
        start = end
    }
    out
}
