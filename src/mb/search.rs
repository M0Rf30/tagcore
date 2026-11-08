//! Interactive MusicBrainz searches behind Picard's search dialogs (`ui/searchdialog/{album,track,
//! artist}.py`, `webservice/api_helpers/musicbrainz.py::_find`).
//!
//! Picard sends the text of the dialog's query box in one of two ways: with *advanced query
//! syntax* it is a raw Lucene query; otherwise it is escaped, trimmed and lower-cased and sent
//! with `dismax=true`, so every word is matched across the entity's default fields. Pages are
//! requested with `limit`/`offset`; [`SearchPage::next_offset`] says where the next page starts.
//!
//! The typed hits carry what the dialogs' columns show. Release and recording hits also keep
//! the [`ReleaseCandidate`] / [`RecordingCandidate`] the matching code scores.

use std::time::Duration;

use serde_json::Value;

use super::{RecordingCandidate, array, escape_lucene, get, map, page_size, str_field};
use crate::Result;
use crate::http;
use crate::metadata::Metadata;
use crate::release::{ReleaseCandidate, ReleaseId};

/// Results per page (Picard's `query_limit` default).
pub const DEFAULT_LIMIT: u32 = 25;

/// Shown as the release of a recording that is on no release (Picard's "Standalone Recording").
pub const STANDALONE_RECORDING: &str = "Standalone Recording";

/// Longest country list shown in full; longer ones keep the ends (Picard `countries_shortlist`).
const COUNTRY_SHORTLIST: usize = 6;

/// One page of search results.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage<T> {
    /// Hits the server found for the whole query.
    pub total: usize,
    /// Position of this page's first hit in the full result list.
    pub offset: usize,
    /// Hits on this page. Recording hits become one row per release, so this can be less than
    /// `items.len()`.
    pub received: usize,
    pub items: Vec<T>,
}

impl<T> SearchPage<T> {
    fn empty(offset: usize) -> Self {
        Self {
            total: 0,
            offset,
            received: 0,
            items: Vec::new(),
        }
    }

    /// Offset of the next page, or `None` when this was the last one.
    pub fn next_offset(&self) -> Option<usize> {
        let next = self.offset + self.received;
        (self.received > 0 && next < self.total).then_some(next)
    }
}

/// A release search hit with the columns of the album search dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseHit {
    pub id: ReleaseId,
    pub release_group_id: Option<String>,
    pub title: String,
    /// Release disambiguation.
    pub comment: String,
    pub artist: String,
    /// "2×CD + DVD-Video" style summary of the media.
    pub format: String,
    pub tracks: Option<u32>,
    pub date: String,
    /// Release countries, shortened like Picard's `countries_shortlist`.
    pub country: String,
    pub labels: Vec<String>,
    pub catalog_numbers: Vec<String>,
    pub barcode: String,
    /// ISO 639-3 language code of the release text.
    pub language: String,
    /// Primary and secondary release types, e.g. `album + live`.
    pub release_type: String,
    pub status: String,
    /// Search relevance, 0..=100.
    pub score: u8,
    pub candidate: ReleaseCandidate,
}

impl ReleaseHit {
    /// Columns from a candidate's album-level tags (works for any provider).
    pub fn from_candidate(candidate: ReleaseCandidate) -> Self {
        let m = &candidate.metadata;
        let countries = m.get_all("~releasecountries");
        let country = if countries.is_empty() {
            m.get("releasecountry").unwrap_or_default().to_owned()
        } else {
            countries_shortlist(countries)
        };
        Self {
            id: candidate.id.clone(),
            release_group_id: m.get("musicbrainz_releasegroupid").map(str::to_owned),
            title: m.get("album").unwrap_or_default().to_owned(),
            comment: m.get("~releasecomment").unwrap_or_default().to_owned(),
            artist: m.get("albumartist").unwrap_or_default().to_owned(),
            format: m.get("media").unwrap_or_default().to_owned(),
            tracks: m.get("totaltracks").and_then(|n| n.parse().ok()),
            date: m.get("date").unwrap_or_default().to_owned(),
            country,
            labels: m.get_all("label").to_vec(),
            catalog_numbers: m.get_all("catalognumber").to_vec(),
            barcode: m.get("barcode").unwrap_or_default().to_owned(),
            language: m.get("~releaselanguage").unwrap_or_default().to_owned(),
            release_type: m.get_all("releasetype").join(" + "),
            status: m.get("releasestatus").unwrap_or_default().to_owned(),
            score: candidate.search_score,
            candidate,
        }
    }
}

