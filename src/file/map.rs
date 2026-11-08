//! Mapping between Picard tag names and Symphonia tags.
//!
//! Reading: every [`Tag`] of every metadata revision becomes zero or more `(name, value)` items.
//! Writing: every visible key of a [`Metadata`] becomes `Tag`s that the `symphonia-tagging`
//! serializers write to the native field Picard uses and the readers map back to the same
//! `StandardTag`.
//!
//! The one non-obvious part is the MusicBrainz identifiers. Picard's `musicbrainz_recordingid`
//! is stored on disk as the *Track* Id (ID3 `UFID`, Vorbis/APE/MP4 `MusicBrainz Track Id`) and
//! its `musicbrainz_trackid` as the *Release Track* Id, so they map to Symphonia's
//! `MusicBrainzRecordingId` and `MusicBrainzReleaseTrackId`. The readers of Vorbis, APE and MP4
//! report the on-disk Track Id as `MusicBrainzTrackId`, which therefore also means "recording".

use std::sync::Arc;

use symphonia_core::meta::{
    RawTag, RawTagSubField, RawValue, StandardTag, StandardVisualKey, Tag, Visual,
};
use symphonia_tagging::Container;

use crate::metadata::{CoverImage, ImageType, Metadata};

/// Raw key of the iTunes free-form atom Picard stores the `artists` tag in. The MP4 reader maps
/// it to `StandardTag::Artist`, so the key is what tells it apart from `©ART`.
const MP4_ARTISTS_KEY: &str = "com.apple.iTunes:ARTISTS";
const ITUNES_MEAN_PREFIX: &str = "com.apple.itunes:";

/// Which native tag format a revision was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Id3v1,
    Id3v2,
    Mp4,
    Ape,
    Other,
}

/// One value read from a tag.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub key: String,
    pub value: String,
    /// Only used when the same revision has no non-weak value for `key` (e.g. `TYER` vs `TDRC`).
    pub weak: bool,
}

macro_rules! text_tags {
    ($($name:literal => $variant:ident),+ $(,)?) => {
        fn text_ctor(name: &str) -> Option<fn(Arc<String>) -> StandardTag> {
            match name {
                $($name => Some(StandardTag::$variant),)+
                _ => None,
            }
        }

        fn text_name(std: &StandardTag) -> Option<(&'static str, &Arc<String>)> {
            match std {
                $(StandardTag::$variant(v) => Some(($name, v)),)+
                _ => None,
            }
        }

        #[cfg(test)]
        const TEXT_NAMES: &[&str] = &[$($name),+];
    };
}

macro_rules! num_tags {
    ($ty:ty, $ctor:ident, $name_fn:ident, $names:ident; $($name:literal => $variant:ident),+ $(,)?) => {
        fn $ctor(name: &str) -> Option<fn($ty) -> StandardTag> {
            match name {
                $($name => Some(StandardTag::$variant),)+
                _ => None,
            }
        }

        fn $name_fn(std: &StandardTag) -> Option<(&'static str, $ty)> {
            match std {
                $(StandardTag::$variant(v) => Some(($name, *v)),)+
                _ => None,
            }
        }

        #[cfg(test)]
        const $names: &[&str] = &[$($name),+];
    };
}

