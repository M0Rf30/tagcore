//! MusicBrainz WS/2 client (JSON) and release → Picard metadata mapping.
//!
//! All calls go through [`http::mb_get_json`]-style throttled requests: the configured server
//! (`http::ws_base()`) and its rate limit (1 s on the official servers).
//!
//! Layout: this file holds the requests and search queries; `map` maps JSON to tags, with
//! `artist` (names, aliases, translation), `relations`, `folksonomy` (genres), `text` helpers,
//! `versions` (other releases of a release group) and the paged recording-relationship fallback
//! in this file.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use serde_json::Value;

use crate::Result;
use crate::http;
use crate::metadata::Metadata;
use crate::release::{Release, ReleaseCandidate, ReleaseQuery, Track};
use crate::settings::MetadataSettings;

mod artist;
mod folksonomy;
mod map;
mod relations;
pub mod search;
mod text;
mod versions;

pub use artist::VARIOUS_ARTISTS_ID;
pub use text::asciipunct;
pub use versions::{ReleaseVersion, parse_versions};

/// Base URL of the web service of the configured MusicBrainz server.
pub fn ws_base() -> String {
    http::ws_base()
}

/// Largest page the search endpoints allow.
const MAX_LIMIT: u32 = 100;
/// Recordings per page of the relationship fallback (Picard `RECORDING_QUERY_LIMIT`).
const RECORDING_QUERY_LIMIT: u32 = 100;
/// Upper bound on pages fetched when listing a release group's releases (100 per page).
const MAX_VERSION_PAGES: u32 = 10;

/// Non-empty string member `key` of `node`.
fn str_field<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    node.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// Picard's `Metadata.add_unique`.
fn add_unique(m: &mut Metadata, key: &str, value: &str) {
    if !value.is_empty() && !m.get_all(key).iter().any(|v| v == value) {
        m.add(key, value);
    }
}

/// Throttled GET; `auth` is an OAuth bearer token (needed for the user's own tags, genres and
/// ratings).
fn get(url: &str, auth: Option<&str>) -> Result<Value> {
    http::throttle_mb();
    match auth {
        Some(token) => http::get_json(url, &[("Authorization", &format!("Bearer {token}"))]),
        None => http::get_json(url, &[]),
    }
}

/// One recording search hit, flattened per release it appears on.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingCandidate {
    pub recording_id: String,
    /// Release the recording appears on (None for standalone recordings).
    pub release_id: Option<String>,
    pub search_score: u8,
    /// Track-level Picard tags (title, artist, ~length, album, albumartist, tracknumber,
    /// totaltracks, date, releasecountry, releasetype, musicbrainz_recordingid,
    /// musicbrainz_albumid, ...).
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Default)]
pub struct RecordingQuery {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub length: Option<Duration>,
    pub tracknumber: Option<u32>,
}

/// Extra inputs of a release lookup.
#[derive(Debug, Clone, Copy, Default)]
pub struct LookupOptions<'a> {
    /// OAuth bearer token of the logged-in user. Without it the user's own genres, tags and
    /// ratings are not requested.
    pub auth: Option<&'a str>,
    /// Disc IDs the release was looked up with: the matching medium's tracks get
    /// `musicbrainz_discid`.
    pub discids: &'a [String],
}

/// `GET /release?query=release:".." AND artist:".." AND tracks:N`. Empty fields are left out; a
/// query with neither album nor artist yields no results without touching the network.
pub fn search_releases(query: &ReleaseQuery, limit: u32) -> Result<Vec<ReleaseCandidate>> {
    let Some(lucene) = release_lucene(query) else {
        return Ok(Vec::new());
    };
    let url = format!(
        "{}/release?query={}&limit={}&fmt=json",
        ws_base(),
        http::encode_query(&lucene),
        page_size(limit)
    );
    let json = get(&url, None)?;
    Ok(array(&json, "releases")
        .iter()
        .filter_map(map::release_candidate)
        .collect())
}

