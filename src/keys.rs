//! Picard tag names (lowercase) used as [`Metadata`](crate::Metadata) keys.

use crate::Metadata;

pub const TITLE: &str = "title";
pub const ARTIST: &str = "artist";
/// Multi-valued list of the individual track artists.
pub const ARTISTS: &str = "artists";
pub const ALBUM: &str = "album";
pub const ALBUMARTIST: &str = "albumartist";
pub const DATE: &str = "date";
pub const ORIGINALDATE: &str = "originaldate";
pub const ORIGINALYEAR: &str = "originalyear";
pub const GENRE: &str = "genre";
pub const TRACKNUMBER: &str = "tracknumber";
pub const TOTALTRACKS: &str = "totaltracks";
pub const DISCNUMBER: &str = "discnumber";
pub const TOTALDISCS: &str = "totaldiscs";
pub const DISCSUBTITLE: &str = "discsubtitle";
pub const COMPOSER: &str = "composer";
pub const LYRICIST: &str = "lyricist";
pub const CONDUCTOR: &str = "conductor";
pub const ARRANGER: &str = "arranger";
pub const PRODUCER: &str = "producer";
pub const ENGINEER: &str = "engineer";
pub const MIXER: &str = "mixer";
pub const REMIXER: &str = "remixer";
pub const LABEL: &str = "label";
pub const CATALOGNUMBER: &str = "catalognumber";
pub const BARCODE: &str = "barcode";
pub const ISRC: &str = "isrc";
pub const ASIN: &str = "asin";
pub const MEDIA: &str = "media";
pub const RELEASECOUNTRY: &str = "releasecountry";
pub const RELEASESTATUS: &str = "releasestatus";
pub const RELEASETYPE: &str = "releasetype";
pub const SCRIPT: &str = "script";
pub const LANGUAGE: &str = "language";
pub const BPM: &str = "bpm";
pub const KEY: &str = "key";
pub const MOOD: &str = "mood";
pub const GROUPING: &str = "grouping";
/// `"1"` when set; absent otherwise.
pub const COMPILATION: &str = "compilation";
pub const COMMENT: &str = "comment";
pub const LYRICS: &str = "lyrics";
pub const COPYRIGHT: &str = "copyright";
pub const LICENSE: &str = "license";
pub const ENCODEDBY: &str = "encodedby";
pub const ENCODERSETTINGS: &str = "encodersettings";
pub const WEBSITE: &str = "website";
pub const TITLESORT: &str = "titlesort";
pub const ARTISTSORT: &str = "artistsort";
pub const ALBUMSORT: &str = "albumsort";
pub const ALBUMARTISTSORT: &str = "albumartistsort";
pub const COMPOSERSORT: &str = "composersort";
/// MusicBrainz recording MBID (Vorbis `MUSICBRAINZ_TRACKID`, ID3 `UFID:http://musicbrainz.org`).
pub const MUSICBRAINZ_RECORDINGID: &str = "musicbrainz_recordingid";
/// MusicBrainz release-track MBID (Vorbis `MUSICBRAINZ_RELEASETRACKID`).
pub const MUSICBRAINZ_TRACKID: &str = "musicbrainz_trackid";
pub const MUSICBRAINZ_ALBUMID: &str = "musicbrainz_albumid";
pub const MUSICBRAINZ_ARTISTID: &str = "musicbrainz_artistid";
pub const MUSICBRAINZ_ALBUMARTISTID: &str = "musicbrainz_albumartistid";
pub const MUSICBRAINZ_RELEASEGROUPID: &str = "musicbrainz_releasegroupid";
pub const MUSICBRAINZ_WORKID: &str = "musicbrainz_workid";
pub const MUSICBRAINZ_DISCID: &str = "musicbrainz_discid";
pub const MUSICBRAINZ_LABELID: &str = "musicbrainz_labelid";
pub const ACOUSTID_ID: &str = "acoustid_id";
pub const ACOUSTID_FINGERPRINT: &str = "acoustid_fingerprint";
pub const REPLAYGAIN_TRACK_GAIN: &str = "replaygain_track_gain";
pub const REPLAYGAIN_TRACK_PEAK: &str = "replaygain_track_peak";
pub const REPLAYGAIN_ALBUM_GAIN: &str = "replaygain_album_gain";
pub const REPLAYGAIN_ALBUM_PEAK: &str = "replaygain_album_peak";

/// Display order of the well-known fields in the editor: the common block
/// first, then credits, release details, sort names and identifiers.
/// Fields not listed here are shown after these, alphabetically.
pub const KNOWN_KEYS: &[&str] = &[
    TITLE,
    ARTIST,
    ALBUM,
    ALBUMARTIST,
    DATE,
    GENRE,
    TRACKNUMBER,
    TOTALTRACKS,
    DISCNUMBER,
    TOTALDISCS,
    DISCSUBTITLE,
    COMPILATION,
    COMMENT,
    LYRICS,
    ARTISTS,
    COMPOSER,
    LYRICIST,
    CONDUCTOR,
    ARRANGER,
    PRODUCER,
    ENGINEER,
    MIXER,
    REMIXER,
    ORIGINALDATE,
    ORIGINALYEAR,
    LABEL,
    CATALOGNUMBER,
    BARCODE,
    ISRC,
    ASIN,
    MEDIA,
    RELEASECOUNTRY,
    RELEASESTATUS,
    RELEASETYPE,
    SCRIPT,
    LANGUAGE,
    BPM,
    KEY,
    MOOD,
    GROUPING,
    COPYRIGHT,
    LICENSE,
    ENCODEDBY,
    ENCODERSETTINGS,
    WEBSITE,
    TITLESORT,
    ARTISTSORT,
    ALBUMSORT,
    ALBUMARTISTSORT,
    COMPOSERSORT,
    MUSICBRAINZ_RECORDINGID,
    MUSICBRAINZ_TRACKID,
    MUSICBRAINZ_ALBUMID,
    MUSICBRAINZ_ARTISTID,
    MUSICBRAINZ_ALBUMARTISTID,
    MUSICBRAINZ_RELEASEGROUPID,
    MUSICBRAINZ_WORKID,
    MUSICBRAINZ_DISCID,
    MUSICBRAINZ_LABELID,
    ACOUSTID_ID,
    ACOUSTID_FINGERPRINT,
    REPLAYGAIN_TRACK_GAIN,
    REPLAYGAIN_TRACK_PEAK,
    REPLAYGAIN_ALBUM_GAIN,
    REPLAYGAIN_ALBUM_PEAK,
];

/// `(key, values)` of `metadata` in editor display order: [`KNOWN_KEYS`] first, then the
/// remaining keys alphabetically. Hidden (`~`) keys are never listed.
pub fn display_order(metadata: &Metadata) -> Vec<(&str, &[String])> {
    let mut out: Vec<(&str, &[String])> = KNOWN_KEYS
        .iter()
        .filter(|k| metadata.contains(k))
        .map(|k| (*k, metadata.get_all(k)))
        .collect();
    out.extend(
        metadata
            .iter()
            .filter(|(k, _)| !k.starts_with('~') && !KNOWN_KEYS.contains(k)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_order_puts_known_keys_first_then_alphabetical() {
        let mut m = Metadata::new();
        m.set("zzz", "1");
        m.set(ALBUM, "B");
        m.set("aaa", "2");
        m.set(TITLE, "A");
        m.set(crate::metadata::LENGTH, "1000");
        let order: Vec<&str> = display_order(&m).into_iter().map(|(k, _)| k).collect();
        assert_eq!(order, ["title", "album", "aaa", "zzz"]);
    }
}