text_tags! {
    "accurateripcount" => AccurateRipCount,
    "accurateripcountalloffsets" => AccurateRipCountAllOffsets,
    "accurateripcountwithoffset" => AccurateRipCountWithOffset,
    "accurateripcrc" => AccurateRipCrc,
    "accurateripdiscid" => AccurateRipDiscId,
    "accurateripid" => AccurateRipId,
    "accurateripoffset" => AccurateRipOffset,
    "accurateripresult" => AccurateRipResult,
    "accurateriptotal" => AccurateRipTotal,
    "acoustid_fingerprint" => AcoustIdFingerprint,
    "acoustid_id" => AcoustIdId,
    "actor" => Actor,
    "album" => Album,
    "albumartist" => AlbumArtist,
    "arranger" => Arranger,
    "artdirector" => ArtDirector,
    "artist" => Artist,
    "assistantdirector" => AssistantDirector,
    "author" => Author,
    "cdtoc" => CdToc,
    "chaptertitle" => ChapterTitle,
    "choregrapher" => Choregrapher,
    "cinematographer" => Cinematographer,
    "collectiontitle" => CollectionTitle,
    "comment" => Comment,
    "composer" => Composer,
    "conductor" => Conductor,
    "contentrating" => ContentRating,
    "contenttype" => ContentType,
    "coproducer" => Coproducer,
    "copyright" => Copyright,
    "costumedesigner" => CostumeDesigner,
    "ctdbdiscconfidence" => CueToolsDbDiscConfidence,
    "ctdbtrackconfidence" => CueToolsDbTrackConfidence,
    "description" => Description,
    "digitizeddate" => DigitizedDate,
    "director" => Director,
    "discsubtitle" => DiscSubtitle,
    "distributor" => Distributor,
    "editedby" => EditedBy,
    "editiontitle" => EditionTitle,
    "encodedby" => EncodedBy,
    "encoder" => Encoder,
    "encodersettings" => EncoderSettings,
    "encodingdate" => EncodingDate,
    "engineer" => Engineer,
    "ensemble" => Ensemble,
    "executiveproducer" => ExecutiveProducer,
    "genre" => Genre,
    "grouping" => Grouping,
    "asin" => IdentAsin,
    "barcode" => IdentBarcode,
    "catalognumber" => IdentCatalogNumber,
    "eanupn" => IdentEanUpn,
    "isbn" => IdentIsbn,
    "isrc" => IdentIsrc,
    "lccn" => IdentLccn,
    "productnumber" => IdentPn,
    "podcastid" => IdentPodcast,
    "upc" => IdentUpc,
    "imdbtitleid" => ImdbTitleId,
    "key" => InitialKey,
    "internetradioname" => InternetRadioName,
    "internetradioowner" => InternetRadioOwner,
    "keywords" => Keywords,
    "label" => Label,
    "labelcode" => LabelCode,
    "language" => Language,
    "license" => License,
    "lyricist" => Lyricist,
    "lyrics" => Lyrics,
    "measure" => Measure,
    "media" => MediaFormat,
    "djmixer" => MixDj,
    "mixer" => MixEngineer,
    "mood" => Mood,
    "movementname" => MovementName,
    "movietitle" => MovieTitle,
    "mp3gain_album_minmax" => Mp3GainAlbumMinMax,
    "mp3gain_minmax" => Mp3GainMinMax,
    "mp3gain_undo" => Mp3GainUndo,
    "musicbrainz_albumartistid" => MusicBrainzAlbumArtistId,
    "musicbrainz_albumid" => MusicBrainzAlbumId,
    "musicbrainz_artistid" => MusicBrainzArtistId,
    "musicbrainz_discid" => MusicBrainzDiscId,
    "musicbrainz_genreid" => MusicBrainzGenreId,
    "musicbrainz_labelid" => MusicBrainzLabelId,
    "musicbrainz_originalalbumid" => MusicBrainzOriginalAlbumId,
    "musicbrainz_originalartistid" => MusicBrainzOriginalArtistId,
    "musicbrainz_recordingid" => MusicBrainzRecordingId,
    "musicbrainz_releasegroupid" => MusicBrainzReleaseGroupId,
    "releasestatus" => MusicBrainzReleaseStatus,
    "musicbrainz_trackid" => MusicBrainzReleaseTrackId,
    "releasetype" => MusicBrainzReleaseType,
    "musicbrainz_trmid" => MusicBrainzTrmId,
    "musicbrainz_workid" => MusicBrainzWorkId,
    "narrator" => Narrator,
    "opus" => Opus,
    "originalalbum" => OriginalAlbum,
    "originalartist" => OriginalArtist,
    "originalfile" => OriginalFile,
    "originallyricist" => OriginalLyricist,
    "originalrecordingdate" => OriginalRecordingDate,
    "originalrecordingtime" => OriginalRecordingTime,
    "originaldate" => OriginalReleaseDate,
    "originalreleasetime" => OriginalReleaseTime,
    "originalwriter" => OriginalWriter,
    "owner" => Owner,
    "part" => Part,
    "parttitle" => PartTitle,
    "performer" => Performer,
    "period" => Period,
    "podcastcategory" => PodcastCategory,
    "podcastdescription" => PodcastDescription,
    "podcastkeywords" => PodcastKeywords,
    "producer" => Producer,
    "productioncopyright" => ProductionCopyright,
    "productiondesigner" => ProductionDesigner,
    "productionstudio" => ProductionStudio,
    "purchasedate" => PurchaseDate,
    "date" => RecordingDate,
    "recordinglocation" => RecordingLocation,
    "recordingtime" => RecordingTime,
    "releasecountry" => ReleaseCountry,
    "releasedate" => ReleaseDate,
    "releasetime" => ReleaseTime,
    "remixer" => Remixer,
    "replaygain_album_gain" => ReplayGainAlbumGain,
    "replaygain_album_peak" => ReplayGainAlbumPeak,
    "replaygain_album_range" => ReplayGainAlbumRange,
    "replaygain_reference_loudness" => ReplayGainReferenceLoudness,
    "replaygain_track_gain" => ReplayGainTrackGain,
    "replaygain_track_peak" => ReplayGainTrackPeak,
    "replaygain_track_range" => ReplayGainTrackRange,
    "screenplayauthor" => ScreenplayAuthor,
    "script" => Script,
    "soloist" => Soloist,
    "albumsort" => SortAlbum,
    "albumartistsort" => SortAlbumArtist,
    "artistsort" => SortArtist,
    "sortcollectiontitle" => SortCollectionTitle,
    "composersort" => SortComposer,
    "sorteditiontitle" => SortEditionTitle,
    "sortmovietitle" => SortMovieTitle,
    "sortopustitle" => SortOpusTitle,
    "sortparttitle" => SortPartTitle,
    "titlesort" => SortTrackTitle,
    "sorttvepisodetitle" => SortTvEpisodeTitle,
    "sorttvseasontitle" => SortTvSeasonTitle,
    "sorttvseriestitle" => SortTvSeriesTitle,
    "sortvolumetitle" => SortVolumeTitle,
    "subject" => Subject,
    "summary" => Summary,
    "synopsis" => Synopsis,
    "taggingdate" => TaggingDate,
    "termsofuse" => TermsOfUse,
    "thanks" => Thanks,
    "tmdbmovieid" => TmdbMovieId,
    "tmdbseriesid" => TmdbSeriesId,
    "subtitle" => TrackSubtitle,
    "title" => TrackTitle,
    "tuning" => Tuning,
    "tvdbepisodeid" => TvdbEpisodeId,
    "tvdbmovieid" => TvdbMovieId,
    "tvdbseriesid" => TvdbSeriesId,
    "tvepisodetitle" => TvEpisodeTitle,
    "tvnetwork" => TvNetwork,
    "tvseasontitle" => TvSeasonTitle,
    "tvseriestitle" => TvSeriesTitle,
    "url" => Url,
    "urlartist" => UrlArtist,
    "urlcopyright" => UrlCopyright,
    "urlinternetradio" => UrlInternetRadio,
    "urllabel" => UrlLabel,
    "urlofficial" => UrlOfficial,
    "urlpayment" => UrlPayment,
    "urlpodcast" => UrlPodcast,
    "urlpurchase" => UrlPurchase,
    "urlsource" => UrlSource,
    "version" => Version,
    "volumetitle" => VolumeTitle,
    "work" => Work,
    "writer" => Writer,
    "writtendate" => WrittenDate,
}