/// `GET /recording?query=recording:".." AND artist:".." AND release:".."`, flattened to one
/// candidate per (recording, release) pair. When `length` is known the hits are restricted to
/// recordings of about that length (`qdur`, MusicBrainz's 2-second duration buckets, ±2 buckets).
/// `tracknumber` is not part of the query: positions differ between releases of the same recording.
pub fn search_recordings(query: &RecordingQuery, limit: u32) -> Result<Vec<RecordingCandidate>> {
    let Some(lucene) = recording_lucene(query) else {
        return Ok(Vec::new());
    };
    let url = format!(
        "{}/recording?query={}&limit={}&fmt=json",
        ws_base(),
        http::encode_query(&lucene),
        page_size(limit)
    );
    let json = get(&url, None)?;
    Ok(array(&json, "recordings")
        .iter()
        .flat_map(map::recording_candidates)
        .collect())
}

/// The `inc` parameter of a full release lookup, like Picard's `Album.load`: aliases,
/// annotation, disc ids and the relationships and genre/rating data the settings ask for.
/// `authenticated` says whether a token is available for the user-specific parts.
pub fn release_inc(settings: &MetadataSettings, authenticated: bool) -> String {
    let mut inc: BTreeSet<&str> = [
        "aliases",
        "annotation",
        "artist-credits",
        "artists",
        "discids",
        "isrcs",
        "labels",
        "media",
        "recordings",
        "release-groups",
        // The cover art links of the release are URL relationships.
        "url-rels",
    ]
    .into();
    if settings.release_ars || settings.track_ars {
        inc.extend([
            "artist-rels",
            "recording-rels",
            "label-rels",
            "release-group-level-rels",
            "release-rels",
            "series-rels",
            "work-rels",
        ]);
        if settings.track_ars {
            inc.extend(["recording-level-rels", "work-level-rels"]);
        }
    }
    genre_inc(&mut inc, settings, authenticated);
    if settings.enable_ratings && authenticated {
        inc.insert("user-ratings");
    }
    inc.into_iter().collect::<Vec<_>>().join("+")
}

/// Picard's `set_genre_inc_params`: community genres and tags, or the user's own.
fn genre_inc(inc: &mut BTreeSet<&str>, settings: &MetadataSettings, authenticated: bool) {
    if !settings.genres.use_genres {
        return;
    }
    if settings.genres.only_my_genres {
        if authenticated {
            inc.extend(["user-tags", "user-genres"]);
        }
    } else {
        inc.extend(["tags", "genres"]);
    }
}

/// Loads `/release/{mbid}` (see [`release_inc`]) and maps it like Picard. When the settings ask
/// for track relationships and the response left them out (large releases), they are fetched
/// with paged `/recording?release=` browse requests and merged in.
pub fn lookup_release(
    mbid: &str,
    settings: &MetadataSettings,
    auth: Option<&str>,
) -> Result<Release> {
    lookup_release_with(mbid, settings, LookupOptions { auth, discids: &[] })
}

/// [`lookup_release`] with [`LookupOptions`].
pub fn lookup_release_with(
    mbid: &str,
    settings: &MetadataSettings,
    options: LookupOptions<'_>,
) -> Result<Release> {
    let mbid = mbid.trim();
    let url = format!(
        "{}/release/{}?inc={}&fmt=json",
        ws_base(),
        http::encode_query(mbid),
        release_inc(settings, options.auth.is_some())
    );
    let mut json = get(&url, options.auth)?;
    if needs_track_relationships(&json, settings) {
        let recordings = browse_recording_relationships(mbid, options.auth)?;
        merge_recording_relationships(&mut json, &recordings);
    }
    map_release_with(&json, settings, options.discids)
}

/// Maps an already-fetched release JSON (same `inc` as [`lookup_release`]). Pure; unit-testable.
/// Sets `artwork` from the release's `cover-art-archive` summary and URL relationships.
///
/// Follows Picard's `mbjson.py` (`release_to_metadata`, `medium_to_metadata`, `track_to_metadata`,
/// `recording_to_metadata`, `artist_credit_from_node`). Deviations: `tracknumber` is the track's
/// position on its medium and `~length` is in milliseconds. `~multiartist` follows Picard's
/// `Album._load_tracks`: `"1"` on every track when the release has several distinct track artists.
pub fn map_release(json: &Value, settings: &MetadataSettings) -> Result<Release> {
    map_release_with(json, settings, &[])
}

/// [`map_release`] for a release looked up by disc id(s): see [`LookupOptions::discids`].
pub fn map_release_with(
    json: &Value,
    settings: &MetadataSettings,
    discids: &[String],
) -> Result<Release> {
    map::release(json, settings, discids)
}

