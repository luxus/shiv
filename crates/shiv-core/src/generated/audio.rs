//! Decode an uploaded hitsound to the mono 44.1 kHz samples the pinned
//! builder expects. A file that is already mono 44.1 kHz is mixed or copied.
//! Anything else is linearly resampled. That is not the browser's
//! `OfflineAudioContext`, which the site uses only for the other rates.

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const RATE: u32 = 44_100;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("the file has no audio")]
    Empty,
    #[error("could not read the audio ({0})")]
    Decode(String),
}

/// Mono little-endian-ready `f32` samples at 44.1 kHz.
pub fn decode_mono_44k(bytes: &[u8]) -> Result<Vec<f32>, AudioError> {
    if bytes.is_empty() {
        return Err(AudioError::Empty);
    }
    let cursor = Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
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
    if rate == 0 {
        return Err(AudioError::Decode("the file has no sample rate".into()));
    }
    let channels = params.channels.map_or(1, |c| c.count()).max(1);
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;
    let mut interleaved = Vec::new();
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
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        let spec = *decoded.spec();
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        interleaved.extend_from_slice(buf.samples());
    }
    if interleaved.is_empty() {
        return Err(AudioError::Empty);
    }
    let frames = interleaved.len() / channels;
    let mono = if channels == 1 {
        interleaved
    } else {
        let mut mixed = vec![0.0; frames];
        for (i, sample) in mixed.iter_mut().enumerate() {
            let mut sum = 0.0;
            for c in 0..channels {
                sum += interleaved.get(i * channels + c).copied().unwrap_or(0.0);
            }
            *sample = sum / channels as f32;
        }
        mixed
    };
    let out = if rate == RATE {
        mono
    } else {
        resample(&mono, rate, RATE)
    };
    if out.is_empty() {
        Err(AudioError::Empty)
    } else {
        Ok(out)
    }
}

fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let out_len = ((input.len() as f64) * f64::from(to) / f64::from(from))
        .ceil()
        .max(1.0) as usize;
    let mut out = vec![0.0; out_len];
    if input.is_empty() {
        return out;
    }
    let last = input.len() - 1;
    for (i, sample) in out.iter_mut().enumerate() {
        let pos = (i as f64) * f64::from(from) / f64::from(to);
        let index = (pos.floor() as usize).min(last);
        let next = (index + 1).min(last);
        let frac = (pos - index as f64) as f32;
        let a = input[index];
        let b = input[next];
        *sample = a + (b - a) * frac;
    }
    out
}
