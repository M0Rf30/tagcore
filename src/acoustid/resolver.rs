//! Recording resolver (Picard's `acoustid/recordings.py`).
//!
//! AcoustID sometimes returns recordings as a bare MBID without title or releases. For each result
//! such recordings are fetched from MusicBrainz (at most [`MAX_NO_METADATA_RECORDINGS`] per
//! result, and only when they have at least [`SOURCE_THRESHOLD_NO_METADATA`] of the sources of the
//! best recording), so the matcher has something to compare. Recordings that stay without
//! metadata are dropped.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use super::{LookupResult, Recording, ReleaseRef};
use crate::{Error, Metadata, Result};

/// Only do an extra lookup for recordings without metadata when they have more than this share of
/// the sources of the recording with the most sources.
pub const SOURCE_THRESHOLD_NO_METADATA: f64 = 0.25;

/// Load at most this many recordings without metadata per AcoustID result.
pub const MAX_NO_METADATA_RECORDINGS: usize = 3;

/// `inc` of the recording lookup (Picard: artists, release-groups, releases, media).
const RECORDING_INC: &str = "artist-credits+release-groups+releases+media";

/// What [`resolve`] produced.
#[derive(Debug)]
pub struct Resolved {
    /// The results, every recording with metadata. Results may end up with no recordings.
    pub results: Vec<LookupResult>,
    /// A failed MusicBrainz request; the results are what could be resolved before it.
    pub error: Option<Error>,
}

/// Fills in recordings without metadata through `fetch` (`Ok(None)` = the recording does not
/// exist). Requests are cached by MBID, including the MBID a redirect resolved to.
pub fn resolve(
    mut results: Vec<LookupResult>,
    mut fetch: impl FnMut(&str) -> Result<Option<Recording>>,
) -> Resolved {
    let mut cache: HashMap<String, Recording> = HashMap::new();
    let mut queue: Vec<(usize, Recording)> = Vec::new();
    for (index, result) in results.iter_mut().enumerate() {
        let max_sources = result
            .recordings
            .iter()
            .map(|r| r.sources)
            .max()
            .unwrap_or(1)
            .max(1);
        let mut incomplete = 0;
        let mut kept = Vec::new();
        let mut recordings = std::mem::take(&mut result.recordings);
        // Most sources first so the cap keeps the most trusted ones.
        recordings.sort_by_key(|r| std::cmp::Reverse(r.sources));
        for rec in recordings {
            if rec.has_metadata() {
                cache.insert(rec.id.clone(), rec.clone());
                kept.push(rec);
            } else if f64::from(rec.sources) / f64::from(max_sources) > SOURCE_THRESHOLD_NO_METADATA
                && incomplete < MAX_NO_METADATA_RECORDINGS
            {
                incomplete += 1;
                queue.push((index, rec));
            }
        }
        result.recordings = kept;
    }

    let mut error = None;
    for (index, incomplete) in queue {
        let fetched = match cache.get(&incomplete.id) {
            Some(rec) => Some(rec.clone()),
            None => match fetch(&incomplete.id) {
                Ok(Some(rec)) => {
                    cache.insert(rec.id.clone(), rec.clone());
                    // A redirected MBID: remember the old one too.
                    cache.insert(incomplete.id.clone(), rec.clone());
                    Some(rec)
                }
                Ok(None) => None,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            },
        };
        let Some(mut rec) = fetched else { continue };
        let result = &mut results[index];
        rec.metadata.set("acoustid_id", result.acoustid.clone());
        match result.recordings.iter_mut().find(|r| r.id == rec.id) {
            Some(existing) => existing.sources += incomplete.sources,
            None => {
                rec.sources = incomplete.sources;
                result.recordings.push(rec);
            }
        }
    }
    Resolved { results, error }
}

/// [`resolve`] against the MusicBrainz web service (1 request per second, shared throttle).
pub fn resolve_online(results: Vec<LookupResult>) -> Resolved {
    resolve(results, fetch_recording)
}