/// Picard's `Album._needs_track_relationships`: track relationships are wanted, but the first
/// non-empty medium's first recording has no `relations`.
pub fn needs_track_relationships(json: &Value, settings: &MetadataSettings) -> bool {
    if !settings.track_ars {
        return false;
    }
    for medium in array(json, "media") {
        if medium["track-count"].as_u64().unwrap_or(0) > 0 {
            return medium["tracks"]
                .get(0)
                .is_some_and(|track| track["recording"].get("relations").is_none());
        }
    }
    false
}

/// Browses the recordings of a release with their relationships, `RECORDING_QUERY_LIMIT` per page
/// until `recording-count` is reached. Returns the recording nodes by recording id.
pub fn browse_recording_relationships(
    release_id: &str,
    auth: Option<&str>,
) -> Result<HashMap<String, Value>> {
    const INC: &str =
        "artist-rels+label-rels+recording-rels+release-rels+url-rels+work-rels+work-level-rels";
    let mut recordings = HashMap::new();
    let mut offset = 0;
    loop {
        let url = format!(
            "{}/recording?release={}&inc={INC}&limit={RECORDING_QUERY_LIMIT}&offset={offset}&fmt=json",
            ws_base(),
            http::encode_query(release_id)
        );
        let page = get(&url, auth)?;
        for recording in array(&page, "recordings") {
            if let Some(id) = str_field(recording, "id") {
                recordings.insert(id.to_owned(), recording.clone());
            }
        }
        let count = page["recording-count"].as_u64().unwrap_or(0);
        offset += RECORDING_QUERY_LIMIT;
        if u64::from(offset) >= count {
            return Ok(recordings);
        }
    }
}

/// Picard's `_merge_release_recording_relationships`: copies the `relations` of separately
/// browsed recordings into the release's tracks (pregap, tracks and data tracks) that lack them.
pub fn merge_recording_relationships(release: &mut Value, recordings: &HashMap<String, Value>) {
    for medium in release
        .get_mut("media")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(pregap) = medium.get_mut("pregap") {
            merge_into_track(pregap, recordings);
        }
        for key in ["tracks", "data-tracks"] {
            for node in medium
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                merge_into_track(node, recordings);
            }
        }
    }
}

fn merge_into_track(node: &mut Value, recordings: &HashMap<String, Value>) {
    let Some(recording) = node.get_mut("recording").filter(|r| r.is_object()) else {
        return;
    };
    if recording.get("relations").is_some() {
        return;
    }
    let found = recording["id"].as_str().and_then(|id| recordings.get(id));
    if let Some(found) = found {
        recording["relations"] = found
            .get("relations")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
    }
}

/// Loads a recording that is not on any release as a standalone track (Picard's "non-album
/// track"): `album` is `settings.nat_name`, `originaldate` the recording's first release date.
/// Request includes follow Picard's `NonAlbumTrack.load`.
pub fn lookup_recording(
    mbid: &str,
    settings: &MetadataSettings,
    auth: Option<&str>,
) -> Result<Track> {
    let mut inc: BTreeSet<&str> = ["aliases", "artist-credits", "artists"].into();
    if settings.track_ars {
        inc.extend([
            "artist-rels",
            "recording-rels",
            "label-rels",
            "series-rels",
            "url-rels",
            "work-level-rels",
            "work-rels",
        ]);
    }
    genre_inc(&mut inc, settings, auth.is_some());
    if settings.enable_ratings && auth.is_some() {
        inc.insert("user-ratings");
    }
    let url = format!(
        "{}/recording/{}?inc={}&fmt=json",
        ws_base(),
        http::encode_query(mbid.trim()),
        inc.into_iter().collect::<Vec<_>>().join("+")
    );
    map_recording(&get(&url, auth)?, settings)
}

/// Maps a recording lookup JSON to a standalone track. Pure; see [`lookup_recording`].
pub fn map_recording(json: &Value, settings: &MetadataSettings) -> Result<Track> {
    map::standalone_recording(json, settings)
}