num_tags! { u64, num_ctor, num_name, NUM_NAMES;
    "tracknumber" => TrackNumber,
    "totaltracks" => TrackTotal,
    "discnumber" => DiscNumber,
    "totaldiscs" => DiscTotal,
    "bpm" => Bpm,
    "movementnumber" => MovementNumber,
    "movementtotal" => MovementTotal,
    "opusnumber" => OpusNumber,
    "partnumber" => PartNumber,
    "parttotal" => PartTotal,
    "playcounter" => PlayCounter,
    "tvepisodenumber" => TvEpisodeNumber,
    "tvepisodetotal" => TvEpisodeTotal,
    "tvseasonnumber" => TvSeasonNumber,
    "tvseasontotal" => TvSeasonTotal,
    "volumenumber" => VolumeNumber,
    "volumetotal" => VolumeTotal,
}

num_tags! { u16, year_ctor, year_name, YEAR_NAMES;
    "originalyear" => OriginalReleaseYear,
    "originalrecordingyear" => OriginalRecordingYear,
    "releaseyear" => ReleaseYear,
}

// ---------------------------------------------------------------------------------------------
// Reading.
// ---------------------------------------------------------------------------------------------

/// Whether the `date` tag lives in the release-date field of this native format (iTunes `©day`,
/// APE `Year`) rather than in a recording-date field.
fn date_is_release_date(container: Container) -> bool {
    matches!(
        container,
        Container::Mp4 | Container::WavPack | Container::MonkeysAudio | Container::Musepack
    )
}

fn strong(key: &str, value: impl Into<String>) -> Item {
    Item {
        key: key.to_owned(),
        value: value.into(),
        weak: false,
    }
}

/// Picard's `RATING_STEPS - 1`.
const MAX_STEP: u32 = crate::file::MAX_RATING as u32;

/// A rating in parts per million of the scale as a `0..=5` step (`round(r * (STEPS - 1))`).
fn rating_step(ppm: u32) -> u32 {
    ((u64::from(ppm.min(1_000_000)) * u64::from(MAX_STEP) + 500_000) / 1_000_000) as u32
}

/// The inverse of [`rating_step`]; `None` unless `value` is a number within `0..=5`.
pub(super) fn rating_ppm(value: &str) -> Option<u32> {
    let step = value.trim().parse::<f64>().ok()?.round();
    (0.0..=f64::from(MAX_STEP))
        .contains(&step)
        .then(|| (step as u32) * 1_000_000 / MAX_STEP)
}

