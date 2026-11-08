//! AcoustID: fingerprint lookup (`https://api.acoustid.org/v2/lookup`), the recording resolver
//! ([`resolver`]) and fingerprint submission ([`submit`]).
//!
//! Throttled with `http::throttle("api.acoustid.org", 334 ms)`.

use std::time::Duration;

use serde_json::Value;

use crate::fingerprint::Fingerprint;
use crate::metadata::Metadata;
use crate::{Error, Result};

pub mod gate;
pub mod resolver;
pub mod submit;

pub const LOOKUP_URL: &str = "https://api.acoustid.org/v2/lookup";

/// `clientversion` sent with lookups and submissions.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `meta` parameter of a lookup (Picard's `acoustid/__init__.py`).
const LOOKUP_META: &str = "recordings releasegroups releases tracks compress sources";

/// One AcoustID result (an AcoustID track id with its match score).
#[derive(Debug, Clone, PartialEq)]
pub struct LookupResult {
    pub acoustid: String,
    /// 0..1.
    pub score: f32,
    pub recordings: Vec<Recording>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub id: String,
    /// title, artist, ~length, musicbrainz_recordingid, musicbrainz_artistid, acoustid_id.
    pub metadata: Metadata,
    /// Number of AcoustID submissions linking the fingerprint to this recording.
    pub sources: u32,
    pub releases: Vec<ReleaseRef>,
}

