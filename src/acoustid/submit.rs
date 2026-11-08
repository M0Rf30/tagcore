//! AcoustID fingerprint submission (`https://api.acoustid.org/v2/submit`), ported from Picard's
//! `acoustid/manager.py` and `webservice/api_helpers/acoustid.py`.
//!
//! A [`Submission`] links one fingerprint to a MusicBrainz recording id (when the file's length is
//! within 30 s of the recording's) or to plain metadata. [`submit_all`] sends them in batches that
//! stay under AcoustID's ~1 MB limit, shrinks the batch on HTTP 413 and gives each item at most
//! five attempts.

use std::time::Duration;

use crate::Metadata;

/// Message used when AcoustID reports an error without one. Hosts that translate their UI
/// compare error messages against this.
pub const UNKNOWN_ERROR: &str = "unknown AcoustID error";

pub const SUBMIT_URL: &str = "https://api.acoustid.org/v2/submit";

/// AcoustID has a post limit of around 1 MB.
pub const MAX_PAYLOAD: usize = 1_000_000;
/// Limit each submission to this many attempts.
pub const MAX_ATTEMPTS: u32 = 5;
/// On "Payload Too Large" the batch size shrinks by this factor and the batch is retried.
pub const BATCH_SIZE_REDUCTION_FACTOR: f64 = 0.7;
/// Maximum difference between the file's length and the MusicBrainz recording's length; above
/// it the MBID is not submitted (mirrors the acoustid-server limit).
pub const MAX_ALLOWED_LENGTH_DIFF_MS: u64 = 30_000;

/// The tags of a file that AcoustID wants when no (trusted) MBID is submitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmissionTags {
    pub puid: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub tracknumber: String,
    pub discnumber: String,
    /// `year`, else the first four characters of `date`; only kept when decimal.
    pub year: String,
    /// `~length` in milliseconds (0 when unknown, like Picard).
    pub length_ms: u64,
}

impl SubmissionTags {
    pub fn from_metadata(md: &Metadata) -> Self {
        let get = |k: &str| md.get(k).unwrap_or_default().to_owned();
        let year = md
            .get("year")
            .map(str::to_owned)
            .filter(|y| !y.is_empty())
            .unwrap_or_else(|| md.get("date").unwrap_or_default().chars().take(4).collect());
        Self {
            puid: get("musicip_puid"),
            title: get("title"),
            artist: get("artist"),
            album: get("album"),
            albumartist: get("albumartist"),
            tracknumber: get("tracknumber"),
            discnumber: get("discnumber"),
            year: if !year.is_empty() && year.chars().all(|c| c.is_ascii_digit()) {
                year
            } else {
                String::new()
            },
            length_ms: md.length().map_or(0, |d| d.as_millis() as u64),
        }
    }
}

/// One fingerprint waiting to be submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub fingerprint: String,
    /// Whole seconds.
    pub duration: u32,
    /// Recording the file is matched to now.
    pub recordingid: Option<String>,
    /// Recording last submitted for this fingerprint.
    pub orig_recordingid: Option<String>,
    pub attempts: u32,
    /// `None` = no metadata is known (Picard's `metadata is None`).
    pub tags: Option<SubmissionTags>,
}

impl Submission {
    pub fn new(
        fingerprint: impl Into<String>,
        duration: u32,
        recordingid: Option<String>,
        tags: Option<SubmissionTags>,
    ) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            duration,
            orig_recordingid: recordingid.clone(),
            recordingid,
            attempts: 0,
            tags,
        }
    }

    /// Picard's `__bool__`.
    pub fn is_valid(&self) -> bool {
        !self.fingerprint.is_empty() && self.duration > 0
    }

    /// Whether the file's length is close enough to the recording's to submit the MBID.
    pub fn valid_duration(&self) -> bool {
        self.tags.as_ref().is_none_or(|t| {
            (u64::from(self.duration) * 1000).abs_diff(t.length_ms) <= MAX_ALLOWED_LENGTH_DIFF_MS
        })
    }

    /// Picard's `is_submitted`: nothing to link, or the link is already on the server.
    pub fn is_submitted(&self) -> bool {
        self.recordingid.is_none() || self.orig_recordingid == self.recordingid
    }

    /// Arguments of this submission, without the batch index.
    pub fn args(&self) -> Vec<(&'static str, String)> {
        let mut args = vec![
            ("fingerprint", self.fingerprint.clone()),
            ("duration", self.duration.to_string()),
        ];
        if let Some(t) = &self.tags
            && !t.puid.is_empty()
        {
            args.push(("puid", t.puid.clone()));
        }
        match (&self.recordingid, &self.tags) {
            (Some(mbid), _) if self.valid_duration() => args.push(("mbid", mbid.clone())),
            (_, Some(t)) => {
                args.push(("track", t.title.clone()));
                args.push(("artist", t.artist.clone()));
                args.push(("album", t.album.clone()));
                args.push(("albumartist", t.albumartist.clone()));
                args.push(("trackno", t.tracknumber.clone()));
                args.push(("discno", t.discnumber.clone()));
                if !t.year.is_empty() {
                    args.push(("year", t.year.clone()));
                }
            }
            _ => {}
        }
        args
    }

    /// Payload approximation: all submitted data plus a little for urlencode expansion (Picard's
    /// `__len__`, counting characters).
    pub fn payload_len(&self) -> usize {
        let sum: usize = self
            .args()
            .iter()
            .map(|(k, v)| k.chars().count() + v.chars().count() + 2)
            .sum();
        (sum as f64 * 1.03) as usize
    }
}