/// Converts one tag read from a file into the items it carries.
pub(super) fn read_tag(tag: &Tag, source: Source, out: &mut Vec<Item>) {
    if let Some(std) = &tag.std
        && let Some(item) = from_std(std, tag, source)
    {
        out.push(item);
        return;
    }

    read_raw(tag, source, out);
}

fn from_std(std: &StandardTag, tag: &Tag, source: Source) -> Option<Item> {
    use StandardTag as S;

    match std {
        // The on-disk Track Id is Picard's recording id (see the module docs).
        S::MusicBrainzTrackId(v) | S::MusicBrainzRecordingId(v) => {
            return Some(strong("musicbrainz_recordingid", v.as_str()));
        }
        S::ReleaseDate(v) if matches!(source, Source::Mp4 | Source::Ape) => {
            return Some(strong("date", v.as_str()));
        }
        S::RecordingYear(year) => {
            return Some(Item {
                key: "date".into(),
                value: year.to_string(),
                weak: true,
            });
        }
        S::Artist(v) if source == Source::Mp4 && is_mp4_artists_key(&tag.raw.key) => {
            return Some(strong("artists", v.as_str()));
        }
        S::CompilationFlag(flag) => {
            return Some(strong("compilation", if *flag { "1" } else { "0" }));
        }
        // POPM / Vorbis RATING: parts per million of the scale, stored as Picard's `~rating` step.
        S::Rating(ppm) => return Some(strong("~rating", rating_step(*ppm).to_string())),
        // A comment with a description is `comment:<description>` (Picard), not `comment`.
        S::Comment(v) if source == Source::Id3v2 => {
            if let Some(description) = comm_description(tag) {
                let key = format!("comment:{}", description.to_lowercase());
                return Some(strong(&key, v.as_str()));
            }
        }
        _ => {}
    }

    if let Some((name, value)) = text_name(std) {
        return Some(strong(name, value.as_str()));
    }
    if let Some((name, value)) = num_name(std) {
        return Some(strong(name, value.to_string()));
    }
    if let Some((name, value)) = year_name(std) {
        return Some(strong(name, value.to_string()));
    }

    None
}

fn is_mp4_artists_key(raw_key: &str) -> bool {
    raw_key
        .trim_start_matches("----:")
        .eq_ignore_ascii_case(MP4_ARTISTS_KEY)
}

/// Keeps tags that have no `StandardTag` (or one this module does not know) under their raw key,
/// lowercased, so a load/save round trip preserves them.
fn read_raw(tag: &Tag, source: Source, out: &mut Vec<Item>) {
    let Some(key) = raw_name(tag, source) else {
        return;
    };

    match &tag.raw.value {
        RawValue::String(s) => push_raw(out, &key, s),
        RawValue::StringList(list) => list.iter().for_each(|s| push_raw(out, &key, s)),
        RawValue::SignedInt(n) => push_raw(out, &key, &n.to_string()),
        RawValue::UnsignedInt(n) => push_raw(out, &key, &n.to_string()),
        RawValue::Float(n) => push_raw(out, &key, &n.to_string()),
        RawValue::Boolean(b) => push_raw(out, &key, if *b { "1" } else { "0" }),
        // Binary payloads (`PRIV`, `POPM`, ...) are preserved by the writer, not by the tag map.
        _ => {}
    }
}

fn push_raw(out: &mut Vec<Item>, key: &str, value: &str) {
    // Leading blanks are part of iTunes' `iTunNORM`/`iTunSMPB` values, so only NULs are trimmed.
    let value = value.trim_matches('\0');
    if !value.trim().is_empty() {
        out.push(strong(key, value));
    }
}

/// The lowercase Picard-side name of a raw tag, or `None` if it is noise.
fn raw_name(tag: &Tag, source: Source) -> Option<String> {
    let raw_key = tag.raw.key.as_str();

    let name: &str = match source {
        Source::Id3v2 if matches!(raw_key, "TXXX" | "TXX") => {
            // A `TXXX` frame names itself in a sub-field. Without one it is unusable.
            tag.raw.sub_fields.as_deref()?.iter().find_map(|f| {
                match (&f.value, f.field.as_str()) {
                    (RawValue::String(s), "DESCRIPTION") => Some(s.as_str()),
                    _ => None,
                }
            })?
        }
        // Any other raw ID3v2 tag is a frame the tag map does not carry (`TLEN`, `PRIV`, `TRCK`
        // that failed to parse, ...). The writer preserves or rebuilds those natively.
        Source::Id3v2 if is_frame_id(raw_key) => return None,
        Source::Mp4 => {
            let key = raw_key.trim_start_matches("----:");
            match key.get(..ITUNES_MEAN_PREFIX.len()) {
                Some(mean) if mean.eq_ignore_ascii_case(ITUNES_MEAN_PREFIX) => {
                    &key[ITUNES_MEAN_PREFIX.len()..]
                }
                _ => key,
            }
        }
        _ => raw_key,
    };

    let name = name.trim().to_lowercase();

    if name.is_empty() || name.starts_with('~') {
        None
    } else {
        Some(name)
    }
}