/// A recording search hit, one per release the recording appears on, with the columns of the
/// track search dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingHit {
    pub recording_id: String,
    /// Recording disambiguation.
    pub comment: String,
    pub title: String,
    pub artist: String,
    pub length: Option<Duration>,
    /// `None` for a standalone recording.
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    /// Release title, or [`STANDALONE_RECORDING`].
    pub release: String,
    pub date: String,
    pub country: String,
    pub release_type: String,
    /// Search relevance, 0..=100.
    pub score: u8,
    pub candidate: RecordingCandidate,
}

/// An artist search hit with the columns of the artist search dialog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtistHit {
    pub id: String,
    pub name: String,
    pub comment: String,
    /// Person, Group, Orchestra, …
    pub kind: String,
    pub gender: String,
    pub area: String,
    pub begin: String,
    pub begin_area: String,
    /// Only set for artists that have ended.
    pub end: String,
    pub end_area: String,
    /// Search relevance, 0..=100.
    pub score: u8,
}

/// Picard's `build_lucene_query`: `field:(escaped value)` for every non-empty value.
pub fn build_lucene_query(fields: &[(&str, &str)]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}:({})", escape_lucene(v)))
        .collect();
    parts.join(" ")
}

/// Picard's `limited_join(countries, 6, '+', '…')`.
pub fn countries_shortlist(countries: &[String]) -> String {
    if countries.len() <= COUNTRY_SHORTLIST {
        return countries.join("+");
    }
    let half = COUNTRY_SHORTLIST / 2;
    let mut parts: Vec<&str> = countries[..half].iter().map(String::as_str).collect();
    parts.push("…");
    parts.extend(
        countries[countries.len() - half..]
            .iter()
            .map(String::as_str),
    );
    parts.join("+")
}

/// The `query` (and whether `dismax` applies) Picard sends for `text`; `None` for blank text.
fn query_args(text: &str, advanced: bool) -> Option<(String, bool)> {
    let query = if advanced {
        text.to_owned()
    } else {
        escape_lucene(text).trim().to_lowercase()
    };
    (!query.trim().is_empty()).then_some((query, !advanced))
}

/// `GET {ws}/{entity}?query=…&limit=…&offset=…[&dismax=true]&fmt=json`; `None` for blank text.
/// `entity` is a search endpoint: `release`, `recording` or `artist`.
pub fn search_url(
    entity: &str,
    text: &str,
    advanced: bool,
    limit: u32,
    offset: usize,
) -> Option<String> {
    let (query, dismax) = query_args(text, advanced)?;
    let mut url = format!(
        "{}/{entity}?query={}&limit={}&offset={offset}",
        super::ws_base(),
        http::encode_query(&query),
        page_size(limit)
    );
    if dismax {
        url.push_str("&dismax=true");
    }
    url.push_str("&fmt=json");
    Some(url)
}

fn fetch(
    entity: &str,
    text: &str,
    advanced: bool,
    limit: u32,
    offset: usize,
) -> Result<Option<Value>> {
    search_url(entity, text, advanced, limit, offset)
        .map(|url| get(&url, None))
        .transpose()
}

/// Release search (Picard `find_releases`). Blank text yields an empty page without a request.
pub fn search_releases_query(
    text: &str,
    advanced: bool,
    limit: u32,
    offset: usize,
) -> Result<SearchPage<ReleaseHit>> {
    Ok(
        fetch("release", text, advanced, limit, offset)?.map_or_else(
            || SearchPage::empty(offset),
            |json| parse_releases(&json, offset),
        ),
    )
}

/// Recording search (Picard `find_tracks`), one hit per (recording, release) pair.
pub fn search_recordings_query(
    text: &str,
    advanced: bool,
    limit: u32,
    offset: usize,
) -> Result<SearchPage<RecordingHit>> {
    Ok(
        fetch("recording", text, advanced, limit, offset)?.map_or_else(
            || SearchPage::empty(offset),
            |json| parse_recordings(&json, offset),
        ),
    )
}

/// Artist search (Picard `find_artists`).
pub fn search_artists_query(
    text: &str,
    advanced: bool,
    limit: u32,
    offset: usize,
) -> Result<SearchPage<ArtistHit>> {
    Ok(fetch("artist", text, advanced, limit, offset)?.map_or_else(
        || SearchPage::empty(offset),
        |json| parse_artists(&json, offset),
    ))
}

