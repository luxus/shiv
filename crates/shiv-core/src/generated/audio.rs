//! Decode an uploaded hitsound to the mono 44.1 kHz samples the pinned
//! builder expects. A file that is already mono 44.1 kHz is mixed or copied.
//! Anything else is linearly resampled.

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const RATE: u32 = 44_100;
const MIN_RATE: u32 = 8_000;
const MAX_RATE: u32 = 384_000;
const MAX_SECONDS: u64 = 180;
#[allow(
    clippy::cast_possible_truncation,
    reason = "44100 * 180 fits in usize on all supported targets"
)]
const MAX_OUT_SAMPLES: usize = (RATE as usize) * (MAX_SECONDS as usize);

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("the file has no audio")]
    Empty,
    #[error("could not read the audio ({0})")]
    Decode(String),
}

/// Mono little-endian-ready `f32` samples at 44.1 kHz.
///
/// # Errors
///
/// [`AudioError::Empty`] when no samples exist, or [`AudioError::Decode`] on
/// unsupported sample rates, decoding failures, or audio longer than three
/// minutes.
pub fn decode_mono_44k(bytes: &[u8]) -> Result<Vec<f32>, AudioError> {
    if bytes.is_empty() {
        return Err(AudioError::Empty);
    }
    let cursor = Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), MediaSourceStreamOptions::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Decode(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| AudioError::Decode("no audio track".into()))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let rate = params.sample_rate.unwrap_or(0);
    if !(MIN_RATE..=MAX_RATE).contains(&rate) {
        return Err(AudioError::Decode(format!(
            "unsupported sample rate {rate} Hz"
        )));
    }
    let max_input_frames =
        usize::try_from(u64::from(rate).saturating_mul(MAX_SECONDS)).unwrap_or(usize::MAX);
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;
    let mut mono: Vec<f32> = Vec::new();
    let mut scratch: Option<(usize, symphonia::core::audio::SignalSpec, SampleBuffer<f32>)> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio_buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        let spec = *audio_buf.spec();
        let channels = spec.channels.count().max(1);
        let capacity = audio_buf.capacity();
        let needs_new = scratch
            .as_ref()
            .is_none_or(|(cap, prev_spec, _)| *cap < capacity || *prev_spec != spec);
        if needs_new {
            scratch = Some((
                capacity,
                spec,
                SampleBuffer::<f32>::new(capacity as u64, spec),
            ));
        }
        let Some((_, _, buf)) = scratch.as_mut() else {
            continue;
        };
        buf.copy_interleaved_ref(audio_buf);
        let samples = buf.samples();
        let frames = samples.len() / channels;
        if mono.len().saturating_add(frames) > max_input_frames {
            return Err(AudioError::Decode(
                "audio is longer than three minutes".into(),
            ));
        }
        if channels == 1 {
            mono.extend_from_slice(samples);
        } else {
            mono.reserve(frames);
            #[allow(
                clippy::cast_precision_loss,
                reason = "audio channel count is small (<= 64)"
            )]
            let scale = 1.0 / (channels as f32);
            for frame in samples.chunks_exact(channels) {
                let sum: f32 = frame.iter().copied().sum();
                mono.push(sum * scale);
            }
        }
    }
    if mono.is_empty() {
        return Err(AudioError::Empty);
    }
    let out = if rate == RATE {
        mono
    } else {
        resample(&mono, rate, RATE)?
    };
    if out.is_empty() {
        Err(AudioError::Empty)
    } else {
        Ok(out)
    }
}

fn resample(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>, AudioError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "input length is bounded by 3 minutes at <= 384 kHz (< 70M frames)"
    )]
    let out_len = ((input.len() as f64) * f64::from(to) / f64::from(from))
        .ceil()
        .max(1.0) as usize;
    if out_len > MAX_OUT_SAMPLES + 1 {
        return Err(AudioError::Decode(
            "audio is longer than three minutes".into(),
        ));
    }
    let mut out = vec![0.0; out_len];
    let last = input.len() - 1;
    for (i, sample) in out.iter_mut().enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "index is bounded by MAX_OUT_SAMPLES (~7.9M)"
        )]
        let pos = (i as f64) * f64::from(from) / f64::from(to);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pos is non-negative and clamped to last"
        )]
        let index = (pos.floor() as usize).min(last);
        let next = (index + 1).min(last);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "fractional interpolation weight in [0, 1)"
        )]
        let frac = (pos - index as f64) as f32;
        let a = input[index];
        let b = input[next];
        *sample = a + (b - a) * frac;
    }
    Ok(out)
}