fn is_frame_id(key: &str) -> bool {
    matches!(key.len(), 3 | 4)
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

// ---------------------------------------------------------------------------------------------
// Writing.
// ---------------------------------------------------------------------------------------------

/// Builds the tags to write for the visible keys of `md`.
pub(super) fn to_tags(md: &Metadata, container: Container) -> Vec<Tag> {
    let mut tags = Vec::new();

    for key in md.visible_keys() {
        let values = md.get_all(key);

        match key {
            "tracknumber" => {
                let (number, total) = (StandardTag::TrackNumber, StandardTag::TrackTotal);
                push_number_and_total(&mut tags, md, key, "totaltracks", number, total);
            }
            "discnumber" => {
                let (number, total) = (StandardTag::DiscNumber, StandardTag::DiscTotal);
                push_number_and_total(&mut tags, md, key, "totaldiscs", number, total);
            }
            // Single-valued fields: only the first value is meaningful.
            _ if is_single_valued(key) => {
                if let Some(value) = values.first() {
                    push_value(&mut tags, key, value, container);
                }
            }
            _ => {
                for value in values {
                    push_value(&mut tags, key, value, container);
                }
            }
        }
    }

    tags
}

/// The tag that stores the `~rating` of `md` (ID3 `POPM`, Vorbis `RATING`), when the format has a
/// rating field and `md` holds a rating.
pub(super) fn rating_tag(md: &Metadata, container: Container) -> Option<Tag> {
    let ppm = md.get("~rating").and_then(rating_ppm)?;
    let key = match container {
        Container::MpegAudio
        | Container::Tta
        | Container::Wav
        | Container::Aiff
        | Container::Dsf => "POPM",
        Container::Flac
        | Container::OggVorbis
        | Container::OggOpus
        | Container::OggFlac
        | Container::OggSpeex => "RATING",
        _ => return None,
    };
    Some(Tag::new_std(
        RawTag::new(key, (ppm / 10_000).to_string()),
        StandardTag::Rating(ppm),
    ))
}

fn is_single_valued(key: &str) -> bool {
    matches!(key, "date" | "compilation") || num_ctor(key).is_some() || year_ctor(key).is_some()
}

fn push_value(tags: &mut Vec<Tag>, key: &str, value: &str, container: Container) {
    if let Some(description) = key.strip_prefix("comment:") {
        tags.push(described_comment(description, value, container));
        return;
    }

    match std_from_name(key, value, container) {
        Some((raw_key, std)) => tags.push(Tag::new_from_parts(raw_key, value, Some(std))),
        None => tags.push(free_form(key, value, container)),
    }
}

/// `tracknumber`/`discnumber` may be `n` or `n/total`; the total is taken from the `n/total` form
/// only when the dedicated total key is absent.
fn push_number_and_total(
    tags: &mut Vec<Tag>,
    md: &Metadata,
    key: &str,
    total_key: &str,
    number: fn(u64) -> StandardTag,
    total: fn(u64) -> StandardTag,
) {
    let Some(value) = md.get(key) else { return };

    match split_pair(value) {
        Some((n, t)) => {
            tags.push(Tag::new_from_parts(key, n.to_string(), Some(number(n))));
            if let (Some(t), false) = (t, md.contains(total_key)) {
                tags.push(Tag::new_from_parts(
                    total_key,
                    t.to_string(),
                    Some(total(t)),
                ));
            }
        }
        // Not a number (`A1`, ...): keep it verbatim as a free-form field.
        None => tags.push(Tag::new(RawTag::new(key, value))),
    }
}

/// Parses `n` or `n/total`.
fn split_pair(value: &str) -> Option<(u64, Option<u64>)> {
    match value.split_once('/') {
        Some((n, t)) => {
            let n = n.trim().parse().ok()?;
            let t = t.trim();
            Some((
                n,
                if t.is_empty() {
                    None
                } else {
                    Some(t.parse().ok()?)
                },
            ))
        }
        None => Some((value.trim().parse().ok()?, None)),
    }
}

/// The `StandardTag` for Picard tag `name` in `container`, plus the raw key to carry. `None` means
/// the tag has no standard form and is written free-form.
fn std_from_name(name: &str, value: &str, container: Container) -> Option<(String, StandardTag)> {
    let text = || Arc::new(value.to_owned());

    match name {
        "date" => {
            let std = if date_is_release_date(container) {
                StandardTag::ReleaseDate(text())
            } else {
                StandardTag::RecordingDate(text())
            };
            return Some((name.to_owned(), std));
        }
        // Only MP4 has a native field (the iTunes free-form `ARTISTS`) that the reader maps back
        // to a standard tag; everywhere else it is a plain free-form field.
        "artists" => {
            return (container == Container::Mp4)
                .then(|| (MP4_ARTISTS_KEY.to_owned(), StandardTag::Artist(text())));
        }
        "compilation" => {
            let flag = match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" => true,
                "0" | "false" | "no" => false,
                _ => return None,
            };
            return Some((name.to_owned(), StandardTag::CompilationFlag(flag)));
        }
        _ => {}
    }

    if let Some(ctor) = text_ctor(name) {
        return Some((name.to_owned(), ctor(text())));
    }
    if let Some(ctor) = num_ctor(name) {
        return Some((name.to_owned(), ctor(value.trim().parse().ok()?)));
    }
    if let Some(ctor) = year_ctor(name) {
        return Some((name.to_owned(), ctor(value.trim().parse().ok()?)));
    }

    None
}

