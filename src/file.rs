//! Reading tags/properties with Symphonia and writing them with `symphonia-tagging`.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use symphonia_core::codecs::CodecParameters;
use symphonia_core::errors::Error as SymphoniaError;
use symphonia_core::formats::probe::Hint;
use symphonia_core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia_core::io::MediaSourceStream;
use symphonia_core::meta::well_known::{
    METADATA_ID_APEV1, METADATA_ID_APEV2, METADATA_ID_ID3V1, METADATA_ID_ID3V2, METADATA_ID_ISOMP4,
};
use symphonia_core::meta::{MetadataOptions, MetadataRevision, Visual};
pub use symphonia_tagging::{
    ApePolicy, Container, FlacId3, Id3Encoding, Id3v1Policy, Id3v2Version, RiffInfoEncoding,
    RiffInfoPolicy, VorbisRatingScale, WriteOptions,
};

use crate::metadata::{CoverImage, Metadata};
use crate::{Error, Result};

mod map;

use map::{Item, Source};

/// Technical properties shown in the info panel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioProperties {
    /// Container short name, e.g. `FLAC`, `MP3`, `MP4`, `Ogg`.
    pub format: String,
    /// Codec short name, e.g. `flac`, `mp3`, `aac`, `vorbis`, `opus`.
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bits_per_sample: Option<u32>,
    /// Average bitrate in bits per second.
    pub bitrate: Option<u32>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct LoadedFile {
    /// Tags as read; `~length` set from `properties.duration`. Tags without a Picard mapping are
    /// kept under their raw native key (lowercased) so a load/save round trip preserves them.
    pub metadata: Metadata,
    pub properties: AudioProperties,
}

/// The highest rating (`~rating` runs from 0 to this).
pub const MAX_RATING: u8 = 5;

/// Containers Symphonia has no demuxer for; tags are read with `symphonia_tagging::read_tags`
/// (AC-3/E-AC-3, TAK, OptimFROG, TTA). Not playable through `playcore`.
pub const TAG_ONLY_EXTENSIONS: &[&str] = &["ac3", "eac3", "tak", "ofr", "ofs", "tta"];

/// File extensions the loader accepts (lowercase, no dot, sorted): every audio-only format
/// `playcore` can demux, plus [`TAG_ONLY_EXTENSIONS`].
pub static SUPPORTED_EXTENSIONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut exts: Vec<&'static str> = playcore::format::SUPPORTED_EXTENSIONS
        .iter()
        .copied()
        .filter(|e| playcore::format::is_audio_only_extension(e))
        .chain(TAG_ONLY_EXTENSIONS.iter().copied())
        .collect();
    exts.sort_unstable();
    exts.dedup();
    exts
});

pub fn is_supported(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        playcore::format::is_audio_only_extension(&e) || TAG_ONLY_EXTENSIONS.contains(&e.as_str())
    })
}

/// Probes `path`, reads every tag and embedded picture (all metadata revisions, latest wins) and
/// the audio properties of the default track. Containers Symphonia has no demuxer for (AC-3,
/// TAK, OptimFROG, TTA) are read with `symphonia_tagging::read_tags`. Only `~length` is derived
/// here; hosts add file-system variables (`~filename`, `~filesize`, ...) themselves.
pub fn load(path: &Path) -> Result<LoadedFile> {
    let container = symphonia_tagging::sniff(path).unwrap_or(Container::Other);
    let tag_only = matches!(
        container,
        Container::Ac3 | Container::Tak | Container::OptimFrog | Container::Tta
    );

    let (mut metadata, properties) = if tag_only {
        load_tag_only(path, container)?
    } else {
        match load_probed(path, container) {
            Ok(loaded) => loaded,
            // Raw ADTS without a probeable stream still carries its APEv2 tag.
            Err(_) if container == Container::Aac => load_tag_only(path, container)?,
            Err(e) => return Err(e),
        }
    };

    if let Some(duration) = properties.duration {
        metadata.set_length(duration);
    }

    Ok(LoadedFile {
        metadata,
        properties,
    })
}