/// Lists every release of a release group (`/release?release-group=..&inc=media+labels`, paged by
/// 100), with the display names Picard's "other versions" menu uses.
pub fn release_group_versions(
    release_group_id: &str,
    auth: Option<&str>,
) -> Result<Vec<ReleaseVersion>> {
    let mut pages = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_VERSION_PAGES {
        let url = format!(
            "{}/release?release-group={}&inc=media+labels&limit={MAX_LIMIT}&offset={offset}&fmt=json",
            ws_base(),
            http::encode_query(release_group_id.trim())
        );
        let page = get(&url, auth)?;
        let count = page["release-count"].as_u64().unwrap_or(0);
        offset += MAX_LIMIT;
        pages.push(page);
        if u64::from(offset) >= count {
            break;
        }
    }
    Ok(parse_versions(&pages))
}

fn array<'a>(json: &'a Value, key: &str) -> &'a [Value] {
    json.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn page_size(limit: u32) -> u32 {
    limit.clamp(1, MAX_LIMIT)
}

// ---- Lucene query building ----------------------------------------------------------------

/// Backslash-escapes every character that is special in Lucene query syntax
/// (`+ - & | ! ( ) { } [ ] ^ " ~ * ? : \ /`), the same set Picard escapes.
fn escape_lucene(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '+' | '-'
                | '&'
                | '|'
                | '!'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '^'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `field:"escaped value"`, or `None` for a blank value. Whitespace runs collapse to one space.
fn phrase(field: &str, value: &str) -> Option<String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    (!words.is_empty()).then(|| format!("{field}:\"{}\"", escape_lucene(&words.join(" "))))
}