/// Total and offset of a search response; `offset` is the requested one when the response omits it.
fn page_of<T>(json: &Value, offset: usize, received: usize, items: Vec<T>) -> SearchPage<T> {
    let number = |key: &str| json[key].as_u64().and_then(|n| usize::try_from(n).ok());
    SearchPage {
        total: number("count").unwrap_or(offset + received),
        offset: number("offset").unwrap_or(offset),
        received,
        items,
    }
}

/// Maps a `/release` search response. Pure; see [`search_releases_query`].
pub fn parse_releases(json: &Value, offset: usize) -> SearchPage<ReleaseHit> {
    let hits = array(json, "releases");
    let items = hits
        .iter()
        .filter_map(map::release_candidate)
        .map(ReleaseHit::from_candidate)
        .collect();
    page_of(json, offset, hits.len(), items)
}

/// Earliest non-empty `date` of a release's `release-events`.
fn first_event_date(release: &Value) -> Option<&str> {
    array(release, "release-events")
        .iter()
        .filter_map(|e| str_field(e, "date"))
        .min()
}

/// Maps a `/recording` search response. Pure; see [`search_recordings_query`].
pub fn parse_recordings(json: &Value, offset: usize) -> SearchPage<RecordingHit> {
    let hits = array(json, "recordings");
    let mut items = Vec::new();
    for hit in hits {
        let releases = array(hit, "releases");
        for (index, candidate) in map::recording_candidates(hit).into_iter().enumerate() {
            let release_node = releases.get(index);
            let m = &candidate.metadata;
            let date = m
                .get("date")
                .or_else(|| release_node.and_then(first_event_date))
                .unwrap_or_default()
                .to_owned();
            let countries = m.get_all("~releasecountries");
            let country = if countries.is_empty() {
                m.get("releasecountry").unwrap_or_default().to_owned()
            } else {
                countries_shortlist(countries)
            };
            let release = match candidate.release_id {
                Some(_) => m.get("album").unwrap_or_default().to_owned(),
                None => STANDALONE_RECORDING.to_owned(),
            };
            items.push(RecordingHit {
                recording_id: candidate.recording_id.clone(),
                comment: m.get("~recordingcomment").unwrap_or_default().to_owned(),
                title: m.get("title").unwrap_or_default().to_owned(),
                artist: m.get("artist").unwrap_or_default().to_owned(),
                length: m.length(),
                release_id: candidate.release_id.clone(),
                release_group_id: m.get("musicbrainz_releasegroupid").map(str::to_owned),
                release,
                date,
                country,
                release_type: m.get_all("releasetype").join(" + "),
                score: candidate.search_score,
                candidate,
            });
        }
    }
    page_of(json, offset, hits.len(), items)
}

/// Name of a nested area object (`area`, `begin-area`, `end-area`).
fn area_name(node: &Value, key: &str) -> String {
    str_field(&node[key], "name").unwrap_or_default().to_owned()
}

/// Maps an `/artist` search response. Pure; see [`search_artists_query`].
pub fn parse_artists(json: &Value, offset: usize) -> SearchPage<ArtistHit> {
    let hits = array(json, "artists");
    let items = hits
        .iter()
        .filter_map(|node| {
            let text = |key: &str| str_field(node, key).unwrap_or_default().to_owned();
            let span = &node["life-span"];
            // Like Picard's `artist_to_metadata`: the end date only counts for ended artists.
            let ended = span["ended"].as_bool().unwrap_or(false);
            Some(ArtistHit {
                id: str_field(node, "id")?.to_owned(),
                name: text("name"),
                comment: text("disambiguation"),
                kind: text("type"),
                gender: text("gender"),
                area: area_name(node, "area"),
                begin: str_field(span, "begin").unwrap_or_default().to_owned(),
                begin_area: area_name(node, "begin-area"),
                end: if ended {
                    str_field(span, "end").unwrap_or_default().to_owned()
                } else {
                    String::new()
                },
                end_area: area_name(node, "end-area"),
                score: score_of(node),
            })
        })
        .collect();
    page_of(json, offset, hits.len(), items)
}

