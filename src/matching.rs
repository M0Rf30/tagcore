//! Similarity scoring modelled on Picard's `Metadata.compare` / `compare_to_release` /
//! `compare_to_track`.
//!
//! All scores are 0..=1.

mod tiers;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub use tiers::{MatchReason, best_match_with_margin, best_recording, release_score};

use crate::acoustid::LookupResult;
use crate::metadata::Metadata;
use crate::settings::MetadataSettings;

/// Lowercased, diacritic-free form with punctuation collapsed to single spaces. Apostrophes are
/// dropped rather than turned into separators ("N'" -> "n"), so "Don't" ~ "Dont".
pub(crate) fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.nfkd().filter(|c| !is_combining_mark(*c)) {
        if matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '`' | '\u{b4}') {
            continue;
        }
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// Similarity of two normalised strings: the better of plain and token-sorted normalised
/// Levenshtein (in the spirit of Picard's `similarity2`).
pub fn string_similarity(a: &str, b: &str) -> f32 {
    let (mut na, mut nb) = (normalize(a), normalize(b));
    if na.is_empty() && nb.is_empty() {
        // Symbol-only strings: fall back to a plain comparison.
        na = a.trim().to_lowercase();
        nb = b.trim().to_lowercase();
    }
    if na == nb {
        return 1.0;
    }
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    let plain = strsim::normalized_levenshtein(&na, &nb);
    let sorted = |s: &str| {
        let mut t: Vec<&str> = s.split(' ').collect();
        t.sort_unstable();
        t.join(" ")
    };
    let token = strsim::normalized_levenshtein(&sorted(&na), &sorted(&nb));
    // Token-order-insensitive comparison alongside plain edit distance, like `similarity2`'s
    // handling of reordered words.
    plain.max(token) as f32
}

/// Weighted mean over `(weight, score)` parts; None when there are no parts.
fn weighted(parts: &[(f32, f32)]) -> Option<f32> {
    let total: f32 = parts.iter().map(|p| p.0).sum();
    (total > 0.0).then(|| parts.iter().map(|p| p.0 * p.1).sum::<f32>() / total)
}

fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn year(s: &str) -> Option<i32> {
    s.trim().get(..4)?.parse().ok()
}

/// Picard's `Metadata.__weights`: tags compared by [`compare`] with their weights.
const COMPARE_WEIGHTS: [(&str, f32); 7] = [
    ("title", 22.0),
    ("artist", 6.0),
    ("album", 12.0),
    ("tracknumber", 6.0),
    ("totaltracks", 5.0),
    ("discnumber", 5.0),
    ("totaldiscs", 4.0),
];
/// Weight of the track length in [`compare`].
const COMPARE_LENGTH_WEIGHT: f32 = 8.0;

/// Picard's `Metadata.compare`: how alike two sets of tags are (0..1). Compares title, artist,
/// album, track/disc numbers and totals, and `~length`, skipping any tag in `ignored`
/// (`compare_ignore_tags`) and any tag one side lacks. `None` when nothing is comparable.
pub fn compare_opt(a: &Metadata, b: &Metadata, ignored: &[String]) -> Option<f32> {
    let is_ignored = |name: &str| ignored.iter().any(|i| i.eq_ignore_ascii_case(name));
    let mut parts: Vec<(f32, f32)> = Vec::new();
    if !is_ignored("~length")
        && let (Some(x), Some(y)) = (
            a.length().filter(|l| !l.is_zero()),
            b.length().filter(|l| !l.is_zero()),
        )
    {
        parts.push((
            COMPARE_LENGTH_WEIGHT,
            tiers::length_score(x.as_millis() as u64, y.as_millis() as u64),
        ));
    }
    for (name, weight) in COMPARE_WEIGHTS {
        if is_ignored(name) {
            continue;
        }
        let (x, y) = (a.display(name), b.display(name));
        if x.is_empty() || y.is_empty() {
            continue;
        }
        let score = if matches!(
            name,
            "tracknumber" | "totaltracks" | "discnumber" | "totaldiscs"
        ) {
            // Integers compare as numbers (`3` = `03`); anything else as text.
            match (x.trim().parse::<i64>(), y.trim().parse::<i64>()) {
                (Ok(i), Ok(j)) => f32::from(i == j),
                _ => f32::from(x == y),
            }
        } else {
            string_similarity(&x, &y)
        };
        parts.push((weight, score));
    }
    weighted(&parts)
}

/// [`compare_opt`], 0 when nothing is comparable.
pub fn compare(a: &Metadata, b: &Metadata, ignored: &[String]) -> f32 {
    compare_opt(a, b, ignored).unwrap_or(0.0)
}

fn track_similarity_opt(file: &Metadata, track: &Metadata) -> Option<f32> {
    compare_opt(file, track, &[])
}