/// Form body of a `/submit` request: `user`, `key.i` for every non-empty argument of the `i`th
/// submission, then the client identification.
pub fn submit_form(
    client_key: &str,
    user_key: &str,
    batch: &[&Submission],
) -> Vec<(String, String)> {
    let mut form = vec![("user".to_owned(), user_key.to_owned())];
    for (i, submission) in batch.iter().enumerate() {
        for (key, value) in submission.args() {
            if !value.is_empty() {
                form.push((format!("{key}.{i}"), value));
            }
        }
    }
    form.push(("client".into(), client_key.to_owned()));
    form.push(("clientversion".into(), super::CLIENT_VERSION.to_owned()));
    form.push(("format".into(), "json".into()));
    form
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    /// HTTP status, when the server answered.
    pub status: Option<u16>,
    pub message: String,
}

/// Why [`submit_all`] gave up on the remaining items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 413 responses shrank the batch size to zero.
    BatchSizeZero,
    /// The remaining items used up their attempts.
    TooManyRetries,
}

/// Overall result of a submission, for the host to turn into a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Everything reached AcoustID.
    Success,
    /// Finished, but at least one request failed along the way.
    Incomplete,
    /// The remaining items were given up on.
    Failed(Failure),
}

/// The outcome of [`submit_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report<K> {
    /// Keys whose fingerprints reached AcoustID.
    pub submitted: Vec<K>,
    /// One message per failed request (413 shrinks are not errors).
    pub errors: Vec<String>,
    /// Set when the remaining items were given up on.
    pub failure: Option<Failure>,
}

impl<K> Report<K> {
    pub fn outcome(&self) -> Outcome {
        match (self.failure, self.errors.is_empty()) {
            (Some(failure), _) => Outcome::Failed(failure),
            (None, true) => Outcome::Success,
            (None, false) => Outcome::Incomplete,
        }
    }

    pub fn is_success(&self) -> bool {
        self.outcome() == Outcome::Success
    }
}

type Pending<K> = Vec<(K, Submission)>;

/// Picard's `_batch`: takes items in order while the payload stays below `max_batch_size`; items
/// that exhausted their attempts, and everything after the first one that does not fit, stay in
/// the remainder.
fn take_batch<K>(items: Pending<K>, max_batch_size: usize) -> (Pending<K>, Pending<K>) {
    let mut batch = Vec::new();
    let mut remaining = Vec::new();
    let mut batch_size = 0usize;
    let mut max_attempts = MAX_ATTEMPTS;
    for (key, mut submission) in items {
        if submission.attempts < max_attempts {
            batch_size += submission.payload_len();
            if batch_size < max_batch_size {
                submission.attempts += 1;
                batch.push((key, submission));
                continue;
            }
            // Once the size limit is hit, everything else waits for the next round.
            max_attempts = 0;
        }
        remaining.push((key, submission));
    }
    (batch, remaining)
}

/// Submits `items` through `send` (one call per batch) and reports what happened. Attempt
/// counters start from zero. `send` is where the throttling and the HTTP request live.
pub fn submit_all<K>(
    items: Vec<(K, Submission)>,
    send: &mut dyn FnMut(&[&Submission]) -> Result<(), SendError>,
) -> Report<K> {
    let mut report = Report {
        submitted: Vec::new(),
        errors: Vec::new(),
        failure: None,
    };
    let mut max_batch_size = MAX_PAYLOAD;
    let mut pending: Pending<K> = items
        .into_iter()
        .filter(|(_, s)| s.is_valid() && !s.is_submitted())
        .map(|(k, mut s)| {
            s.attempts = 0;
            (k, s)
        })
        .collect();
    while !pending.is_empty() {
        let (batch, mut remaining) = take_batch(pending, max_batch_size);
        if batch.is_empty() {
            report.failure = Some(if max_batch_size == 0 {
                Failure::BatchSizeZero
            } else {
                Failure::TooManyRetries
            });
            return report;
        }
        let refs: Vec<&Submission> = batch.iter().map(|(_, s)| s).collect();
        match send(&refs) {
            Ok(()) => report.submitted.extend(batch.into_iter().map(|(k, _)| k)),
            Err(e) if e.status == Some(413) => {
                max_batch_size = (max_batch_size as f64 * BATCH_SIZE_REDUCTION_FACTOR) as usize;
                log::warn!("AcoustID: payload too large, batch size reduced to {max_batch_size}");
                remaining.extend(batch);
            }
            Err(e) => {
                log::error!("AcoustID submission failed: {}", e.message);
                report.errors.push(e.message);
                remaining.extend(batch);
            }
        }
        pending = remaining;
    }
    report
}