impl Recording {
    /// Picard's `recording_has_metadata`: AcoustID returned a title for the recording.
    pub fn has_metadata(&self) -> bool {
        self.metadata.get("title").is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseRef {
    pub id: String,
    /// album, albumartist, date, releasecountry, totaltracks, totaldiscs, releasetype (primary +
    /// secondary types), musicbrainz_albumid, musicbrainz_releasegroupid, plus for the
    /// recording's position: tracknumber, discnumber, media.
    pub metadata: Metadata,
}

/// What a lookup asks AcoustID about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupKey<'a> {
    /// A fingerprint and the track length in whole seconds (floats are not allowed).
    Fingerprint { fingerprint: &'a str, duration: u64 },
    /// A MusicBrainz recording id (Picard's `recordingid` mode).
    Recording(&'a str),
}

/// Form body of a `/lookup` request.
pub fn lookup_form(api_key: &str, key: LookupKey<'_>) -> Vec<(String, String)> {
    let mut form = vec![
        ("client".to_owned(), api_key.to_owned()),
        ("clientversion".to_owned(), CLIENT_VERSION.to_owned()),
    ];
    match key {
        LookupKey::Fingerprint {
            fingerprint,
            duration,
        } => {
            form.push(("duration".into(), duration.to_string()));
            form.push(("fingerprint".into(), fingerprint.to_owned()));
        }
        LookupKey::Recording(id) => form.push(("recordingid".into(), id.to_owned())),
    }
    form.push(("meta".into(), LOOKUP_META.into()));
    form.push(("format".into(), "json".into()));
    form
}

/// POSTs a fingerprint lookup. Results are sorted by descending score; an AcoustID error status
/// maps to `Error::Response`.
pub fn lookup(api_key: &str, fingerprint: &Fingerprint) -> Result<Vec<LookupResult>> {
    lookup_key(
        api_key,
        LookupKey::Fingerprint {
            fingerprint: &fingerprint.fingerprint,
            duration: fingerprint.duration.as_secs(),
        },
    )
}

/// POSTs a lookup of `key` (see [`lookup_form`]).
pub fn lookup_key(api_key: &str, key: LookupKey<'_>) -> Result<Vec<LookupResult>> {
    crate::http::throttle("api.acoustid.org", Duration::from_millis(334));
    let form = lookup_form(api_key, key);
    let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let json = crate::http::post_form_json(LOOKUP_URL, &pairs)?;
    parse_response(&json)
}

fn parse_response(json: &Value) -> Result<Vec<LookupResult>> {
    if json["status"].as_str() == Some("error") {
        let msg = json["error"]["message"]
            .as_str()
            .unwrap_or("unknown AcoustID error");
        return Err(Error::Response(msg.to_string()));
    }
    let results = json["results"]
        .as_array()
        .ok_or_else(|| Error::Response("AcoustID response has no `results`".into()))?;
    let mut out: Vec<LookupResult> = results
        .iter()
        .filter_map(|r| {
            let acoustid = r["id"].as_str()?.to_string();
            let recordings = r["recordings"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|rec| parse_recording(rec, &acoustid))
                        .collect()
                })
                .unwrap_or_default();
            Some(LookupResult {
                acoustid,
                score: r["score"].as_f64().unwrap_or(1.0) as f32,
                recordings,
            })
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// Joins artist names with their join phrases (`"A feat. "` style), returning the credit string
/// and the artist ids.
fn artist_credit(v: &Value) -> (String, Vec<String>) {
    let mut name = String::new();
    let mut ids = Vec::new();
    if let Some(arr) = v.as_array() {
        for a in arr {
            name.push_str(a["name"].as_str().unwrap_or_default());
            name.push_str(a["joinphrase"].as_str().unwrap_or_default());
            if let Some(id) = a["id"].as_str() {
                ids.push(id.to_string());
            }
        }
    }
    (name, ids)
}

fn parse_recording(rec: &Value, acoustid: &str) -> Option<Recording> {
    let id = rec["id"].as_str()?.to_string();
    let mut md = Metadata::new();
    md.set("musicbrainz_recordingid", id.clone());
    md.set("acoustid_id", acoustid);
    if let Some(t) = rec["title"].as_str() {
        md.set("title", t);
    }
    let (artist, artist_ids) = artist_credit(&rec["artists"]);
    md.set("artist", artist);
    md.set_all("musicbrainz_artistid", artist_ids);
    if let Some(secs) = rec["duration"].as_f64() {
        md.set_length(Duration::from_millis((secs * 1000.0).round() as u64));
    }
    let sources = rec["sources"].as_u64().map_or(1, |s| s as u32);

    let mut releases: Vec<ReleaseRef> = Vec::new();
    let mut push = |rel: &Value, group: Option<&Value>| {
        if let Some(r) = parse_release(rel, group)
            && !releases.iter().any(|x| x.id == r.id)
        {
            releases.push(r);
        }
    };
    if let Some(groups) = rec["releasegroups"].as_array() {
        for g in groups {
            for rel in g["releases"].as_array().into_iter().flatten() {
                push(rel, Some(g));
            }
        }
    }
    for rel in rec["releases"].as_array().into_iter().flatten() {
        push(rel, None);
    }
    Some(Recording {
        id,
        metadata: md,
        sources,
        releases,
    })
}

fn parse_release(rel: &Value, group: Option<&Value>) -> Option<ReleaseRef> {
    let id = rel["id"].as_str()?.to_string();
    let mut md = Metadata::new();
    md.set("musicbrainz_albumid", id.clone());
    if let Some(t) = rel["title"].as_str() {
        md.set("album", t);
    }
    let artists = if rel["artists"].is_array() {
        &rel["artists"]
    } else {
        group.map(|g| &g["artists"]).unwrap_or(&Value::Null)
    };
    let (albumartist, albumartist_ids) = artist_credit(artists);
    md.set("albumartist", albumartist);
    md.set_all("musicbrainz_albumartistid", albumartist_ids);
    if let Some(c) = rel["country"].as_str() {
        md.set("releasecountry", c);
    }
    md.set("date", date_string(&rel["date"]));
    if let Some(n) = rel["medium_count"].as_u64() {
        md.set("totaldiscs", n.to_string());
    }
    if let Some(n) = rel["track_count"].as_u64() {
        md.set("totaltracks", n.to_string());
    }
    if let Some(g) = group {
        if let Some(gid) = g["id"].as_str() {
            md.set("musicbrainz_releasegroupid", gid);
        }
        let mut types: Vec<String> = Vec::new();
        if let Some(t) = g["type"].as_str() {
            types.push(t.to_lowercase());
        }
        for t in g["secondarytypes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            types.push(t.to_lowercase());
        }
        md.set_all("releasetype", types);
    }
    // The medium holding this recording (the API only lists the matching tracks).
    if let Some(medium) = rel["mediums"].as_array().and_then(|m| {
        m.iter()
            .find(|m| m["tracks"].as_array().is_some_and(|t| !t.is_empty()))
    }) {
        if let Some(p) = medium["position"].as_u64() {
            md.set("discnumber", p.to_string());
        }
        if let Some(f) = medium["format"].as_str() {
            md.set("media", f);
        }
        if let Some(n) = medium["track_count"].as_u64() {
            md.set("totaltracks", n.to_string());
        }
        if let Some(p) = medium["tracks"][0]["position"].as_u64() {
            md.set("tracknumber", p.to_string());
        }
    }
    Some(ReleaseRef { id, metadata: md })
}

/// `{year, month, day}` object or `"YYYY-MM-DD"` string → `YYYY[-MM[-DD]]`.
fn date_string(d: &Value) -> String {
    if let Some(s) = d.as_str() {
        return s.to_string();
    }
    let Some(y) = d["year"].as_u64() else {
        return String::new();
    };
    let mut s = format!("{y:04}");
    if let Some(m) = d["month"].as_u64() {
        s.push_str(&format!("-{m:02}"));
        if let Some(day) = d["day"].as_u64() {
            s.push_str(&format!("-{day:02}"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lookup_response() {
        let json: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/acoustid/lookup.json"))
                .unwrap();
        let results = parse_response(&json).unwrap();
        assert_eq!(results.len(), 2);
        // Sorted by score descending (fixture lists the weaker one first).
        assert!(results[0].score > results[1].score);
        let top = &results[0];
        assert_eq!(top.acoustid, "9ff43b6a-4f16-427c-93c2-92307ca505e0");
        let rec = &top.recordings[0];
        assert_eq!(rec.id, "cd2e7c47-16f5-46c6-a37c-a1eb7bf599ff");
        assert_eq!(rec.sources, 25);
        assert_eq!(rec.metadata.get("title"), Some("Intro"));
        assert_eq!(rec.metadata.get("artist"), Some("Ben Lee feat. Someone"));
        assert_eq!(rec.metadata.get_all("musicbrainz_artistid").len(), 2);
        assert_eq!(rec.metadata.length(), Some(Duration::from_millis(215_500)));
        // Release reachable through both the group and the flat list is deduplicated.
        assert_eq!(rec.releases.len(), 2);
        let r = &rec.releases[0];
        assert_eq!(r.metadata.get("album"), Some("Awake Is the New Sleep"));
        assert_eq!(r.metadata.get("date"), Some("2005-04-12"));
        assert_eq!(r.metadata.get("releasecountry"), Some("US"));
        assert_eq!(r.metadata.get("tracknumber"), Some("3"));
        assert_eq!(r.metadata.get("discnumber"), Some("2"));
        assert_eq!(r.metadata.get("media"), Some("CD"));
        assert_eq!(r.metadata.get("totaltracks"), Some("12"));
        assert_eq!(r.metadata.get("totaldiscs"), Some("2"));
        assert_eq!(r.metadata.get_all("releasetype"), ["album", "live"]);
        assert_eq!(
            r.metadata.get("musicbrainz_releasegroupid"),
            Some("1b1a1a1a-0000-4000-8000-000000000001")
        );
        // Release without a group and with a year-only date.
        let r2 = &rec.releases[1];
        assert_eq!(r2.metadata.get("date"), Some("2005"));
        assert!(r2.metadata.get("releasetype").is_none());
    }

    #[test]
    fn lookup_form_by_fingerprint_and_recording() {
        let form = lookup_form(
            "appkey",
            LookupKey::Fingerprint {
                fingerprint: "AQAA",
                duration: 215,
            },
        );
        let get = |k: &str| form.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("client"), Some("appkey"));
        assert_eq!(get("duration"), Some("215"));
        assert_eq!(get("fingerprint"), Some("AQAA"));
        assert_eq!(
            get("meta"),
            Some("recordings releasegroups releases tracks compress sources")
        );
        assert_eq!(get("format"), Some("json"));
        assert_eq!(get("clientversion"), Some(CLIENT_VERSION));
        let form = lookup_form("appkey", LookupKey::Recording("rid"));
        assert!(form.iter().any(|(k, v)| k == "recordingid" && v == "rid"));
        assert!(
            !form
                .iter()
                .any(|(k, _)| k == "fingerprint" || k == "duration")
        );
    }

    #[test]
    fn missing_score_and_sources_default_to_one() {
        let json = serde_json::json!({"status":"ok","results":[{"id":"a","recordings":[{"id":"r","title":"T"}]}]});
        let results = parse_response(&json).unwrap();
        assert_eq!(results[0].score, 1.0);
        assert_eq!(results[0].recordings[0].sources, 1);
        assert!(results[0].recordings[0].has_metadata());
    }

    #[test]
    fn error_status_maps_to_response_error() {
        let json =
            serde_json::json!({"status":"error","error":{"code":4,"message":"invalid API key"}});
        match parse_response(&json) {
            Err(Error::Response(m)) => assert_eq!(m, "invalid API key"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_results_ok() {
        let json = serde_json::json!({"status":"ok","results":[]});
        assert!(parse_response(&json).unwrap().is_empty());
    }
}