/// Similarity of a file's metadata to a track's metadata: Picard's `Metadata.compare` without
/// ignored tags. Fields missing on either side are skipped; 0 when nothing is comparable.
pub fn track_similarity(file: &Metadata, track: &Metadata) -> f32 {
    compare(file, track, &[])
}

/// Assigns files to tracks one-to-one, best pairs first, discarding pairs below `threshold`.
/// Returns `assignment[i] = Some(track_index)` for `files[i]`.
pub fn assign_files(
    files: &[&Metadata],
    tracks: &[&Metadata],
    threshold: f32,
) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        for (j, t) in tracks.iter().enumerate() {
            let s = track_similarity(f, t);
            if s >= threshold && s > 0.0 {
                pairs.push((s, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out = vec![None; files.len()];
    let mut taken = vec![false; tracks.len()];
    for (_, i, j) in pairs {
        if out[i].is_none() && !taken[j] {
            out[i] = Some(j);
            taken[j] = true;
        }
    }
    out
}

/// Picks the best `(recording_id, release_id)` from AcoustID results for a file, combining
/// AcoustID score, source count, and similarity of each release/track to the file's existing
/// tags (prefers releases matching the file's album/date/totaltracks, preferred countries and
/// formats, then official albums over compilations). None below `threshold`.
pub fn best_acoustid_match(
    file: &Metadata,
    results: &[LookupResult],
    settings: &MetadataSettings,
    threshold: f32,
) -> Option<(String, String)> {
    let mut best: Option<(f32, &str, &str)> = None;
    for result in results {
        for rec in &result.recordings {
            for rel in &rec.releases {
                let mut combined = rec.metadata.clone();
                combined.update(&rel.metadata);
                let mut sim = track_similarity_opt(file, &combined).unwrap_or(0.5);
                // Date/year agreement is not part of track_similarity; fold it in.
                if let (Some(a), Some(b)) = (
                    file.get("date").and_then(year),
                    combined.get("date").and_then(year),
                ) {
                    sim = 0.9 * sim + 0.1 * f32::from(a == b);
                }
                let sources = (0.02 * (1.0 + rec.sources as f32).ln()).min(0.08);
                let pref = 0.03 * tiers::preference_score(&rel.metadata, settings).unwrap_or(0.0);
                let kind = 0.03 * tiers::type_preference(&rel.metadata, settings).unwrap_or(0.0);
                let score =
                    (result.score * (0.6 + 0.4 * sim) + sources + pref + kind).clamp(0.0, 1.0);
                if best.is_none_or(|(s, ..)| score > s) {
                    best = Some((score, &rec.id, &rel.id));
                }
            }
        }
    }
    best.filter(|(s, ..)| *s >= threshold)
        .map(|(_, r, rel)| (r.to_owned(), rel.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::acoustid::{Recording, ReleaseRef};

    fn md(pairs: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::new();
        for (k, v) in pairs {
            m.set(k, *v);
        }
        m
    }

    fn track(n: u32, title: &str, secs: u64) -> Metadata {
        let mut m = md(&[
            ("title", title),
            ("tracknumber", &n.to_string()),
            ("album", "Album"),
        ]);
        m.set_length(Duration::from_secs(secs));
        m
    }

    #[test]
    fn similarity_ignores_accents_and_punctuation() {
        assert_eq!(string_similarity("Beyoncé", "Beyonce"), 1.0);
        assert_eq!(string_similarity("Guns N' Roses", "Guns N Roses"), 1.0);
        assert_eq!(string_similarity("  AC/DC ", "ac dc"), 1.0);
        assert!(string_similarity("Dark Side of the Moon", "The Dark Side of Moon") > 0.8);
        assert!(string_similarity("Thriller", "Back in Black") < 0.4);
        assert_eq!(string_similarity("", "x"), 0.0);
    }

    #[test]
    fn assign_ten_files_to_twelve_tracks_shuffled_with_jitter() {
        let titles = [
            "Opening Credits",
            "Midnight Rain",
            "Paper Boats",
            "Silver Lining",
            "Northern Lights",
            "Broken Compass",
            "Glass Houses",
            "Harbour Fog",
            "Winter Garden",
            "Static Dreams",
            "Last Train Home",
            "Epilogue",
        ];
        let lens = [200u64, 245, 180, 310, 220, 275, 190, 260, 230, 205, 340, 95];
        let tracks: Vec<Metadata> = (0..12)
            .map(|i| track(i as u32 + 1, titles[i], lens[i]))
            .collect();
        // Files are tracks 1..=10 (indices 0..10), shuffled, with +3 s length jitter, no numbers.
        let order = [7usize, 2, 9, 0, 5, 3, 8, 1, 6, 4];
        let files: Vec<Metadata> = order
            .iter()
            .map(|&i| {
                let mut m = md(&[("title", titles[i]), ("album", "Album")]);
                m.set_length(Duration::from_secs(lens[i] + 3));
                m
            })
            .collect();
        let f: Vec<&Metadata> = files.iter().collect();
        let t: Vec<&Metadata> = tracks.iter().collect();
        let a = assign_files(&f, &t, 0.5);
        for (k, &i) in order.iter().enumerate() {
            assert_eq!(a[k], Some(i), "file {k}");
        }
    }

    #[test]
    fn assign_is_one_to_one_and_respects_threshold() {
        let t = [track(1, "Alpha", 100)];
        let f1 = track(1, "Alpha", 100);
        let f2 = md(&[("title", "Zzzzzz Qqqq")]);
        let a = assign_files(&[&f1, &f1.clone(), &f2], &[&t[0]], 0.5);
        assert_eq!(a, vec![Some(0), None, None]);
    }

    #[test]
    fn track_similarity_skips_missing_fields() {
        let file = md(&[("title", "Song")]);
        let tr = track(4, "Song", 200);
        assert_eq!(track_similarity(&file, &tr), 1.0);
        assert_eq!(track_similarity(&md(&[("genre", "x")]), &tr), 0.0);
        // Length: linear from 1 (equal) to 0 (30 s apart), like Picard's `length_score`.
        let mut a = Metadata::new();
        a.set_length(Duration::from_secs(100));
        let mut b = Metadata::new();
        b.set_length(Duration::from_secs(100));
        assert_eq!(track_similarity(&a, &b), 1.0);
        b.set_length(Duration::from_secs(130));
        assert_eq!(track_similarity(&a, &b), 0.0);
        b.set_length(Duration::from_secs(115));
        assert!((track_similarity(&a, &b) - 0.5).abs() < 0.01);
    }

    #[test]
    fn compare_uses_picards_weights_numbers_and_ignore_list() {
        let a = md(&[
            ("title", "Song"),
            ("album", "Other"),
            ("tracknumber", "03"),
            ("totaltracks", "12/12"),
        ]);
        let b = md(&[
            ("title", "Song"),
            ("album", "Zzzz Qqqq"),
            ("tracknumber", "3"),
            ("totaltracks", "12"),
        ]);
        // title 22 → 1, album 12 → low, tracknumber 6 → 1 (`03` = `3`), totaltracks 5 → 0 (`12/12` is text).
        let full = compare(&a, &b, &[]);
        assert!(full > 0.5 && full < 0.8, "{full}");
        let ignoring = compare(&a, &b, &["Album".to_owned(), "totaltracks".to_owned()]);
        assert!(
            (ignoring - 1.0).abs() < 1e-6,
            "title and tracknumber agree: {ignoring}"
        );
        assert_eq!(
            compare(
                &a,
                &b,
                &[
                    "title".into(),
                    "album".into(),
                    "tracknumber".into(),
                    "totaltracks".into()
                ]
            ),
            0.0
        );
        assert_eq!(compare_opt(&Metadata::new(), &b, &[]), None);
    }

    #[test]
    fn acoustid_prefers_release_matching_file_album_over_compilation() {
        let file = md(&[
            ("title", "Song"),
            ("album", "Real Album"),
            ("tracknumber", "3"),
        ]);
        let rec_md = md(&[("title", "Song")]);
        let rel = |id: &str, album: &str, ty: &[&str], tn: &str| {
            let mut m = md(&[("album", album), ("tracknumber", tn)]);
            m.set_all("releasetype", ty.iter().map(|s| s.to_string()));
            ReleaseRef {
                id: id.into(),
                metadata: m,
            }
        };
        let results = vec![LookupResult {
            acoustid: "a".into(),
            score: 0.98,
            recordings: vec![
                Recording {
                    id: "rec-comp".into(),
                    metadata: rec_md.clone(),
                    sources: 500,
                    releases: vec![rel("comp", "Hits 2001", &["album", "compilation"], "11")],
                },
                Recording {
                    id: "rec-real".into(),
                    metadata: rec_md,
                    sources: 20,
                    releases: vec![rel("real", "Real Album", &["album"], "3")],
                },
                Recording {
                    id: "standalone".into(),
                    metadata: Metadata::new(),
                    sources: 900,
                    releases: vec![],
                },
            ],
        }];
        let got = best_acoustid_match(&file, &results, &MetadataSettings::default(), 0.5);
        assert_eq!(got, Some(("rec-real".into(), "real".into())));
        assert_eq!(
            best_acoustid_match(&file, &results[..0], &MetadataSettings::default(), 0.0),
            None
        );
    }
}