/// Loads one recording from MusicBrainz; a 404 yields `None`.
pub fn fetch_recording(mbid: &str) -> Result<Option<Recording>> {
    crate::http::throttle("musicbrainz.org", Duration::from_secs(1));
    let url = format!(
        "{}/recording/{}?inc={RECORDING_INC}&fmt=json",
        crate::mb::ws_base(),
        crate::http::encode_query(mbid.trim())
    );
    match crate::http::get_json(&url, &[]) {
        Ok(json) => Ok(recording_from_mb(&json)),
        Err(e) if crate::http::status_code(&e) == Some(404) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Joins a MusicBrainz `artist-credit` array into the credit string and the artist ids.
fn artist_credit(credit: &Value) -> (String, Vec<String>) {
    let mut name = String::new();
    let mut ids = Vec::new();
    for part in credit.as_array().into_iter().flatten() {
        name.push_str(
            part["name"]
                .as_str()
                .or_else(|| part["artist"]["name"].as_str())
                .unwrap_or_default(),
        );
        name.push_str(part["joinphrase"].as_str().unwrap_or_default());
        if let Some(id) = part["artist"]["id"].as_str() {
            ids.push(id.to_owned());
        }
    }
    (name, ids)
}

/// Maps a MusicBrainz recording lookup (see [`RECORDING_INC`]) to the shape AcoustID results use.
pub fn recording_from_mb(json: &Value) -> Option<Recording> {
    let id = json["id"].as_str()?.to_owned();
    let mut md = Metadata::new();
    md.set("musicbrainz_recordingid", id.clone());
    md.set("title", json["title"].as_str()?);
    let (artist, artist_ids) = artist_credit(&json["artist-credit"]);
    md.set("artist", artist);
    md.set_all("musicbrainz_artistid", artist_ids);
    if let Some(ms) = json["length"].as_u64() {
        md.set_length(Duration::from_millis(ms));
    }
    let releases = json["releases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(release_from_mb)
        .collect();
    Some(Recording {
        id,
        metadata: md,
        sources: 1,
        releases,
    })
}

fn release_from_mb(rel: &Value) -> Option<ReleaseRef> {
    let id = rel["id"].as_str()?.to_owned();
    let mut md = Metadata::new();
    md.set("musicbrainz_albumid", id.clone());
    md.set("album", rel["title"].as_str().unwrap_or_default());
    let (albumartist, ids) = artist_credit(&rel["artist-credit"]);
    md.set("albumartist", albumartist);
    md.set_all("musicbrainz_albumartistid", ids);
    md.set("date", rel["date"].as_str().unwrap_or_default());
    md.set(
        "releasecountry",
        rel["country"].as_str().unwrap_or_default(),
    );
    if let Some(n) = rel["track-count"].as_u64() {
        md.set("totaltracks", n.to_string());
    }
    let group = &rel["release-group"];
    md.set(
        "musicbrainz_releasegroupid",
        group["id"].as_str().unwrap_or_default(),
    );
    let types = group["primary-type"]
        .as_str()
        .into_iter()
        .chain(
            group["secondary-types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    md.set_all("releasetype", types);
    // The recording lookup only lists the media that hold the recording.
    if let Some(medium) = rel["media"].as_array().and_then(|m| m.first()) {
        if let Some(p) = medium["position"].as_u64() {
            md.set("discnumber", p.to_string());
        }
        md.set("media", medium["format"].as_str().unwrap_or_default());
        if let Some(n) = medium["track-count"].as_u64() {
            md.set("totaltracks", n.to_string());
        }
        if let Some(offset) = medium["track-offset"].as_u64() {
            md.set("tracknumber", (offset + 1).to_string());
        }
    }
    Some(ReleaseRef { id, metadata: md })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(id: &str, sources: u32) -> Recording {
        let mut md = Metadata::new();
        md.set("musicbrainz_recordingid", id);
        Recording {
            id: id.into(),
            metadata: md,
            sources,
            releases: Vec::new(),
        }
    }

    fn full(id: &str, title: &str, sources: u32) -> Recording {
        let mut r = bare(id, sources);
        r.metadata.set("title", title);
        r
    }

    fn result(acoustid: &str, recordings: Vec<Recording>) -> LookupResult {
        LookupResult {
            acoustid: acoustid.into(),
            score: 0.9,
            recordings,
        }
    }

    fn mb_fixture() -> Recording {
        let json: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/acoustid/mb-recording.json"
        ))
        .unwrap();
        recording_from_mb(&json).unwrap()
    }

    #[test]
    fn maps_a_musicbrainz_recording() {
        let rec = mb_fixture();
        assert_eq!(rec.id, "5b3f6f4a-1111-4222-8333-444455556666");
        assert_eq!(rec.metadata.get("title"), Some("Resolved Song"));
        assert_eq!(
            rec.metadata.get("artist"),
            Some("Resolved Band feat. Guest")
        );
        assert_eq!(rec.metadata.get_all("musicbrainz_artistid").len(), 2);
        assert_eq!(rec.metadata.length(), Some(Duration::from_millis(215_000)));
        assert_eq!(rec.releases.len(), 2);
        let r = &rec.releases[0].metadata;
        assert_eq!(r.get("album"), Some("Resolved Album"));
        assert_eq!(r.get("albumartist"), Some("Resolved Band"));
        assert_eq!(r.get("date"), Some("2011-05-06"));
        assert_eq!(r.get("releasecountry"), Some("GB"));
        assert_eq!(r.get("discnumber"), Some("2"));
        assert_eq!(r.get("media"), Some("Vinyl"));
        assert_eq!(r.get("tracknumber"), Some("5"));
        assert_eq!(r.get("totaltracks"), Some("12"));
        assert_eq!(r.get_all("releasetype"), ["album", "live"]);
        assert_eq!(
            r.get("musicbrainz_releasegroupid"),
            Some("c1c1c1c1-0000-4000-8000-000000000001")
        );
        assert_eq!(rec.releases[1].metadata.get("tracknumber"), None);
        assert!(recording_from_mb(&serde_json::json!({"id": "x"})).is_none());
    }

    #[test]
    fn complete_results_need_no_requests() {
        let results = vec![result("a1", vec![full("r1", "Song", 10)])];
        let mut calls = 0;
        let out = resolve(results, |_| {
            calls += 1;
            Ok(None)
        });
        assert_eq!(calls, 0);
        assert_eq!(out.results[0].recordings.len(), 1);
        assert!(out.error.is_none());
    }

    #[test]
    fn fetches_only_well_sourced_recordings_up_to_the_cap() {
        // Max sources 100; 25 is not > 25 %, 26 is. Five qualifying bare recordings, cap is 3.
        let recs = vec![
            full("full", "Known", 100),
            bare("low", 25),
            bare("a", 60),
            bare("b", 50),
            bare("c", 40),
            bare("d", 30),
            bare("e", 26),
        ];
        let mut asked = Vec::new();
        let out = resolve(vec![result("a1", recs)], |id| {
            asked.push(id.to_owned());
            let mut rec = mb_fixture();
            rec.id = id.to_owned();
            Ok(Some(rec))
        });
        // Highest sources first; the cap keeps three.
        assert_eq!(asked, ["a", "b", "c"]);
        let ids: Vec<_> = out.results[0]
            .recordings
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids, ["full", "a", "b", "c"]);
        let a = &out.results[0].recordings[1];
        assert_eq!(a.sources, 60);
        assert_eq!(a.metadata.get("acoustid_id"), Some("a1"));
        assert_eq!(a.metadata.get("title"), Some("Resolved Song"));
    }

    #[test]
    fn missing_recordings_are_skipped_and_errors_stop_with_partial_results() {
        let recs = vec![
            full("ok", "Song", 10),
            bare("gone", 10),
            bare("boom", 9),
            bare("never", 8),
        ];
        let mut asked = Vec::new();
        let out = resolve(vec![result("a1", recs)], |id| {
            asked.push(id.to_owned());
            match id {
                "gone" => Ok(None),
                _ => Err(Error::Http("status 503".into())),
            }
        });
        assert_eq!(asked, ["gone", "boom"]);
        assert!(out.error.is_some());
        assert_eq!(out.results[0].recordings.len(), 1);
        assert_eq!(out.results[0].recordings[0].id, "ok");
    }

    #[test]
    fn redirects_are_cached_and_sources_merge() {
        // Two results hold the same bare MBID; the MB answer redirects to another id that the
        // first result already lists with metadata.
        let redirect_target = mb_fixture();
        let target_id = redirect_target.id.clone();
        let results = vec![
            result(
                "a1",
                vec![full(&target_id, "Resolved Song", 10), bare("old", 5)],
            ),
            result("a2", vec![bare("old", 4)]),
        ];
        let mut calls = 0;
        let out = resolve(results, |_| {
            calls += 1;
            Ok(Some(mb_fixture()))
        });
        assert_eq!(
            calls, 1,
            "the second lookup of `old` is served from the cache"
        );
        let first = &out.results[0].recordings;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sources, 15);
        let second = &out.results[1].recordings;
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, target_id);
        assert_eq!(second[0].sources, 4);
    }
}