fn load_probed(path: &Path, container: Container) -> Result<(Metadata, AudioProperties)> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| probe_error(e, path))?;

    let mut metadata = collect_metadata(format.as_mut());
    let properties = audio_properties(format.as_ref(), size);

    // The probe does not necessarily find the APEv2 tag of an ADTS stream.
    if container == Container::Aac
        && let Ok(read) = symphonia_tagging::read_tags(path)
    {
        merge_read_tags(&mut metadata, &read, Source::Ape);
    }

    Ok((metadata, properties))
}

/// Tags and properties of the containers Symphonia has no demuxer for.
fn load_tag_only(path: &Path, container: Container) -> Result<(Metadata, AudioProperties)> {
    let read = symphonia_tagging::read_tags(path).map_err(|e| tagging_error(e, path, false))?;
    let source = if container == Container::Tta {
        Source::Id3v2
    } else {
        Source::Ape
    };
    let mut metadata = Metadata::new();
    merge_read_tags(&mut metadata, &read, source);

    let size = std::fs::metadata(path)?.len();
    let properties = match container {
        Container::Tta => tta_properties(path, size)?,
        Container::Ac3 => ac3_properties(path, size)?,
        Container::Tak => AudioProperties {
            format: "TAK".into(),
            codec: "tak".into(),
            ..Default::default()
        },
        Container::OptimFrog => AudioProperties {
            format: "OptimFROG".into(),
            codec: "optimfrog".into(),
            ..Default::default()
        },
        _ => AudioProperties {
            format: "AAC".into(),
            codec: "aac".into(),
            ..Default::default()
        },
    };
    Ok((metadata, properties))
}

/// Merges tags from `symphonia_tagging::read_tags` (highest priority first). A value that is a
/// prefix of one already read is a truncated ID3v1 copy and dropped.
fn merge_read_tags(metadata: &mut Metadata, read: &symphonia_tagging::ReadTags, source: Source) {
    let mut items = Vec::new();
    for tag in &read.tags {
        let mut one = Vec::new();
        map::read_tag(tag, source, &mut one);
        for item in one {
            let truncated = items
                .iter()
                .any(|i: &Item| i.key == item.key && i.value.starts_with(&item.value));
            if !truncated {
                items.push(item);
            }
        }
    }
    let images = read.visuals.iter().map(map::from_visual).collect();
    apply(
        metadata,
        Revision {
            source,
            items,
            images,
        },
    );
}

/// Reads up to `len` bytes of the audio stream after its leading ID3v2 tags.
fn audio_head(path: &Path, len: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = File::open(path)?;
    let mut start = 0u64;
    let mut header = [0u8; 10];
    loop {
        file.seek(SeekFrom::Start(start))?;
        if file.read(&mut header)? < 10 || &header[..3] != b"ID3" {
            break;
        }
        let size = header[6..10]
            .iter()
            .fold(0u64, |acc, b| (acc << 7) | u64::from(b & 0x7f));
        start += 10 + size + if header[5] & 0x10 != 0 { 10 } else { 0 };
    }
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// TTA1 header: `format u16, channels u16, bits u16, sample rate u32, samples u32` (LE).
fn tta_properties(path: &Path, size: u64) -> Result<AudioProperties> {
    let mut properties = AudioProperties {
        format: "TTA".into(),
        codec: "tta".into(),
        ..Default::default()
    };
    let head = audio_head(path, 22)?;
    if head.len() >= 22 && &head[..4] == b"TTA1" {
        let u16_at = |i: usize| u32::from(u16::from_le_bytes([head[i], head[i + 1]]));
        let u32_at =
            |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        properties.channels = Some(u16_at(6));
        properties.bits_per_sample = Some(u16_at(8));
        let rate = u32_at(10);
        properties.sample_rate = Some(rate).filter(|r| *r > 0);
        let samples = u32_at(14);
        if rate > 0 && samples > 0 {
            let duration = Duration::from_secs_f64(f64::from(samples) / f64::from(rate));
            properties.duration = Some(duration).filter(|d| !d.is_zero());
            properties.bitrate = bitrate_of(size, duration);
        }
    }
    Ok(properties)
}

/// AC-3 sync frame: sample rate and bit rate from `fscod` / `frmsizecod`. E-AC-3 (`bsid > 10`)
/// has neither in that form, so only the format is reported for it.
fn ac3_properties(path: &Path, size: u64) -> Result<AudioProperties> {
    const RATES: [u32; 3] = [48_000, 44_100, 32_000];
    const KBPS: [u32; 19] = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
    ];

    let mut properties = AudioProperties {
        format: "AC3".into(),
        codec: "ac3".into(),
        ..Default::default()
    };
    let head = audio_head(path, 8)?;
    if head.len() >= 6 && head[0] == 0x0b && head[1] == 0x77 {
        let (fscod, frmsizecod, bsid) = (head[4] >> 6, usize::from(head[4] & 0x3f), head[5] >> 3);
        if bsid > 10 {
            properties.format = "E-AC3".into();
            properties.codec = "eac3".into();
        } else if let (Some(rate), Some(kbps)) =
            (RATES.get(usize::from(fscod)), KBPS.get(frmsizecod >> 1))
        {
            properties.sample_rate = Some(*rate);
            properties.bitrate = Some(kbps * 1000);
            let duration = Duration::from_secs_f64(size as f64 * 8.0 / f64::from(kbps * 1000));
            properties.duration = Some(duration).filter(|d| !d.is_zero());
        }
    }
    Ok(properties)
}

