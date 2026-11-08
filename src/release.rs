//! Provider-neutral release model shared by MusicBrainz and Discogs.

use std::time::Duration;

use crate::metadata::Metadata;

/// Where a release comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Source {
    MusicBrainz,
    Discogs,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::MusicBrainz => "MusicBrainz",
            Source::Discogs => "Discogs",
        }
    }
}

/// A release identified across providers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleaseId {
    pub source: Source,
    /// MBID or Discogs numeric release id.
    pub id: String,
}

impl ReleaseId {
    /// Web page of the release.
    pub fn url(&self) -> String {
        match self.source {
            Source::MusicBrainz => format!("{}/release/{}", crate::http::server_origin(), self.id),
            Source::Discogs => format!("https://www.discogs.com/release/{}", self.id),
        }
    }

    /// Parses a pasted MusicBrainz/Discogs release URL, a bare MBID, or `discogs:<id>`.
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim().trim_end_matches('/');
        let is_mbid = |t: &str| {
            t.len() == 36
                && t.chars().enumerate().all(|(i, c)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        c == '-'
                    } else {
                        c.is_ascii_hexdigit()
                    }
                })
        };
        if let Some(rest) = s.strip_prefix("discogs:") {
            return rest.parse::<u64>().ok().map(|n| Self {
                source: Source::Discogs,
                id: n.to_string(),
            });
        }
        if s.contains("discogs.com") {
            // .../release/123456-Artist-Title or .../release/123456
            let tail = s.split("/release/").nth(1)?;
            let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
            return (!digits.is_empty()).then_some(Self {
                source: Source::Discogs,
                id: digits,
            });
        }
        let last = s.rsplit('/').next()?;
        let last = last.split(['?', '#']).next()?;
        (is_mbid(last) && (s == last || s.contains("musicbrainz.org/release/"))).then(|| Self {
            source: Source::MusicBrainz,
            id: last.to_ascii_lowercase(),
        })
    }
}

/// One release search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseCandidate {
    pub id: ReleaseId,
    /// Provider relevance, 0..=100 (Discogs: derived from result rank).
    pub search_score: u8,
    /// Album-level Picard tags known from the search result (album, albumartist, date,
    /// releasecountry, totaltracks, media, label, catalognumber, barcode, releasetype, ...).
    pub metadata: Metadata,
}

/// A fully loaded release.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: ReleaseId,
    /// Album-level tags (no per-track keys).
    pub metadata: Metadata,
    /// Tracks in medium order then position (Discogs headings/index tracks flattened, headings
    /// dropped).
    pub tracks: Vec<Track>,
    /// Remote cover art references, resolved by [`crate::coverart`].
    pub artwork: Artwork,
}

/// Where to get cover images for a release.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Artwork {
    #[default]
    None,
    /// MusicBrainz release: the Cover Art Archive (release, then release group) and the cover
    /// art links of its URL relationships.
    CoverArtArchive {
        release_group_id: Option<String>,
        /// The release's `cover-art-archive` summary; `None` when the web service sent none.
        archive: Option<CaaSummary>,
        /// URLs of `cover art link` / `has_cover_art_at` relationships, in relationship order.
        links: Vec<String>,
    },
    /// Discogs image list: `(uri, is_primary)`.
    Discogs(Vec<(String, bool)>),
}

/// The `cover-art-archive` object of a MusicBrainz release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaaSummary {
    pub artwork: bool,
    pub front: bool,
    pub back: bool,
    pub count: u32,
}

/// Relationship types whose URL is a cover image (Picard's `CoverArtProviderUrlRelationships`).
const COVER_ART_LINK_TYPES: [&str; 2] = ["cover art link", "has_cover_art_at"];

impl Artwork {
    /// The artwork of a MusicBrainz release JSON (`cover-art-archive` and, when fetched with
    /// `inc=url-rels`, `relations`).
    pub fn from_musicbrainz(json: &serde_json::Value, release_group_id: Option<String>) -> Self {
        let node = &json["cover-art-archive"];
        let archive = node.is_object().then(|| CaaSummary {
            artwork: node["artwork"].as_bool().unwrap_or(false),
            front: node["front"].as_bool().unwrap_or(false),
            back: node["back"].as_bool().unwrap_or(false),
            count: node["count"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        });
        let links = json["relations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["target-type"].as_str() == Some("url"))
            .filter(|r| {
                r["type"]
                    .as_str()
                    .is_some_and(|t| COVER_ART_LINK_TYPES.contains(&t))
            })
            .filter_map(|r| r["url"]["resource"].as_str())
            .map(str::to_owned)
            .collect();
        Self::CoverArtArchive {
            release_group_id,
            archive,
            links,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// Provider-unique: MusicBrainz release track MBID; Discogs `"{release}:{position}"`.
    pub id: String,
    /// Complete tags for a file matched to this track: album-level tags plus title, artist,
    /// artists, artistsort, tracknumber, totaltracks, discnumber, totaldiscs, discsubtitle, media,
    /// ~length, `~multiartist` = "1" on every track when the release has more than one distinct
    /// track artist credit (Picard's rule), and
    /// provider ids (MusicBrainz: musicbrainz_recordingid, musicbrainz_trackid,
    /// musicbrainz_artistid, isrc; Discogs: discogs_albumid, discogs_artistid, discogs_labelid,
    /// discogs_masterid).
    pub metadata: Metadata,
}

impl Track {
    pub fn length(&self) -> Option<Duration> {
        self.metadata.length()
    }
}

/// Free-text release query; empty fields are omitted.
#[derive(Debug, Clone, Default)]
pub struct ReleaseQuery {
    pub album: String,
    pub artist: String,
    pub tracks: Option<u32>,
}