/// Names iTunes writes with mixed case, which Metadata's lowercase keys cannot carry.
const ITUNES_NAMES: &[&str] = &[
    "iTunNORM",
    "iTunSMPB",
    "iTunPGAP",
    "iTunEXTC",
    "iTunMOVI",
    "iTunes_CDDB_IDs",
    "iTunes_CDDB_1",
    "iTunes_CDDB_TrackNumber",
];

/// Restores iTunes' spelling of a lowercased name (`itunnorm` -> `iTunNORM`).
fn canonical_case(name: &str) -> &str {
    ITUNES_NAMES
        .iter()
        .find(|n| n.eq_ignore_ascii_case(name))
        .copied()
        .unwrap_or(name)
}

/// The description of a `COMM` frame, if it has one.
fn comm_description(tag: &Tag) -> Option<&str> {
    if !matches!(tag.raw.key.as_str(), "COMM" | "COM") {
        return None;
    }
    tag.raw
        .sub_fields
        .as_deref()?
        .iter()
        .find_map(|f| match (&f.value, f.field.as_str()) {
            (RawValue::String(s), "SHORT_DESCRIPTION") if !s.is_empty() => Some(s.as_str()),
            _ => None,
        })
}

/// Picard keeps a comment with description `D` as `comment:D`. ID3 has a native field for it
/// (`COMM` with a description); elsewhere it is a free-form field named `comment:D`.
fn described_comment(description: &str, value: &str, container: Container) -> Tag {
    let description = canonical_case(description);
    if container == Container::MpegAudio {
        let raw = RawTag::new_with_sub_fields(
            "COMM",
            value,
            vec![RawTagSubField::new("SHORT_DESCRIPTION", description)].into_boxed_slice(),
        );
        Tag::new_std(raw, StandardTag::Comment(Arc::new(value.to_owned())))
    } else {
        Tag::new(RawTag::new(format!("comment:{description}"), value))
    }
}

fn free_form(key: &str, value: &str, container: Container) -> Tag {
    // Picard names this one in upper case wherever it is a free-form field.
    let raw_key = if key == "artists" && container != Container::Mp4 {
        "ARTISTS"
    } else {
        canonical_case(key)
    };
    Tag::new(RawTag::new(raw_key, value))
}

// ---------------------------------------------------------------------------------------------
// Pictures.
// ---------------------------------------------------------------------------------------------

pub(super) fn to_visual(image: &CoverImage) -> Visual {
    // Picard's `__ID3_IMAGE_TYPE_MAP`: track and medium are both "Media"; the types ID3's
    // picture types have no entry for (obi, tray, spine, sticker, ...) are "Other".
    let usage = match image.kind {
        ImageType::Front => StandardVisualKey::FrontCover,
        ImageType::Back => StandardVisualKey::BackCover,
        ImageType::Booklet => StandardVisualKey::Leaflet,
        ImageType::Medium | ImageType::Track => StandardVisualKey::Media,
        _ => StandardVisualKey::Other,
    };

    let tags = if image.description.is_empty() {
        Vec::new()
    } else {
        let description = Arc::new(image.description.clone());
        vec![Tag::new_from_parts(
            "DESCRIPTION",
            description.clone(),
            Some(StandardTag::Description(description)),
        )]
    };

    Visual {
        media_type: (!image.mime.is_empty()).then(|| image.mime.clone()),
        dimensions: None,
        color_mode: None,
        usage: Some(usage),
        tags,
        data: image.data.to_vec().into_boxed_slice(),
    }
}