fn bitrate_of(size: u64, duration: Duration) -> Option<u32> {
    let bits_per_second = size as f64 * 8.0 / duration.as_secs_f64();
    (bits_per_second.is_finite() && bits_per_second <= f64::from(u32::MAX))
        .then_some(bits_per_second.round() as u32)
}

fn probe_error(err: SymphoniaError, path: &Path) -> Error {
    match err {
        SymphoniaError::IoError(e) => Error::Io(e),
        SymphoniaError::Unsupported(_) => Error::Unsupported(path.to_path_buf()),
        other => Error::Decode(other.to_string()),
    }
}

/// What one metadata revision contributes.
struct Revision {
    source: Source,
    items: Vec<Item>,
    images: Vec<CoverImage>,
}

/// Merges all metadata revisions of `format`, oldest first, so the latest wins per key. An ID3v1
/// tag (30-character fields) never overrides a richer tag, whatever its position in the file.
fn collect_metadata(format: &mut dyn FormatReader) -> Metadata {
    let mut revisions = Vec::new();

    let mut log = format.metadata();
    loop {
        if let Some(revision) = log.current() {
            revisions.push(read_revision(revision));
        }
        if log.pop().is_none() {
            break;
        }
    }

    let mut metadata = Metadata::new();
    let (v1, others): (Vec<_>, Vec<_>) = revisions
        .into_iter()
        .partition(|r| r.source == Source::Id3v1);

    for revision in v1.into_iter().chain(others) {
        apply(&mut metadata, revision);
    }

    metadata
}

fn read_revision(revision: &MetadataRevision) -> Revision {
    let id = revision.info.metadata;
    let source = if id == METADATA_ID_ID3V2 {
        Source::Id3v2
    } else if id == METADATA_ID_ID3V1 {
        Source::Id3v1
    } else if id == METADATA_ID_ISOMP4 {
        Source::Mp4
    } else if id == METADATA_ID_APEV1 || id == METADATA_ID_APEV2 {
        Source::Ape
    } else {
        Source::Other
    };

    let mut items = Vec::new();
    for tag in &revision.media.tags {
        map::read_tag(tag, source, &mut items);
    }

    let images = revision
        .media
        .visuals
        .iter()
        .map(map::from_visual)
        .collect();

    Revision {
        source,
        items,
        images,
    }
}

/// Replaces, per key, what `metadata` holds with what `revision` carries.
fn apply(metadata: &mut Metadata, revision: Revision) {
    let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for item in revision.items.iter().filter(|i| !i.weak) {
        push_unique(&mut by_key, &item.key, &item.value);
    }
    for item in revision.items.iter().filter(|i| i.weak) {
        if !by_key.contains_key(&item.key) {
            push_unique(&mut by_key, &item.key, &item.value);
        }
    }

    for (key, values) in by_key {
        metadata.set_all(&key, values);
    }

    if !revision.images.is_empty() {
        metadata.images = revision.images;
    }
}

fn push_unique(by_key: &mut BTreeMap<String, Vec<String>>, key: &str, value: &str) {
    let values = by_key.entry(key.to_owned()).or_default();
    if !values.iter().any(|v| v == value) {
        values.push(value.to_owned());
    }
}

