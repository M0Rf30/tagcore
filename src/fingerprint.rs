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

/// Decodes the first [`MAX_SECONDS`] of the default track through [`playcore::SymphoniaDecoder`]
/// to interleaved i16 and fingerprints it with the default algorithm. Bit-identical to
/// `fpcalc -length 120` for the same PCM. DSD streams are decimated to [`DSD_PCM_RATE`] PCM.
pub fn compute(path: &Path) -> Result<Fingerprint> {
    use playcore::{DecoderOptions, SymphoniaDecoder};

    // Surface a missing/unreadable file as an I/O error, anything else as unsupported.
    std::fs::File::open(path)?;
    let mut decoder = SymphoniaDecoder::open(path, DecoderOptions::default())
        .map_err(|_| Error::Unsupported(path.to_path_buf()))?;
    if decoder.is_dsd() {
        decoder
            .enable_pcm_conversion(DSD_PCM_RATE)
            .map_err(|e| Error::Decode(e.to_string()))?;
    }
    let declared = decoder.duration();
    let rate = decoder.sample_rate();
    let channels = usize::from(decoder.channels());
    let mut fp = chromaprint::Fingerprinter::new(chromaprint::Algorithm::default());
    fp.start(rate, u16::from(decoder.channels()))
        .map_err(|e| Error::Fingerprint(e.to_string()))?;

    let limit_samples = u64::from(rate) * u64::from(MAX_SECONDS) * channels as u64;
    let mut fed = 0u64;
    let mut total = 0u64;
    let mut buf = vec![0f32; channels.max(1) * 4096];
    let mut pcm: Vec<i16> = Vec::new();
    loop {
        let n = match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if total > 0 => {
                log::warn!("{}: stopping fingerprint decode: {e}", path.display());
                break;
            }
            Err(e) => return Err(Error::Decode(e.to_string())),
        };
        total += n as u64;
        if fed < limit_samples {
            let take = (n as u64).min(limit_samples - fed) as usize;
            pcm.clear();
            pcm.extend(
                buf[..take]
                    .iter()
                    .map(|&s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16),
            );
            fp.feed(&pcm)
                .map_err(|e| Error::Fingerprint(e.to_string()))?;
            fed += take as u64;
        }
        // Stop once enough audio is fed and the full duration is known without decoding on.
        if fed >= limit_samples && declared.is_some() {
            break;
        }
    }
    if total == 0 {
        return Err(Error::Decode(format!(
            "{}: no decodable audio",
            path.display()
        )));
    }
    fp.finish().map_err(|e| Error::Fingerprint(e.to_string()))?;

    let duration = match declared {
        Some(secs) => Duration::from_secs_f64(secs),
        None => Duration::from_secs_f64(
            (total / channels.max(1) as u64) as f64 / f64::from(rate.max(1)),
        ),
    };
    Ok(Fingerprint {
        fingerprint: fp.encode(),
        duration,
    })
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