pub(super) fn from_visual(visual: &Visual) -> CoverImage {
    let kind = match visual.usage {
        // Containers without picture types (MP4 `covr`, Vorbis `COVERART`) only hold covers.
        None | Some(StandardVisualKey::FrontCover) => ImageType::Front,
        Some(StandardVisualKey::BackCover) => ImageType::Back,
        Some(StandardVisualKey::Leaflet) => ImageType::Booklet,
        Some(StandardVisualKey::Media) => ImageType::Medium,
        Some(_) => ImageType::Other,
    };

    let description = visual
        .tags
        .iter()
        .find_map(|t| match &t.std {
            Some(StandardTag::Description(d)) => Some(d.to_string()),
            _ => None,
        })
        .unwrap_or_default();

    let mime = visual
        .media_type
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| sniff_mime(&visual.data).to_owned());

    let mut image = CoverImage::new(visual.data.clone(), mime, kind, description);
    // A picture type in the file means the container reports types (ID3, FLAC, ASF, ...).
    if visual.usage.is_some() {
        image.types = vec![kind];
    }
    image
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(s: &str) -> Arc<String> {
        Arc::new(s.to_owned())
    }

    fn read(std: StandardTag, raw_key: &str, source: Source) -> Vec<Item> {
        let tag = Tag::new_from_parts(raw_key, "x", Some(std));
        let mut out = Vec::new();
        read_tag(&tag, source, &mut out);
        out
    }

    /// Every table entry must survive name -> StandardTag -> name, or a tag would silently
    /// change key (or be dropped) on a load/save round trip.
    #[test]
    fn name_tables_are_bijective() {
        for name in TEXT_NAMES {
            // `date` and `artists` are container specific and covered below.
            let std = text_ctor(name).expect("ctor")(arc("v"));
            let (back, v) = text_name(&std).expect("name");
            assert_eq!(back, *name);
            assert_eq!(v.as_str(), "v");
        }
        for name in NUM_NAMES {
            let std = num_ctor(name).expect("ctor")(7);
            assert_eq!(num_name(&std), Some((*name, 7)));
        }
        for name in YEAR_NAMES {
            let std = year_ctor(name).expect("ctor")(1999);
            assert_eq!(year_name(&std), Some((*name, 1999)));
        }
    }

    #[test]
    fn text_names_are_unique() {
        let mut names: Vec<&str> = TEXT_NAMES
            .iter()
            .chain(NUM_NAMES)
            .chain(YEAR_NAMES)
            .copied()
            .collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    /// Picard's recording id is the on-disk "Track Id"; its track id is the "Release Track Id".
    #[test]
    fn recording_and_track_ids_are_not_swapped() {
        let key = |s: StandardTag| read(s, "", Source::Other)[0].key.clone();
        // Vorbis/APE/MP4 readers report the on-disk Track Id as MusicBrainzTrackId.
        assert_eq!(
            key(StandardTag::MusicBrainzTrackId(arc("x"))),
            "musicbrainz_recordingid"
        );
        // ID3 `UFID` is reported as MusicBrainzRecordingId.
        assert_eq!(
            key(StandardTag::MusicBrainzRecordingId(arc("x"))),
            "musicbrainz_recordingid"
        );
        assert_eq!(
            key(StandardTag::MusicBrainzReleaseTrackId(arc("x"))),
            "musicbrainz_trackid"
        );

        let mut md = Metadata::new();
        md.set("musicbrainz_recordingid", "rec");
        md.set("musicbrainz_trackid", "trk");
        let tags = to_tags(&md, Container::Flac);
        let std_of = |raw: &str| {
            tags.iter()
                .find(|t| t.raw.key == raw)
                .and_then(|t| t.std.clone())
                .expect("tag")
        };
        assert_eq!(
            std_of("musicbrainz_recordingid"),
            StandardTag::MusicBrainzRecordingId(arc("rec"))
        );
        assert_eq!(
            std_of("musicbrainz_trackid"),
            StandardTag::MusicBrainzReleaseTrackId(arc("trk"))
        );
    }

    #[test]
    fn date_goes_to_the_release_date_field_of_mp4_only() {
        let mut md = Metadata::new();
        md.set("date", "2001-02-03");
        let std = |c| to_tags(&md, c)[0].std.clone().expect("std");
        assert_eq!(
            std(Container::Mp4),
            StandardTag::ReleaseDate(arc("2001-02-03"))
        );
        assert_eq!(
            std(Container::Flac),
            StandardTag::RecordingDate(arc("2001-02-03"))
        );
        assert_eq!(
            std(Container::MpegAudio),
            StandardTag::RecordingDate(arc("2001-02-03"))
        );

        // And back: MP4's ReleaseDate is `date`, but in ID3 it is a different (TXXX) field.
        assert_eq!(
            read(StandardTag::ReleaseDate(arc("x")), "", Source::Mp4)[0].key,
            "date"
        );
        assert_eq!(
            read(StandardTag::ReleaseDate(arc("x")), "", Source::Id3v2)[0].key,
            "releasedate"
        );
    }

    #[test]
    fn recording_year_only_fills_in_a_missing_date() {
        let items = read(StandardTag::RecordingYear(1999), "TYER", Source::Id3v2);
        assert_eq!(
            items,
            vec![Item {
                key: "date".into(),
                value: "1999".into(),
                weak: true
            }]
        );
    }

    #[test]
    fn track_number_and_total_forms() {
        let std_values = |md: &Metadata| -> Vec<(String, String)> {
            to_tags(md, Container::Flac)
                .into_iter()
                .map(|t| (t.raw.key.clone(), t.raw.value.to_string()))
                .collect()
        };

        let mut md = Metadata::new();
        md.set("tracknumber", "03/12");
        assert_eq!(
            std_values(&md),
            vec![
                ("tracknumber".into(), "3".into()),
                ("totaltracks".into(), "12".into())
            ]
        );

        // An explicit total wins over the one embedded in `tracknumber`.
        md.set("totaltracks", "14");
        let tags = to_tags(&md, Container::Flac);
        let totals: Vec<_> = tags
            .iter()
            .filter_map(|t| match &t.std {
                Some(StandardTag::TrackTotal(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(totals, vec![14]);

        // A non-numeric track number is kept verbatim instead of being dropped.
        let mut md = Metadata::new();
        md.set("tracknumber", "A1");
        let tags = to_tags(&md, Container::Flac);
        assert_eq!(tags.len(), 1);
        assert!(tags[0].std.is_none());
        assert_eq!(tags[0].raw.value.to_string(), "A1");
    }

    #[test]
    fn mp4_artists_is_the_itunes_free_form_atom() {
        let mut md = Metadata::new();
        md.set_all("artists", ["A".to_string(), "B".to_string()]);

        let mp4 = to_tags(&md, Container::Mp4);
        assert_eq!(mp4.len(), 2);
        assert!(
            mp4.iter()
                .all(|t| t.raw.key == MP4_ARTISTS_KEY && t.std.is_some())
        );

        let flac = to_tags(&md, Container::Flac);
        assert!(
            flac.iter()
                .all(|t| t.raw.key == "ARTISTS" && t.std.is_none())
        );

        // The MP4 reader reports ARTISTS as `Artist`; only its raw key tells it from `©ART`.
        let from_freeform = read(
            StandardTag::Artist(arc("A")),
            "com.apple.iTunes:ARTISTS",
            Source::Mp4,
        );
        assert_eq!(from_freeform[0].key, "artists");
        let from_atom = read(StandardTag::Artist(arc("A")), "\u{a9}ART", Source::Mp4);
        assert_eq!(from_atom[0].key, "artist");
    }

    #[test]
    fn raw_tags_keep_their_name_but_noise_is_dropped() {
        let raw = |key: &str, value: &str, source| {
            let mut out = Vec::new();
            read_tag(&Tag::new(RawTag::new(key, value)), source, &mut out);
            out
        };

        // Vorbis/APE: lowercased raw key.
        assert_eq!(
            raw("DISCOGS_ALBUMID", "123", Source::Other)[0].key,
            "discogs_albumid"
        );
        // MP4 free-form: namespace stripped.
        assert_eq!(
            raw("com.apple.iTunes:DISCOGS_ALBUMID", "1", Source::Mp4)[0].key,
            "discogs_albumid"
        );
        // iTunes' gapless data is kept verbatim (leading blank included); ID3 frames the tag map
        // does not carry are not.
        let smpb = raw("com.apple.iTunes:iTunSMPB", " 0 1 2", Source::Mp4);
        assert_eq!(smpb, vec![strong("itunsmpb", " 0 1 2")]);
        assert!(raw("TLEN", "1000", Source::Id3v2).is_empty());

        // ID3 TXXX names itself in a sub-field.
        let txxx = Tag::new(RawTag::new_with_sub_fields(
            "TXXX",
            "99",
            vec![symphonia_core::meta::RawTagSubField::new(
                "DESCRIPTION",
                "DISCOGS_ALBUMID",
            )]
            .into_boxed_slice(),
        ));
        let mut out = Vec::new();
        read_tag(&txxx, Source::Id3v2, &mut out);
        assert_eq!(out, vec![strong("discogs_albumid", "99")]);
    }

    #[test]
    fn picture_types_round_trip_where_id3_has_them() {
        for kind in [
            ImageType::Front,
            ImageType::Back,
            ImageType::Booklet,
            ImageType::Medium,
            ImageType::Other,
        ] {
            let image = CoverImage::new(&b"\xff\xd8\xff\xe0"[..], "image/jpeg", kind, "d");
            assert_eq!(from_visual(&to_visual(&image)), image);
        }
    }
}