fn release_lucene(query: &ReleaseQuery) -> Option<String> {
    let album = phrase("release", &query.album);
    let artist = phrase("artist", &query.artist);
    if album.is_none() && artist.is_none() {
        return None;
    }
    let tracks = query
        .tracks
        .filter(|&n| n > 0)
        .map(|n| format!("tracks:{n}"));
    Some(
        [album, artist, tracks]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

fn recording_lucene(query: &RecordingQuery) -> Option<String> {
    let title = phrase("recording", &query.title)?;
    let artist = phrase("artist", &query.artist);
    let album = phrase("release", &query.album);
    let duration = query.length.filter(|l| !l.is_zero()).map(|l| {
        let bucket = l.as_millis() / 2000;
        format!("qdur:[{} TO {}]", bucket.saturating_sub(2), bucket + 2)
    });
    Some(
        [Some(title), artist, album, duration]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lucene_special_characters_are_escaped() {
        assert_eq!(
            escape_lucene(r#"AC/DC: "Back" in (Black) + 1 - [x] {y} ^ ~ * ? ! & | \"#),
            r#"AC\/DC\: \"Back\" in \(Black\) \+ 1 \- \[x\] \{y\} \^ \~ \* \? \! \& \| \\"#
        );
        assert_eq!(
            escape_lucene("Björk – Ágætis"),
            "Björk – Ágætis",
            "non-ASCII letters and plain punctuation are left alone"
        );
        assert_eq!(escape_lucene("it's 100% fine, ok."), "it's 100% fine, ok.");
    }

    #[test]
    fn release_query_joins_present_fields_with_and() {
        let full = ReleaseQuery {
            album: "OK Computer".into(),
            artist: "Radiohead".into(),
            tracks: Some(12),
        };
        assert_eq!(
            release_lucene(&full).as_deref(),
            Some(r#"release:"OK Computer" AND artist:"Radiohead" AND tracks:12"#)
        );

        let tricky = ReleaseQuery {
            album: "  Who's  Next?\t(Deluxe)  ".into(),
            artist: r#"The "Who""#.into(),
            tracks: None,
        };
        assert_eq!(
            release_lucene(&tricky).as_deref(),
            Some(r#"release:"Who's Next\? \(Deluxe\)" AND artist:"The \"Who\"""#)
        );

        let album_only = ReleaseQuery {
            album: "OK Computer".into(),
            ..ReleaseQuery::default()
        };
        assert_eq!(
            release_lucene(&album_only).as_deref(),
            Some(r#"release:"OK Computer""#)
        );
        let artist_tracks = ReleaseQuery {
            artist: "Radiohead".into(),
            tracks: Some(12),
            ..ReleaseQuery::default()
        };
        assert_eq!(
            release_lucene(&artist_tracks).as_deref(),
            Some(r#"artist:"Radiohead" AND tracks:12"#)
        );
    }

    #[test]
    fn release_query_without_album_and_artist_is_not_sent() {
        assert_eq!(
            release_lucene(&ReleaseQuery {
                tracks: Some(12),
                ..ReleaseQuery::default()
            }),
            None
        );
        assert_eq!(
            release_lucene(&ReleaseQuery {
                album: "  ".into(),
                artist: "\n".into(),
                tracks: None
            }),
            None
        );
        assert_eq!(
            search_releases(&ReleaseQuery::default(), 5).expect("no request needed"),
            []
        );
    }

    #[test]
    fn recording_query_needs_a_title_and_buckets_the_length() {
        let q = RecordingQuery {
            title: "Paranoid Android".into(),
            artist: "Radiohead".into(),
            album: "OK Computer".into(),
            length: Some(Duration::from_millis(383_493)),
            tracknumber: Some(2),
        };
        assert_eq!(
            recording_lucene(&q).as_deref(),
            Some(
                r#"recording:"Paranoid Android" AND artist:"Radiohead" AND release:"OK Computer" AND qdur:[189 TO 193]"#
            )
        );
        let bare = RecordingQuery {
            title: "Airbag".into(),
            ..RecordingQuery::default()
        };
        assert_eq!(
            recording_lucene(&bare).as_deref(),
            Some(r#"recording:"Airbag""#)
        );
        let short = RecordingQuery {
            title: "x".into(),
            length: Some(Duration::from_millis(1500)),
            ..RecordingQuery::default()
        };
        assert_eq!(
            recording_lucene(&short).as_deref(),
            Some(r#"recording:"x" AND qdur:[0 TO 2]"#)
        );
        assert_eq!(
            recording_lucene(&RecordingQuery {
                artist: "Radiohead".into(),
                ..RecordingQuery::default()
            }),
            None
        );
        assert_eq!(
            search_recordings(&RecordingQuery::default(), 5).expect("no request needed"),
            []
        );
    }

    const OK_COMPUTER_FULL: &str =
        include_str!("../tests/fixtures/musicbrainz/ok-computer-full.json");
    const RECORDINGS_PAGE: &str =
        include_str!("../tests/fixtures/musicbrainz/recordings-browse-page.json");
    const VERSIONS: &str =
        include_str!("../tests/fixtures/musicbrainz/release-group-versions.json");

    fn incs(settings: &MetadataSettings, authenticated: bool) -> Vec<String> {
        release_inc(settings, authenticated)
            .split('+')
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn release_inc_follows_picards_load() {
        let defaults = incs(&MetadataSettings::default(), false);
        for wanted in [
            "aliases",
            "annotation",
            "artist-credits",
            "artists",
            "discids",
            "isrcs",
            "labels",
            "media",
            "recordings",
            "release-groups",
            "artist-rels",
            "label-rels",
            "recording-rels",
            "release-group-level-rels",
            "release-rels",
            "series-rels",
            "url-rels",
            "work-rels",
        ] {
            assert!(
                defaults.iter().any(|i| i == wanted),
                "missing {wanted}: {defaults:?}"
            );
        }
        for unwanted in [
            "recording-level-rels",
            "work-level-rels",
            "tags",
            "genres",
            "user-tags",
            "user-ratings",
        ] {
            assert!(
                !defaults.iter().any(|i| i == unwanted),
                "unexpected {unwanted}"
            );
        }

        let track_ars = MetadataSettings {
            track_ars: true,
            ..MetadataSettings::default()
        };
        let inc = incs(&track_ars, false);
        assert!(
            inc.contains(&"recording-level-rels".to_owned())
                && inc.contains(&"work-level-rels".to_owned())
        );

        let no_rels = MetadataSettings {
            release_ars: false,
            ..MetadataSettings::default()
        };
        let inc = incs(&no_rels, false);
        assert!(!inc.contains(&"artist-rels".to_owned()));
        assert!(
            inc.contains(&"url-rels".to_owned()),
            "cover art links are always requested"
        );
    }

    #[test]
    fn genre_and_rating_includes_depend_on_login() {
        let mut s = MetadataSettings::default();
        s.genres.use_genres = true;
        let inc = incs(&s, false);
        assert!(inc.contains(&"tags".to_owned()) && inc.contains(&"genres".to_owned()));
        assert!(!inc.iter().any(|i| i.starts_with("user-")));

        s.genres.only_my_genres = true;
        let anonymous = incs(&s, false);
        assert!(
            !anonymous
                .iter()
                .any(|i| i == "tags" || i == "genres" || i.starts_with("user-")),
            "{anonymous:?}"
        );
        let logged_in = incs(&s, true);
        assert!(
            logged_in.contains(&"user-tags".to_owned())
                && logged_in.contains(&"user-genres".to_owned())
        );
        assert!(!logged_in.contains(&"tags".to_owned()));

        let ratings = MetadataSettings {
            enable_ratings: true,
            ..MetadataSettings::default()
        };
        assert!(!incs(&ratings, false).contains(&"user-ratings".to_owned()));
        assert!(incs(&ratings, true).contains(&"user-ratings".to_owned()));
        assert!(ratings.requires_auth());
        assert!(!MetadataSettings::default().requires_auth());
    }

    #[test]
    fn missing_track_relationships_are_detected_and_merged_from_browsed_recordings() {
        let settings = MetadataSettings {
            track_ars: true,
            ..MetadataSettings::default()
        };
        let mut release: Value = serde_json::from_str(OK_COMPUTER_FULL).expect("fixture");
        assert!(
            !needs_track_relationships(&release, &settings),
            "relations present"
        );
        assert!(
            !needs_track_relationships(&release, &MetadataSettings::default()),
            "track_ars off"
        );

        // Large releases come without recording relationships.
        for medium in release["media"].as_array_mut().expect("media") {
            for track in medium["tracks"].as_array_mut().expect("tracks") {
                track["recording"]
                    .as_object_mut()
                    .expect("recording")
                    .remove("relations");
            }
        }
        assert!(needs_track_relationships(&release, &settings));
        let before = map_release(&release, &settings).expect("maps");
        assert!(!before.tracks[0].metadata.contains("engineer"));

        // The first browsed page covers track 1 (recording 4a7fea2e...).
        let page: Value = serde_json::from_str(RECORDINGS_PAGE).expect("fixture");
        assert_eq!(page["recording-count"], 12);
        let recordings: HashMap<String, Value> = array(&page, "recordings")
            .iter()
            .map(|r| (r["id"].as_str().expect("id").to_owned(), r.clone()))
            .collect();
        merge_recording_relationships(&mut release, &recordings);
        let first = &release["media"][0]["tracks"][0]["recording"];
        assert!(
            first["relations"].as_array().is_some_and(|r| !r.is_empty()),
            "merged"
        );
        assert!(
            release["media"][0]["tracks"][2]["recording"]
                .get("relations")
                .is_none(),
            "not on the page: untouched"
        );
        assert!(!needs_track_relationships(&release, &settings));
        let merged = map_release(&release, &settings).expect("maps");
        let expected: Vec<String> = array(first, "relations")
            .iter()
            .filter(|r| r["target-type"] == "artist" && r["type"] == "engineer")
            .filter_map(|r| r["artist"]["name"].as_str().map(str::to_owned))
            .collect();
        assert!(!expected.is_empty());
        let engineers = merged.tracks[0].metadata.get_all("engineer");
        assert!(
            expected.iter().all(|p| engineers.contains(p)),
            "{expected:?} in {engineers:?}"
        );
    }

    #[test]
    fn release_group_versions_of_a_real_response() {
        let page: Value = serde_json::from_str(VERSIONS).expect("fixture");
        let versions = parse_versions(std::slice::from_ref(&page));
        assert_eq!(versions.len(), array(&page, "releases").len());
        let ids: BTreeSet<_> = versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids.len(), versions.len(), "one entry per release");
        let us_cd = versions
            .iter()
            .find(|v| v.id == "6a09041b-0f79-3278-88d9-0552dd28d37f");
        if let Some(v) = us_cd {
            assert!(
                v.name.starts_with("12 / 1997 / US / CD / Capitol Records"),
                "{}",
                v.name
            );
            assert_eq!(v.totaltracks, 12);
        }
        assert!(versions.iter().all(|v| v.name.starts_with(&v.tracks)));
    }
}