/// Search `score` of a hit: an integer in current responses, a string in older ones.
fn score_of(hit: &Value) -> u8 {
    let score = &hit["score"];
    let raw = score
        .as_u64()
        .or_else(|| score.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    u8::try_from(raw.min(100)).unwrap_or(100)
}

/// Seeds `metadata` into a Picard `show_similar_albums` query: `artist`, `release` and `tracks`.
pub fn similar_albums_query(album: &str, artist: &str, tracks: usize, advanced: bool) -> String {
    if advanced {
        build_lucene_query(&[
            ("artist", artist),
            ("release", album),
            ("tracks", &tracks.to_string()),
        ])
    } else {
        album.to_owned()
    }
}

/// Picard's `show_similar_tracks` query for a file's tags: `track`, `artist`, `release`, `tnum`,
/// `tracks`, `qdur` (the length in 2 s buckets) and `isrc`; plain text is the title only.
pub fn similar_tracks_query(file: &Metadata, advanced: bool) -> String {
    let get = |key: &str| file.get(key).unwrap_or_default();
    if !advanced {
        return get("title").to_owned();
    }
    let qdur = file
        .length()
        .map(|l| (l.as_millis() / 2000).to_string())
        .unwrap_or_default();
    build_lucene_query(&[
        ("track", get("title")),
        ("artist", get("artist")),
        ("release", get("album")),
        ("tnum", get("tracknumber")),
        ("tracks", get("totaltracks")),
        ("qdur", &qdur),
        ("isrc", get("isrc")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_SEARCH: &str =
        include_str!("../../tests/fixtures/musicbrainz/release-search.json");
    const RECORDING_SEARCH: &str =
        include_str!("../../tests/fixtures/musicbrainz/recording-search.json");
    const ARTIST_SEARCH: &str = include_str!("../../tests/fixtures/musicbrainz/artist-search.json");

    fn load(text: &str) -> Value {
        serde_json::from_str(text).expect("fixture is valid JSON")
    }

    fn query_of(url: &str) -> String {
        let start = url.find("query=").expect("query parameter") + "query=".len();
        let value = url[start..].split('&').next().expect("value");
        // Decode %XX for readable assertions.
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).expect("hex"));
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn plain_text_is_escaped_lowercased_and_sent_with_dismax() {
        let url =
            search_url("release", "  OK Computer (Deluxe) / AC:DC  ", false, 25, 50).expect("url");
        assert_eq!(query_of(&url), r"ok computer \(deluxe\) \/ ac\:dc");
        assert!(url.contains("/release?query="), "{url}");
        assert!(
            url.ends_with("&limit=25&offset=50&dismax=true&fmt=json"),
            "{url}"
        );
    }

    #[test]
    fn advanced_text_is_passed_through_without_dismax() {
        let lucene = r#"release:"OK Computer" AND artist:Radiohead"#;
        let url = search_url("release", lucene, true, 10, 0).expect("url");
        assert_eq!(query_of(&url), lucene);
        assert!(url.ends_with("&limit=10&offset=0&fmt=json"), "{url}");
        assert!(!url.contains("dismax"));
    }

    #[test]
    fn blank_queries_are_not_sent() {
        assert_eq!(search_url("artist", "   ", false, 25, 0), None);
        assert_eq!(search_url("artist", "", true, 25, 0), None);
        assert_eq!(
            search_releases_query(" ", false, 25, 0).expect("no request needed"),
            SearchPage::empty(0)
        );
        assert_eq!(
            search_recordings_query("", true, 25, 25)
                .expect("no request needed")
                .offset,
            25
        );
        assert!(
            search_artists_query("\t", false, 25, 0)
                .expect("no request needed")
                .items
                .is_empty()
        );
    }

    #[test]
    fn page_size_is_clamped() {
        let url = search_url("release", "x", false, 500, 0).expect("url");
        assert!(url.contains("&limit=100&"), "{url}");
    }

    #[test]
    fn lucene_query_skips_empty_values_and_escapes() {
        assert_eq!(
            build_lucene_query(&[
                ("artist", "AC/DC"),
                ("release", ""),
                ("tracks", "10"),
                ("track", "Who Made Who?")
            ]),
            r"artist:(AC\/DC) tracks:(10) track:(Who Made Who\?)"
        );
        assert_eq!(build_lucene_query(&[("artist", "")]), "");
    }

    #[test]
    fn country_lists_are_shortened_in_the_middle() {
        let list = |n: usize| (0..n).map(|i| format!("C{i}")).collect::<Vec<_>>();
        assert_eq!(countries_shortlist(&[]), "");
        assert_eq!(countries_shortlist(&list(2)), "C0+C1");
        assert_eq!(countries_shortlist(&list(6)), "C0+C1+C2+C3+C4+C5");
        assert_eq!(countries_shortlist(&list(8)), "C0+C1+C2+…+C5+C6+C7");
    }

    #[test]
    fn release_hits_carry_the_dialog_columns() {
        let page = parse_releases(&load(RELEASE_SEARCH), 0);
        assert_eq!(
            (page.total, page.offset, page.received, page.items.len()),
            (2, 0, 2, 2)
        );
        assert_eq!(page.next_offset(), None);

        let ok = &page.items[0];
        assert_eq!(
            ok.id,
            ReleaseId {
                source: crate::release::Source::MusicBrainz,
                id: "6a09041b-0f79-3278-88d9-0552dd28d37f".into()
            }
        );
        assert_eq!(
            ok.release_group_id.as_deref(),
            Some("b1392450-e666-3926-a536-22c65f834433")
        );
        assert_eq!(
            (ok.title.as_str(), ok.artist.as_str()),
            ("OK Computer", "Radiohead")
        );
        assert_eq!((ok.format.as_str(), ok.tracks), ("CD", Some(12)));
        assert_eq!(
            (ok.date.as_str(), ok.country.as_str()),
            ("1997-07-01", "US")
        );
        assert_eq!(ok.labels, ["Capitol Records"]);
        assert_eq!(ok.catalog_numbers, ["CDP 7243 8 55229 2 5"]);
        assert_eq!(ok.barcode, "724385522925");
        assert_eq!(
            (
                ok.language.as_str(),
                ok.status.as_str(),
                ok.release_type.as_str()
            ),
            ("eng", "official", "album")
        );
        assert_eq!(ok.score, 100);
        assert_eq!(ok.comment, "");
        assert_eq!(ok.candidate.metadata.get("album"), Some("OK Computer"));

        let beach = &page.items[1];
        assert_eq!(
            (beach.title.as_str(), beach.comment.as_str()),
            ("Plastic Beach", "deluxe version")
        );
        assert_eq!(beach.tracks, Some(19));
        assert_eq!(beach.country, "", "no release events, no country");
    }

    #[test]
    fn recording_hits_are_one_per_release() {
        let page = parse_recordings(&load(RECORDING_SEARCH), 0);
        assert_eq!((page.total, page.received), (3, 3));
        assert_eq!(
            page.items.len(),
            4,
            "one hit on two releases, one on one release, one on none"
        );
        assert_eq!(page.next_offset(), None);

        let live = &page.items[0];
        assert_eq!(live.recording_id, "d1400ebf-fcdb-43fa-947b-073e6ec190df");
        assert_eq!(
            live.release_id.as_deref(),
            Some("7f904fa5-1859-42bf-83be-256c3bf99c2c")
        );
        assert_eq!(live.title, "Paranoid Android");
        assert_eq!(
            (live.release.as_str(), live.artist.as_str()),
            ("The Mix 193", "Radiohead"),
            "the recording's artist, not the release's"
        );
        assert_eq!(
            (live.date.as_str(), live.country.as_str()),
            ("1997-12", "GB")
        );
        assert_eq!(live.length, Some(Duration::from_millis(384_000)));
        assert_eq!(
            live.comment,
            "live, 1997-06-28: Glastonbury Festival, Pilton, UK"
        );
        assert_eq!(live.score, 100);
        let same_recording = &page.items[1];
        assert_eq!(same_recording.recording_id, live.recording_id);
        assert_eq!(
            same_recording.release,
            "1997-06-28: Glastonbury Festival, Pilton, UK"
        );
        assert_eq!(
            page.items[2].release_id.as_deref(),
            Some("0c1ef0ab-7183-4620-814f-0c9f978200ff")
        );
        assert_eq!(
            (page.items[2].date.as_str(), page.items[2].country.as_str()),
            ("2012", "XE")
        );

        let standalone = &page.items[3];
        assert_eq!(standalone.release_id, None);
        assert_eq!(standalone.release, STANDALONE_RECORDING);
        assert_eq!(standalone.title, "Paranoid Android (demo)");
        assert_eq!(standalone.length, Some(Duration::from_millis(386_373)));
        assert_eq!(
            standalone.comment,
            "live, 2003-06-05: Beacon Theatre, New York, NY, USA"
        );
        assert_eq!(standalone.score, 87);
        assert_eq!(
            (standalone.date.as_str(), standalone.country.as_str()),
            ("", "")
        );
    }

    #[test]
    fn artist_hits_carry_the_dialog_columns() {
        let page = parse_artists(&load(ARTIST_SEARCH), 0);
        assert_eq!((page.total, page.received, page.items.len()), (3, 3, 3));

        let band = &page.items[0];
        assert_eq!(band.id, "a74b1b7f-71a5-4011-9441-d0b5e4122711");
        assert_eq!(
            (band.name.as_str(), band.kind.as_str(), band.gender.as_str()),
            ("Radiohead", "Group", "")
        );
        assert_eq!(
            (
                band.area.as_str(),
                band.begin.as_str(),
                band.begin_area.as_str()
            ),
            ("United Kingdom", "1985", "Abingdon")
        );
        assert_eq!(
            (band.end.as_str(), band.end_area.as_str(), band.score),
            ("", "", 100)
        );

        let person = &page.items[1];
        assert_eq!(
            (
                person.gender.as_str(),
                person.comment.as_str(),
                person.score
            ),
            ("Male", "Radiohead frontman", 92)
        );
        assert_eq!(person.begin, "1968-10-07");

        let ended = &page.items[2];
        assert_eq!(
            (
                ended.end.as_str(),
                ended.end_area.as_str(),
                ended.begin_area.as_str()
            ),
            ("1999-12-31", "Tacoma", "Seattle")
        );
    }

    #[test]
    fn end_dates_only_count_for_ended_artists() {
        let json = serde_json::json!({"artists": [{"id": "x", "name": "A", "score": "81", "life-span": {"begin": "1990", "end": "2000", "ended": false}}]});
        let hit = &parse_artists(&json, 0).items[0];
        assert_eq!(
            (hit.end.as_str(), hit.score),
            ("", 81),
            "string scores are accepted"
        );
    }

    #[test]
    fn pagination_follows_count_and_offset() {
        let json = serde_json::json!({"count": 130, "offset": 50, "artists": [{"id": "a", "name": "A", "score": 1}, {"id": "b", "name": "B", "score": 1}]});
        let page = parse_artists(&json, 50);
        assert_eq!((page.total, page.offset, page.received), (130, 50, 2));
        assert_eq!(page.next_offset(), Some(52));

        let last = parse_artists(
            &serde_json::json!({"count": 2, "offset": 0, "artists": [{"id": "a", "name": "A"}, {"id": "b", "name": "B"}]}),
            0,
        );
        assert_eq!(last.next_offset(), None);
        let empty = parse_artists(
            &serde_json::json!({"count": 9, "offset": 0, "artists": []}),
            0,
        );
        assert_eq!(
            empty.next_offset(),
            None,
            "an empty page ends paging even if the count says more"
        );
        let no_artists = parse_artists(&serde_json::json!({"error": "x"}), 0);
        assert_eq!((no_artists.total, no_artists.items.len()), (0, 0));
    }

    #[test]
    fn recording_pages_advance_by_hits_not_rows() {
        let json = serde_json::json!({"count": 40, "offset": 0, "recordings": [
            {"id": "r1", "score": 100, "title": "T", "releases": [
                {"id": "a", "title": "A"}, {"id": "b", "title": "B"}, {"id": "c", "title": "C"}]}
        ]});
        let page = parse_recordings(&json, 0);
        assert_eq!((page.received, page.items.len()), (1, 3));
        assert_eq!(page.next_offset(), Some(1));
    }

    #[test]
    fn similar_albums_query_follows_picard() {
        assert_eq!(
            similar_albums_query("OK Computer", "Radiohead", 12, true),
            "artist:(Radiohead) release:(OK Computer) tracks:(12)"
        );
        assert_eq!(
            similar_albums_query("OK Computer", "Radiohead", 12, false),
            "OK Computer"
        );
        assert_eq!(
            similar_albums_query("Who's Next?", "", 9, true),
            r"release:(Who's Next\?) tracks:(9)"
        );
    }

    #[test]
    fn similar_tracks_query_follows_picard() {
        let mut file = Metadata::new();
        file.set("title", "Paranoid Android");
        file.set("artist", "Radiohead");
        file.set("album", "OK Computer");
        file.set("tracknumber", "2");
        file.set("totaltracks", "12");
        file.set_length(Duration::from_millis(383_493));
        assert_eq!(
            similar_tracks_query(&file, true),
            "track:(Paranoid Android) artist:(Radiohead) release:(OK Computer) tnum:(2) tracks:(12) qdur:(191)"
        );
        assert_eq!(similar_tracks_query(&file, false), "Paranoid Android");
        assert_eq!(similar_tracks_query(&Metadata::new(), true), "");
    }
}
