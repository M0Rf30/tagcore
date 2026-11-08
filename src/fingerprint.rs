//! Chromaprint fingerprints via `chromaprint-next`, PCM decoded by Symphonia.

use std::path::Path;
use std::time::Duration;

use crate::{Error, Result};

/// PCM rate DSD streams are decimated to before fingerprinting.
pub const DSD_PCM_RATE: u32 = 44_100;

/// Seconds of audio fed to the fingerprinter (same as `fpcalc` default).
pub const MAX_SECONDS: u32 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Compressed base64 fingerprint (`fpcalc` output format).
    pub fingerprint: String,
    /// Full track duration.
    pub duration: Duration,
}

/// Decodes the first [`MAX_SECONDS`] of the default track to interleaved i16 and fingerprints
/// it with the default algorithm. Bit-identical to `fpcalc -length 120` for the same PCM. DSD
/// streams are decimated to [`DSD_PCM_RATE`] PCM first.
pub fn compute(path: &Path) -> Result<Fingerprint> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::units::Timestamp;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| Error::Unsupported(path.to_path_buf()))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Unsupported(path.to_path_buf()))?;
    let track_id = track.id;
    let mut params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Unsupported(path.to_path_buf()))?
        .clone();
    if is_dsd(&params) {
        // The fork's DSD decoder converts to PCM at the rate given in `extra_data`.
        params.extra_data = Some(DSD_PCM_RATE.to_le_bytes().to_vec().into_boxed_slice());
    }
    let declared_rate = params.sample_rate;
    let declared_frames = track.num_frames;
    let declared_time = match (track.time_base, track.duration) {
        (Some(tb), Some(d)) => i64::try_from(d.get())
            .ok()
            .and_then(|d| tb.calc_time(Timestamp::new(d)))
            .map(|t| t.as_nanos()),
        _ => None,
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| Error::Decode(e.to_string()))?;

    let mut fp = chromaprint::Fingerprinter::new(chromaprint::Algorithm::default());
    let mut started = false;
    let mut rate = 0u32;
    let mut channels = 0usize;
    let mut limit_frames = 0u64;
    let mut fed_frames = 0u64;
    let mut total_frames = 0u64;
    let mut pcm: Vec<i16> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(_)) | Err(SymError::DecodeError(_)) => continue,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if started {
                    log::warn!("{}: stopping fingerprint decode: {e}", path.display());
                    break;
                }
                return Err(Error::Decode(e.to_string()));
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::IoError(_)) | Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(Error::Decode(e.to_string())),
        };
        let frames = decoded.frames() as u64;
        if frames == 0 {
            continue;
        }
        total_frames += frames;
        if !started {
            rate = decoded.spec().rate();
            channels = decoded.spec().channels().count();
            let ch = u16::try_from(channels)
                .map_err(|_| Error::Fingerprint(format!("too many channels: {channels}")))?;
            fp.start(rate, ch)
                .map_err(|e| Error::Fingerprint(e.to_string()))?;
            started = true;
            limit_frames = u64::from(rate) * u64::from(MAX_SECONDS);
        }
        if fed_frames < limit_frames {
            let take = frames.min(limit_frames - fed_frames);
            decoded.copy_to_vec_interleaved(&mut pcm);
            pcm.truncate(take as usize * channels);
            fp.feed(&pcm)
                .map_err(|e| Error::Fingerprint(e.to_string()))?;
            fed_frames += take;
        }
        // Stop once enough audio is fed and the full duration is known without decoding on.
        if fed_frames >= limit_frames && (declared_frames.is_some() || declared_time.is_some()) {
            break;
        }
    }
    if !started {
        return Err(Error::Decode(format!(
            "{}: no decodable audio",
            path.display()
        )));
    }
    fp.finish().map_err(|e| Error::Fingerprint(e.to_string()))?;

    let duration = if let Some(n) = declared_frames {
        Duration::from_secs_f64(n as f64 / f64::from(declared_rate.unwrap_or(rate).max(1)))
    } else if let Some(ns) = declared_time {
        Duration::from_nanos(u64::try_from(ns).unwrap_or(0))
    } else {
        Duration::from_secs_f64(total_frames as f64 / f64::from(rate.max(1)))
    };
    Ok(Fingerprint {
        fingerprint: fp.encode(),
        duration,
    })
}

/// Whether `params` describe raw DSD (1-bit) audio.
pub fn is_dsd(params: &symphonia::core::codecs::audio::AudioCodecParameters) -> bool {
    params.codec == symphonia::default::formats::CODEC_TYPE_DSD
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5 s stereo 44.1 kHz FLAC; the expected string is `chromaprint::fingerprint_audio` on the
    /// PCM produced by `ffmpeg -f s16le -ac 2 -ar 44100` for the same file.
    #[test]
    fn flac_matches_direct_chromaprint_on_ffmpeg_pcm() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fingerprint/tone.flac");
        let fp = compute(&path).unwrap();
        assert_eq!(
            fp.fingerprint,
            "AQAAE4u4RdGURDgT8UEO7YiUOg12HQlD5M9xkxrM9Hhy_JgSTNOSI6aYQZ8O_eiLxskBAogiBBACBDJWCCMcEQY"
        );
        assert_eq!(fp.duration, Duration::from_secs(5));
    }

    #[test]
    fn missing_file_is_error() {
        assert!(compute(Path::new("/nonexistent/x.flac")).is_err());
    }
}