fn audio_properties(format: &dyn FormatReader, file_size: u64) -> AudioProperties {
    let mut properties = AudioProperties {
        format: format_name(format.format_info().short_name),
        ..Default::default()
    };

    let track = format
        .default_track(TrackType::Audio)
        .or_else(|| format.first_track(TrackType::Audio));
    let Some(track) = track else {
        return properties;
    };

    if let Some(CodecParameters::Audio(params)) = &track.codec_params {
        properties.codec = symphonia::default::get_codecs()
            .get_audio_decoder(params.codec)
            .map_or_else(
                || "unknown".to_owned(),
                |d| d.codec.info.short_name.to_owned(),
            );
        properties.sample_rate = params.sample_rate;
        properties.channels = params
            .channels
            .as_ref()
            .and_then(|c| u32::try_from(c.count()).ok());
        properties.bits_per_sample = params.bits_per_sample;
    }

    properties.duration = track
        .time_base
        .zip(track.duration)
        .and_then(|(base, duration)| base.calc_duration(duration))
        .and_then(|time| u64::try_from(time.as_nanos()).ok())
        .map(Duration::from_nanos)
        .or_else(|| {
            let frames = track.num_frames?;
            let rate = properties.sample_rate.filter(|r| *r > 0)?;
            Some(Duration::from_secs_f64(frames as f64 / f64::from(rate)))
        })
        .filter(|d| !d.is_zero());

    // The containers do not state a bitrate, so it is the whole file over the whole duration.
    properties.bitrate = properties.duration.and_then(|d| {
        let bits_per_second = file_size as f64 * 8.0 / d.as_secs_f64();
        (bits_per_second.is_finite() && bits_per_second <= f64::from(u32::MAX))
            .then_some(bits_per_second.round() as u32)
    });

    properties
}

fn format_name(short_name: &str) -> String {
    match short_name {
        "isomp4" => "MP4",
        "ogg" => "Ogg",
        "wave" | "wav" | "riff" => "WAV",
        "wavpack" => "WavPack",
        "musepack" => "MPC",
        other => return other.to_ascii_uppercase(),
    }
    .to_owned()
}

/// Identifies `path`'s container from its leading bytes (never from the extension).
pub fn sniff(path: &Path) -> Result<Container> {
    symphonia_tagging::sniff(path).map_err(|e| tagging_error(e, path, false))
}

/// Replaces the file's tags with the visible keys of `metadata` (and its `~rating` when
/// `options.rating_email` is set) and its images, through `symphonia_tagging::write_file`.
///
/// With `preserve_timestamps` the file's access and modification times are restored afterwards.
pub fn save(
    path: &Path,
    metadata: &Metadata,
    options: &WriteOptions,
    preserve_timestamps: bool,
) -> Result<()> {
    let times = if preserve_timestamps {
        Some(file_times(path)?)
    } else {
        None
    };
    let container = symphonia_tagging::sniff(path).map_err(|e| tagging_error(e, path, false))?;
    if !container.is_writable() {
        return Err(Error::Unsupported(path.to_path_buf()));
    }

    let mut tags = map::to_tags(metadata, container);
    if options.rating_email.is_some() {
        tags.extend(map::rating_tag(metadata, container));
    }
    let visuals: Vec<Visual> = metadata.images.iter().map(map::to_visual).collect();

    // (the times were captured before `sniff` read the file and moved its access time)
    symphonia_tagging::write_file(path, &tags, &visuals, options)
        .map_err(|e| tagging_error(e, path, true))?;
    if let Some(times) = times {
        restore_times(path, times)?;
    }
    Ok(())
}

fn file_times(path: &Path) -> Result<std::fs::FileTimes> {
    let meta = std::fs::metadata(path)?;
    Ok(std::fs::FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?))
}

fn restore_times(path: &Path, times: std::fs::FileTimes) -> Result<()> {
    File::options().write(true).open(path)?.set_times(times)?;
    Ok(())
}

fn tagging_error(err: symphonia_tagging::Error, path: &Path, writing: bool) -> Error {
    match err {
        symphonia_tagging::Error::Io(e) => Error::Io(e),
        symphonia_tagging::Error::Unsupported(_) if !writing => {
            Error::Unsupported(path.to_path_buf())
        }
        other => Error::Tagging(format!("{}: {other}", path.display())),
    }
}