/// Submits to the real AcoustID service. `client_key` is the application key, `user_key` the
/// user's key from <https://acoustid.org/api-key>.
pub fn submit<K>(client_key: &str, user_key: &str, items: Vec<(K, Submission)>) -> Report<K> {
    submit_all(items, &mut |batch| {
        crate::http::throttle("api.acoustid.org", Duration::from_millis(334));
        let form = submit_form(client_key, user_key, batch);
        let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match crate::http::post_form_json(SUBMIT_URL, &pairs) {
            Ok(json) if json["status"].as_str() == Some("error") => Err(SendError {
                status: None,
                message: json["error"]["message"]
                    .as_str()
                    .unwrap_or(UNKNOWN_ERROR)
                    .to_owned(),
            }),
            Ok(_) => Ok(()),
            Err(e) => Err(SendError {
                status: crate::http::status_code(&e),
                message: e.to_string(),
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(length_ms: u64) -> SubmissionTags {
        SubmissionTags {
            title: "Song".into(),
            artist: "Band".into(),
            album: "LP".into(),
            albumartist: "Band".into(),
            tracknumber: "3".into(),
            discnumber: "1".into(),
            year: "2005".into(),
            length_ms,
            ..SubmissionTags::default()
        }
    }

    fn sub(fp: &str, mbid: Option<&str>) -> Submission {
        let mut s = Submission::new(fp, 200, mbid.map(str::to_owned), Some(tags(200_000)));
        s.orig_recordingid = None;
        s
    }

    fn get<'a>(args: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        args.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn mbid_is_submitted_only_within_thirty_seconds() {
        let close = sub("fp", Some("rec"));
        assert_eq!(get(&close.args(), "mbid"), Some("rec"));
        assert_eq!(get(&close.args(), "track"), None);

        let mut far = sub("fp", Some("rec"));
        far.tags = Some(tags(200_000 + 30_001));
        assert!(!far.valid_duration());
        let args = far.args();
        assert_eq!(get(&args, "mbid"), None);
        assert_eq!(get(&args, "track"), Some("Song"));
        assert_eq!(get(&args, "trackno"), Some("3"));
        assert_eq!(get(&args, "year"), Some("2005"));

        let mut edge = sub("fp", Some("rec"));
        edge.tags = Some(tags(200_000 - 30_000));
        assert_eq!(get(&edge.args(), "mbid"), Some("rec"));

        // Without a recording the tags go out as plain metadata.
        let none = sub("fp", None);
        assert_eq!(get(&none.args(), "artist"), Some("Band"));
        // Without any metadata only fingerprint and duration are sent.
        let bare = Submission::new("fp", 200, Some("rec".into()), None);
        assert_eq!(bare.args().len(), 3);
        assert_eq!(get(&bare.args(), "mbid"), Some("rec"));
        assert_eq!(Submission::new("fp", 200, None, None).args().len(), 2);
    }

    #[test]
    fn tags_from_metadata_derive_year_and_length() {
        let mut md = Metadata::new();
        md.set("date", "2001-02-03");
        md.set("musicip_puid", "puid1");
        md.set_length(Duration::from_millis(1234));
        let t = SubmissionTags::from_metadata(&md);
        assert_eq!(
            (t.year.as_str(), t.puid.as_str(), t.length_ms),
            ("2001", "puid1", 1234)
        );
        md.set("date", "xx");
        assert_eq!(SubmissionTags::from_metadata(&md).year, "");
        md.set("year", "1999");
        assert_eq!(SubmissionTags::from_metadata(&md).year, "1999");
    }

    #[test]
    fn submitted_state_follows_recording_link() {
        let mut s = Submission::new("fp", 100, None, None);
        assert!(s.is_submitted());
        s.recordingid = Some("r".into());
        assert!(!s.is_submitted());
        s.orig_recordingid = Some("r".into());
        assert!(s.is_submitted());
    }

    #[test]
    fn form_indexes_non_empty_arguments() {
        let mut a = sub("fpA", Some("recA"));
        a.tags.as_mut().unwrap().puid = "pu".into();
        let b = sub("fpB", None);
        let form = submit_form("app", "usr", &[&a, &b]);
        let get = |k: &str| form.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(form[0], ("user".to_owned(), "usr".to_owned()));
        assert_eq!(get("fingerprint.0"), Some("fpA"));
        assert_eq!(get("duration.0"), Some("200"));
        assert_eq!(get("puid.0"), Some("pu"));
        assert_eq!(get("mbid.0"), Some("recA"));
        assert_eq!(get("fingerprint.1"), Some("fpB"));
        assert_eq!(get("track.1"), Some("Song"));
        assert_eq!(get("mbid.1"), None);
        assert_eq!(get("client"), Some("app"));
        assert_eq!(get("format"), Some("json"));
        assert!(get("clientversion").is_some());
        // Empty values (no puid on #1, no mbid) are dropped.
        assert_eq!(get("puid.1"), None);
    }

    #[test]
    fn batches_stay_under_the_payload_limit() {
        // Each item is ~ (fingerprint + small) chars; 4 of ~300 kB do not fit under 1 MB together.
        let big = "x".repeat(300_000);
        let items: Vec<(usize, Submission)> = (0..4).map(|i| (i, sub(&big, Some("rec")))).collect();
        let mut sizes = Vec::new();
        let report = submit_all(items, &mut |batch| {
            sizes.push(batch.len());
            Ok(())
        });
        assert_eq!(sizes, vec![3, 1]);
        assert_eq!(report.submitted, vec![0, 1, 2, 3]);
        assert!(report.is_success());
        assert_eq!(report.outcome(), Outcome::Success);
    }

    #[test]
    fn payload_too_large_shrinks_the_batch_and_retries() {
        let fp = "y".repeat(100_000);
        let items: Vec<(usize, Submission)> = (0..5).map(|i| (i, sub(&fp, Some("rec")))).collect();
        let mut calls = Vec::new();
        let report = submit_all(items, &mut |batch| {
            calls.push(batch.len());
            // The server only accepts up to two items per request.
            if batch.len() > 2 {
                Err(SendError {
                    status: Some(413),
                    message: "too large".into(),
                })
            } else {
                Ok(())
            }
        });
        // 5 items are ~515 kB: all five go first and are refused; the batch keeps shrinking
        // (700 kB, 490 kB, 343 kB, 240 kB limit) until at most two fit.
        assert_eq!(calls[0], 5);
        assert!(calls.len() >= 4, "{calls:?}");
        assert!(calls.last().is_some_and(|n| *n <= 2), "{calls:?}");
        assert_eq!(report.submitted.len(), 5);
        assert!(report.errors.is_empty(), "413 is not reported as an error");
    }

    #[test]
    fn gives_up_after_five_attempts() {
        let items = vec![(1usize, sub("fp", Some("rec")))];
        let mut calls = 0;
        let report = submit_all(items, &mut |_| {
            calls += 1;
            Err(SendError {
                status: Some(500),
                message: "boom".into(),
            })
        });
        assert_eq!(calls, MAX_ATTEMPTS);
        assert!(report.submitted.is_empty());
        assert_eq!(report.errors.len(), MAX_ATTEMPTS as usize);
        assert_eq!(report.failure, Some(Failure::TooManyRetries));
    }

    #[test]
    fn an_item_larger_than_the_limit_fails_permanently() {
        let huge = "z".repeat(MAX_PAYLOAD);
        let report = submit_all(vec![(0usize, sub(&huge, Some("rec")))], &mut |_| Ok(()));
        assert!(report.submitted.is_empty());
        assert!(report.failure.is_some());
    }

    #[test]
    fn already_submitted_and_invalid_items_are_skipped() {
        let mut done = sub("fp", Some("rec"));
        done.orig_recordingid = Some("rec".into());
        let nolink = sub("fp2", None);
        let empty = Submission::new("", 0, Some("rec".into()), None);
        let mut sent = 0;
        let report = submit_all(
            vec![(0usize, done), (1, nolink), (2, empty)],
            &mut |batch| {
                sent += batch.len();
                Ok(())
            },
        );
        assert_eq!(sent, 0);
        assert!(report.submitted.is_empty() && report.is_success());
    }

    #[test]
    fn partial_failure_is_reported_in_the_summary() {
        let items = vec![(0usize, sub("fp", Some("rec")))];
        let mut n = 0;
        let report = submit_all(items, &mut |_| {
            n += 1;
            if n == 1 {
                Err(SendError {
                    status: Some(503),
                    message: "busy".into(),
                })
            } else {
                Ok(())
            }
        });
        assert_eq!(report.submitted, vec![0]);
        assert_eq!(report.errors, vec!["busy".to_owned()]);
        assert_eq!(report.outcome(), Outcome::Incomplete);
    }
}
