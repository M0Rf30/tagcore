//! Picard's tiered release and recording matching (`matching.py`: `compare_to_release`,
//! `compare_to_track`, `find_best_match_with_margin`).
//!
//! Scores come in three tiers. Identifiers (barcode, catalogue number, ISRC) are decisive when
//! present; similarity (album, artist, track count, date, title, length, ...) carries the match
//! otherwise; preferences (countries, formats, release types) only break ties. The result is
//! multiplied by the search server's relevance score.

use super::{leading_number, string_similarity};
use crate::mb::RecordingCandidate;
use crate::metadata::Metadata;
use crate::release::ReleaseCandidate;
use crate::settings::MetadataSettings;

/// `(score, weight)`.
type Part = (f32, f32);

/// Weight that makes a release lose against any other (Picard `_SKIP_RELEASE_WEIGHT`).
const SKIP_RELEASE: Part = (0.0, 999.0);
/// Length difference (ms) at which the length score reaches 0.
const LENGTH_SCORE_THRESHOLD_MS: f32 = 30_000.0;
const DISCRIMINATOR_TITLE_MATCH: f32 = 0.8;
const DISCRIMINATOR_MISMATCH: f32 = 0.4;
const DISCRIMINATOR_ARTIST_PENALTY: f32 = 0.6;
const DISCRIMINATOR_BOTH_PENALTY: f32 = 0.5;
const CATNO_NEAR_MISS_THRESHOLD: f32 = 0.6;
const CATNO_NEAR_MISS_CEILING: f32 = 0.75;

/// Per-field weights of one comparison; 0 disables a field. Picard's
/// `CLUSTER_COMPARISON_WEIGHTS` / `FILE_COMPARISON_WEIGHTS`.
#[derive(Debug, Clone, Copy, Default)]
struct Weights {
    barcode: f32,
    catno: f32,
    isrc: f32,
    album: f32,
    albumartist: f32,
    artist: f32,
    title: f32,
    length: f32,
    tracknumber: f32,
    date: f32,
    /// Compared with the cluster's `~totalalbumtracks`/`totaltracks`.
    total_album_tracks: f32,
    /// Compared with the file's `totaltracks` against the best matching medium.
    total_tracks: f32,
    format: f32,
    isvideo: f32,
    country: f32,
    release_type: f32,
}

const CLUSTER_WEIGHTS: Weights = Weights {
    barcode: 28.0,
    catno: 22.0,
    album: 17.0,
    albumartist: 6.0,
    date: 4.0,
    total_album_tracks: 5.0,
    format: 2.0,
    country: 2.0,
    release_type: 10.0,
    isrc: 0.0,
    artist: 0.0,
    title: 0.0,
    length: 0.0,
    tracknumber: 0.0,
    total_tracks: 0.0,
    isvideo: 0.0,
};

const FILE_WEIGHTS: Weights = Weights {
    barcode: 28.0,
    catno: 22.0,
    isrc: 28.0,
    album: 5.0,
    artist: 4.0,
    date: 4.0,
    length: 10.0,
    title: 13.0,
    total_tracks: 4.0,
    tracknumber: 4.0,
    format: 2.0,
    isvideo: 2.0,
    country: 2.0,
    release_type: 14.0,
    albumartist: 0.0,
    total_album_tracks: 0.0,
};

/// Picard's `ReleaseMatchParts`.
#[derive(Debug, Clone)]
struct Parts {
    identifiers: Vec<Part>,
    similarity: Vec<Part>,
    preferences: Vec<Part>,
    /// Multiplier on the similarity tier when the title matches but the artist contradicts.
    penalty: f32,
}

impl Parts {
    fn new() -> Self {
        Self {
            identifiers: Vec::new(),
            similarity: Vec::new(),
            preferences: Vec::new(),
            penalty: 1.0,
        }
    }

    fn merged(mut self, other: Parts) -> Self {
        self.identifiers.extend(other.identifiers);
        self.similarity.extend(other.similarity);
        self.preferences.extend(other.preferences);
        self.penalty *= other.penalty;
        self
    }

    /// Picard's `combine_tiers`.
    fn combine(&self) -> f32 {
        let sim = linear(&self.similarity).map(|s| s * self.penalty);
        let pref = linear(&self.preferences).unwrap_or(0.5);
        let Some(id) = linear(&self.identifiers) else {
            return match sim {
                None => pref,
                Some(sim) => sim * 0.9 + pref * 0.1,
            };
        };
        if id >= 0.9 {
            0.85 + sim.unwrap_or(1.0) * 0.1 + pref * 0.05
        } else if id <= 0.1 {
            (sim.unwrap_or(0.0) * 0.3).min(0.3)
        } else {
            id * 0.4 + sim.unwrap_or(0.5) * 0.5 + pref * 0.1
        }
    }
}

/// `linear_combination_of_weights`; `None` for no parts (or zero total weight).
fn linear(parts: &[Part]) -> Option<f32> {
    let total: f32 = parts.iter().map(|p| p.1).sum();
    (!parts.is_empty() && total > 0.0).then(|| parts.iter().map(|p| p.0 * p.1).sum::<f32>() / total)
}

// ---- component scores ----------------------------------------------------------------------

/// Picard's `compare_barcodes`: equal, or equal after zero-padding to 13 digits (UPC vs EAN).
fn barcodes_match(a: &str, b: &str) -> bool {
    a == b || (!a.is_empty() && !b.is_empty() && format!("{a:0>13}") == format!("{b:0>13}"))
}

/// Picard's `_date_score`.
fn date_score(release_date: Option<&str>, file_date: Option<&str>) -> f32 {
    let Some(release) = release_date.filter(|d| !d.is_empty()) else {
        return 0.25;
    };
    let Some(file) = file_date.filter(|d| !d.is_empty()) else {
        return 0.65;
    };
    if release == file {
        return 1.0;
    }
    let (Some(a), Some(b)) = (year_of(release), year_of(file)) else {
        return 0.0;
    };
    match a.abs_diff(b) {
        0 => 0.95,
        1 | 2 => 0.85,
        _ => 0.0,
    }
}

fn year_of(date: &str) -> Option<i32> {
    date.trim().get(..4)?.parse().ok()
}

/// Picard's `_trackcount_score`.
fn trackcount_score(actual: u32, expected: u32) -> f32 {
    if actual == expected {
        return 1.0;
    }
    if expected == 0 {
        return 0.0;
    }
    let ratio = actual.abs_diff(expected) as f32 / expected as f32;
    let factor = if actual > expected { 3.0 } else { 2.0 };
    (1.0 - ratio * factor).max(0.0)
}

fn is_placeholder_catno(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("[none]")
}

/// Picard's `_catno_label_score`: `release_catnos` and `release_labels` are the release's
/// catalogue numbers and label names.
fn catno_label_score(
    file_catno: &str,
    file_label: &str,
    release_catnos: &[String],
    release_labels: &[String],
) -> f32 {
    if is_placeholder_catno(file_catno) {
        return 0.5;
    }
    let candidates: Vec<&String> = release_catnos
        .iter()
        .filter(|c| !c.is_empty() && !is_placeholder_catno(c))
        .collect();
    if candidates.is_empty() {
        return 0.5; // the release has no catalogue number: neutral
    }
    let file_catno = file_catno.trim().to_lowercase();
    let file_label = file_label.trim().to_lowercase();
    let mut best = 0.0_f32;
    for catno in candidates {
        let catno = catno.trim().to_lowercase();
        if file_catno == catno {
            if file_label.is_empty() || release_labels.is_empty() {
                return 1.0;
            }
            return if release_labels
                .iter()
                .any(|l| l.trim().to_lowercase() == file_label)
            {
                1.0
            } else {
                0.8
            };
        }
        if !file_catno.is_empty() && !catno.is_empty() {
            best = best.max(strsim::normalized_levenshtein(&file_catno, &catno) as f32);
        }
    }
    if best >= CATNO_NEAR_MISS_THRESHOLD {
        best.min(CATNO_NEAR_MISS_CEILING)
    } else {
        0.0
    }
}

/// Picard's `length_score`: linear from 1 (equal) to 0 (30 s apart).
pub(super) fn length_score(a_ms: u64, b_ms: u64) -> f32 {
    1.0 - (a_ms.abs_diff(b_ms) as f32).min(LENGTH_SCORE_THRESHOLD_MS) / LENGTH_SCORE_THRESHOLD_MS
}

/// Picard's `_isrcs_score`.
fn isrcs_score(file: &[String], track: &[String]) -> f32 {
    let up = |v: &[String]| -> Vec<String> { v.iter().map(|s| s.to_uppercase()).collect() };
    let (file, track) = (up(file), up(track));
    if file.is_empty() || track.is_empty() {
        0.5
    } else if file.iter().all(|i| track.contains(i)) {
        1.0
    } else if file.iter().any(|i| track.contains(i)) {
        0.8
    } else {
        0.0
    }
}

/// Picard's `_compare_tracknumber`: 1 when the file's track number is the candidate's position,
/// 0 when not, 0.5 when the file has no usable number.
fn tracknumber_score(file: &Metadata, candidate: &Metadata) -> f32 {
    match file
        .get("tracknumber")
        .and_then(leading_number)
        .filter(|&n| n > 0)
    {
        Some(n) => f32::from(candidate.get("tracknumber").and_then(leading_number) == Some(n)),
        None => 0.5,
    }
}

// ---- preferences ----------------------------------------------------------------------------

/// The `(n - i) / n` score of position `i` in a preference list of `n` entries; `None` for an
/// empty list.
fn list_preference(list: &[String], value: Option<&str>) -> Option<f32> {
    if list.is_empty() {
        return None;
    }
    let n = list.len() as f32;
    Some(
        value
            .and_then(|v| list.iter().position(|p| p == v))
            .map_or(0.0, |i| (n - i as f32) / n),
    )
}

/// Picard's `_weights_from_preferred_formats`: the mean over the media that have a format.
fn format_preference(list: &[String], formats: &[String]) -> Option<f32> {
    if list.is_empty() || formats.is_empty() {
        return None;
    }
    let n = list.len() as f32;
    let total: f32 = formats
        .iter()
        .map(|f| {
            list.iter()
                .position(|p| p == f)
                .map_or(0.0, |i| (n - i as f32) / n)
        })
        .sum();
    Some(total / formats.len() as f32)
}

/// Outcome of the release-type preference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TypeScore {
    /// A discouraged type: the release is only chosen when nothing else is left.
    Skip,
    Score(f32),
}

/// Picard's `_weights_from_preferred_release_types`: every type scores 0.5, plus up to 0.5 by
/// position in `preferred`; the scores are averaged. Any discouraged type skips the release.
/// Without type information the score is neutral. Names compare case-insensitively.
pub(crate) fn release_type_score(
    types: &[String],
    preferred: &[String],
    discouraged: &[String],
) -> TypeScore {
    if types.is_empty() {
        return TypeScore::Score(0.5);
    }
    let position = |list: &[String], t: &str| list.iter().position(|p| p.eq_ignore_ascii_case(t));
    let mut score = 0.0;
    for t in types {
        if position(discouraged, t).is_some() {
            return TypeScore::Skip;
        }
        score += match position(preferred, t) {
            Some(i) => 0.5 + 0.5 * (preferred.len() - i) as f32 / preferred.len() as f32,
            None => 0.5,
        };
    }
    TypeScore::Score(score / types.len() as f32)
}

fn release_types(m: &Metadata) -> Vec<String> {
    m.get_all("releasetype").to_vec()
}

fn type_part(m: &Metadata, settings: &MetadataSettings, weight: f32) -> Part {
    match release_type_score(
        &release_types(m),
        &settings.preferred_types,
        &settings.discouraged_types,
    ) {
        TypeScore::Skip => SKIP_RELEASE,
        TypeScore::Score(s) => (s, weight),
    }
}

/// Country, format and release-type preference parts of a candidate's metadata.
fn preference_parts(c: &Metadata, settings: &MetadataSettings, w: &Weights) -> Vec<Part> {
    let mut parts = Vec::new();
    if w.country > 0.0
        && let Some(s) = list_preference(&settings.preferred_countries, c.get("releasecountry"))
    {
        parts.push((s, w.country));
    }
    if w.format > 0.0 {
        let formats = medium_formats(c);
        if let Some(s) = format_preference(&settings.preferred_formats, &formats) {
            parts.push((s, w.format));
        }
    }
    if w.release_type > 0.0 {
        parts.push(type_part(c, settings, w.release_type));
    }
    parts
}

/// Formats of a candidate's media: `~mediaformats` (one per medium) when known, else `media`.
fn medium_formats(c: &Metadata) -> Vec<String> {
    let per_medium = c.get_all("~mediaformats");
    if per_medium.is_empty() {
        c.get_all("media").to_vec()
    } else {
        per_medium.to_vec()
    }
}

/// Country and format preference of a release (0..1), for callers that only have a few tags.
pub(crate) fn preference_score(m: &Metadata, settings: &MetadataSettings) -> Option<f32> {
    let parts: Vec<Part> = [
        list_preference(&settings.preferred_countries, m.get("releasecountry")),
        format_preference(&settings.preferred_formats, &medium_formats(m)),
    ]
    .into_iter()
    .flatten()
    .map(|s| (s, 1.0))
    .collect();
    linear(&parts)
}

/// Release-type preference of a release, 0..1; `None` for a discouraged type.
pub(crate) fn type_preference(m: &Metadata, settings: &MetadataSettings) -> Option<f32> {
    match release_type_score(
        &release_types(m),
        &settings.preferred_types,
        &settings.discouraged_types,
    ) {
        TypeScore::Skip => None,
        TypeScore::Score(s) => Some(s),
    }
}

// ---- release comparison -----------------------------------------------------------------------

/// Names that denote "Various Artists" in any language (Picard `_VARIOUS_ARTISTS_NAMES`).
const VARIOUS_ARTISTS_NAMES: &[&str] = &[
    "various artists",
    "various artist",
    "various",
    "va",
    "v.a.",
    "v/a",
    "vv aa",
    "va.",
    "v. a.",
    "verschiedene interpreten",
    "verschiedene künstler",
    "verschiedene",
    "diverse interpreten",
    "diverse",
    "multi-interprètes",
    "artistes variés",
    "divers",
    "collectif",
    "varios artistas",
    "varios",
    "artisti vari",
    "vari",
    "aa.vv.",
    "vv.aa.",
    "vários artistas",
    "vários",
    "vários intérpretes",
    "coletânea",
    "diverse artiesten",
    "blandade artister",
    "diverse artister",
    "diverse artistar",
    "div. kunstnere",
    "eri esittäjiä",
    "różni wykonawcy",
    "różni artyści",
    "různí interpreti",
    "разные артисты",
    "различные исполнители",
    "різні виконавці",
    "розныя выканаўцы",
    "различни изпълнители",
    "разни изведувачи",
    "διάφοροι καλλιτέχνες",
    "çeşitli sanatçılar",
    "אמנים שונים",
    "مختلف الفنانين",
    "هنرمندان مختلف",
    "群星",
    "ヴァリアス・アーティスト",
    "オムニバス",
    "여러 아티스트",
    "รวมศิลปิน",
    "nhiều nghệ sĩ",
    "nhiều ca sĩ",
    "pluraj artistoj",
    "diversos artistes",
    "hainbat artista",
    "įvairūs atlikėjai",
    "erinevad esitajad",
];

/// Picard's `_is_various_artists`: an artist id is authoritative; without one the name is
/// compared against the known "Various Artists" names and the configured `va_name`.
fn is_various_artists(name: Option<&str>, id: Option<&str>, settings: &MetadataSettings) -> bool {
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        return id == crate::mb::VARIOUS_ARTISTS_ID;
    }
    let Some(name) = name else { return false };
    let name = name.trim().to_lowercase();
    VARIOUS_ARTISTS_NAMES.contains(&name.as_str())
        || (!settings.va_name.is_empty() && settings.va_name.trim().to_lowercase() == name)
}

/// Picard's `_compare_to_release_parts` for a release described by search-hit style metadata.
fn release_parts(
    file: &Metadata,
    c: &Metadata,
    w: &Weights,
    settings: &MetadataSettings,
    file_count: usize,
) -> Parts {
    let mut p = Parts::new();

    // Tier 1: identifiers.
    if w.barcode > 0.0
        && let (Some(a), Some(b)) = (
            file.get("barcode").filter(|b| !b.is_empty()),
            c.get("barcode"),
        )
    {
        p.identifiers
            .push((f32::from(barcodes_match(a, b)), w.barcode));
    }
    if w.catno > 0.0
        && let Some(catno) = file.get("catalognumber").filter(|c| !c.is_empty())
        && !c.get_all("catalognumber").is_empty()
    {
        let score = catno_label_score(
            catno,
            file.get("label").unwrap_or_default(),
            c.get_all("catalognumber"),
            c.get_all("label"),
        );
        p.identifiers.push((score, w.catno));
    }

    // Tier 2: similarity.
    let mut album_sim = None;
    let mut artist_sim = None;
    let mut trackcount_sim = None;
    if w.album > 0.0
        && let (Some(a), Some(b)) = (file.get("album"), c.get("album"))
    {
        let s = string_similarity(a, b);
        album_sim = Some(s);
        p.similarity.push((s, w.album));
    }
    if w.albumartist > 0.0
        && let (Some(a), Some(b)) = (file.get("albumartist"), c.get("albumartist"))
    {
        let s = string_similarity(a, b);
        artist_sim = Some(s);
        p.similarity.push((s, w.albumartist));
    }
    let candidate_total = c.get("totaltracks").and_then(leading_number);
    if w.total_tracks > 0.0
        && let (Some(a), Some(b)) = (
            file.get("totaltracks").and_then(leading_number),
            candidate_total,
        )
    {
        let s = trackcount_score(a, b);
        trackcount_sim = Some(s);
        p.similarity.push((s, w.total_tracks));
    }
    if w.total_album_tracks > 0.0 {
        let wanted = file
            .get("~totalalbumtracks")
            .or_else(|| file.get("totaltracks"))
            .and_then(leading_number)
            .filter(|n| *n > 0)
            .or((file_count > 0).then(|| u32::try_from(file_count).unwrap_or(u32::MAX)));
        if let (Some(a), Some(b)) = (wanted, candidate_total) {
            let s = trackcount_score(a, b);
            trackcount_sim = Some(s);
            p.similarity.push((s, w.total_album_tracks));
        }
    }
    if w.date > 0.0 {
        p.similarity
            .push((date_score(c.get("date"), file.get("date")), w.date));
    }

    // A matching title must not carry the match when the artist clearly contradicts.
    if let (Some(album), Some(artist)) = (album_sim, artist_sim)
        && album >= DISCRIMINATOR_TITLE_MATCH
        && artist < DISCRIMINATOR_MISMATCH
        && !is_various_artists(
            file.get("albumartist"),
            file.get("musicbrainz_albumartistid"),
            settings,
        )
    {
        let tracks_mismatch = trackcount_sim.is_some_and(|s| s < DISCRIMINATOR_MISMATCH);
        p.penalty = if tracks_mismatch {
            DISCRIMINATOR_BOTH_PENALTY
        } else {
            DISCRIMINATOR_ARTIST_PENALTY
        };
    }

    // Tier 3: preferences.
    p.preferences = preference_parts(c, settings, w);
    p
}

/// Score (0..1) of a release search hit against a cluster's metadata and file count: Picard's
/// `compare_to_release` with the cluster weights, times the search relevance.
pub fn release_score(
    cluster: &Metadata,
    file_count: usize,
    candidate: &ReleaseCandidate,
    settings: &MetadataSettings,
) -> f32 {
    let base = release_parts(
        cluster,
        &candidate.metadata,
        &CLUSTER_WEIGHTS,
        settings,
        file_count,
    )
    .combine();
    (base * f32::from(candidate.search_score.min(100)) / 100.0).clamp(0.0, 1.0)
}

// ---- recording comparison ----------------------------------------------------------------------

/// Parts of a recording candidate for one file (Picard's `compare_to_track`).
fn track_parts(file: &Metadata, c: &RecordingCandidate, settings: &MetadataSettings) -> Parts {
    let w = &FILE_WEIGHTS;
    let m = &c.metadata;
    let mut p = Parts::new();
    if !file.get_all("isrc").is_empty() {
        p.identifiers
            .push((isrcs_score(file.get_all("isrc"), m.get_all("isrc")), w.isrc));
    }
    if let (Some(a), Some(b)) = (file.get("title"), m.get("title")) {
        p.similarity.push((string_similarity(a, b), w.title));
    }
    if let (Some(a), Some(b)) = (file.get("artist"), m.get("artist")) {
        p.similarity.push((string_similarity(a, b), w.artist));
    }
    if let (Some(a), Some(b)) = (file.length(), m.length()) {
        let (a, b) = (a.as_millis() as u64, b.as_millis() as u64);
        if a > 0 {
            p.similarity.push((length_score(a, b), w.length));
        }
    }
    let video = |m: &Metadata| m.get("~video") == Some("1");
    p.preferences
        .push((f32::from(video(file) == video(m)), w.isvideo));

    if c.release_id.is_none() {
        // No release: neutral (0.5) for every release-level weight, so tracks with releases win.
        p.similarity.push((0.5, w.album + w.date + w.total_tracks));
        p.preferences
            .push((0.5, w.format + w.country + w.release_type));
        return p;
    }
    if file.get("tracknumber").is_some() {
        p.similarity
            .push((tracknumber_score(file, m), w.tracknumber));
    }
    p.merged(release_parts(file, m, w, settings, 0))
}

/// Best recording search hit for a single file: `(index, score)`; `None` below `threshold`.
/// `score` is Picard's `compare_to_track` similarity times the search relevance.
pub fn best_recording(
    file: &Metadata,
    candidates: &[RecordingCandidate],
    settings: &MetadataSettings,
    threshold: f32,
) -> Option<(usize, f32)> {
    candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let score = track_parts(file, c, settings).combine()
                * f32::from(c.search_score.min(100))
                / 100.0;
            (i, score.clamp(0.0, 1.0))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, s)| *s >= threshold)
}

// ---- margin ------------------------------------------------------------------------------------------

/// Why a best match was returned (Picard's `MatchResult.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchReason {
    /// The best score is clearly ahead of the runner-up.
    Confident,
    /// The runner-up is closer than the minimum margin; the best match is still returned.
    Ambiguous,
}

/// Picard's `find_best_match_with_margin`: the index of the best score, `None` when it is below
/// `min_similarity`; flagged ambiguous when the runner-up trails by less than `min_margin`
/// (never with a single candidate).
pub fn best_match_with_margin(
    scores: &[f32],
    min_similarity: f32,
    min_margin: f32,
) -> Option<(usize, MatchReason)> {
    let mut best: Option<(usize, f32)> = None;
    let mut second = -1.0_f32;
    for (i, &s) in scores.iter().enumerate() {
        match best {
            Some((_, b)) if s <= b => second = second.max(s),
            _ => {
                if let Some((_, b)) = best {
                    second = b;
                }
                best = Some((i, s));
            }
        }
    }
    let (index, score) = best.filter(|(_, s)| *s >= min_similarity)?;
    let reason = if second >= 0.0 && score - second < min_margin {
        MatchReason::Ambiguous
    } else {
        MatchReason::Confident
    };
    Some((index, reason))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::release::{ReleaseId, Source};

    fn md(pairs: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::new();
        for (k, v) in pairs {
            m.set(k, *v);
        }
        m
    }

    fn cand(score: u8, pairs: &[(&str, &str)]) -> ReleaseCandidate {
        ReleaseCandidate {
            id: ReleaseId {
                source: Source::MusicBrainz,
                id: "x".into(),
            },
            search_score: score,
            metadata: md(pairs),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn date_and_trackcount_scores_follow_picard() {
        assert_eq!(date_score(None, Some("1999")), 0.25);
        assert_eq!(date_score(Some("1999"), None), 0.65);
        assert_eq!(date_score(Some("1999-01-02"), Some("1999-01-02")), 1.0);
        assert_eq!(date_score(Some("1999-01-02"), Some("1999-05")), 0.95);
        assert_eq!(date_score(Some("1999"), Some("2001")), 0.85);
        assert_eq!(date_score(Some("1999"), Some("2003")), 0.0);
        assert_eq!(trackcount_score(10, 10), 1.0);
        assert!(
            (trackcount_score(8, 10) - 0.6).abs() < 1e-6,
            "fewer tracks: ratio * 2"
        );
        assert!(
            (trackcount_score(12, 10) - 0.4).abs() < 1e-6,
            "more tracks: ratio * 3"
        );
        assert_eq!(trackcount_score(5, 0), 0.0);
        assert!(barcodes_match("727361379704", "0727361379704"));
        assert!(!barcodes_match("727361379704", ""));
    }

    #[test]
    fn catno_scoring_handles_labels_near_misses_and_placeholders() {
        let catnos = strings(&["CDP 7243 8 55229 2 5"]);
        let labels = strings(&["Capitol"]);
        assert_eq!(
            catno_label_score("cdp 7243 8 55229 2 5", "capitol", &catnos, &labels),
            1.0
        );
        assert_eq!(
            catno_label_score("CDP 7243 8 55229 2 5", "Other", &catnos, &labels),
            0.8
        );
        let near = catno_label_score("CDP 7243 8 55229 2 6", "", &catnos, &labels);
        assert!(near > 0.6 && near <= 0.75, "{near}");
        assert_eq!(catno_label_score("ZZZ", "", &catnos, &labels), 0.0);
        assert_eq!(catno_label_score("[none]", "", &catnos, &labels), 0.5);
        assert_eq!(
            catno_label_score("X", "", &strings(&["[none]"]), &labels),
            0.5
        );
    }

    #[test]
    fn release_type_preferences_rank_and_discourage() {
        let preferred = strings(&["Album", "Single"]);
        let discouraged = strings(&["Compilation", "DJ-mix"]);
        let score = |types: &[&str]| release_type_score(&strings(types), &preferred, &discouraged);
        assert_eq!(
            score(&["album"]),
            TypeScore::Score(1.0),
            "first preferred gets 1.0"
        );
        assert_eq!(score(&["single"]), TypeScore::Score(0.75));
        assert_eq!(score(&["ep"]), TypeScore::Score(0.5), "unlisted is neutral");
        assert_eq!(
            score(&["album", "live"]),
            TypeScore::Score(0.75),
            "scores are averaged"
        );
        assert_eq!(score(&["album", "compilation"]), TypeScore::Skip);
        assert_eq!(score(&[]), TypeScore::Score(0.5));
    }

    #[test]
    fn preferred_types_decide_between_otherwise_equal_releases() {
        let cluster = md(&[("album", "A"), ("albumartist", "X")]);
        let s = MetadataSettings {
            preferred_types: strings(&["Album", "EP", "Single"]),
            discouraged_types: strings(&["Compilation"]),
            ..MetadataSettings::default()
        };
        let base = [("album", "A"), ("albumartist", "X")];
        let make = |types: &[&str]| {
            let mut c = cand(100, &base);
            c.metadata.set_all("releasetype", strings(types));
            c
        };
        let album = release_score(&cluster, 0, &make(&["album"]), &s);
        let ep = release_score(&cluster, 0, &make(&["ep"]), &s);
        let live = release_score(&cluster, 0, &make(&["album", "live"]), &s);
        let comp = release_score(&cluster, 0, &make(&["album", "compilation"]), &s);
        assert!(
            album > ep && ep > live && live > comp,
            "{album} {ep} {live} {comp}"
        );
    }

    #[test]
    fn country_and_format_preferences_use_their_list_position() {
        let cluster = md(&[("album", "A")]);
        let s = MetadataSettings {
            preferred_countries: strings(&["IT", "GB"]),
            preferred_formats: strings(&["CD", "Digital Media"]),
            ..MetadataSettings::default()
        };
        let it = cand(
            80,
            &[("album", "A"), ("releasecountry", "IT"), ("media", "CD")],
        );
        let gb = cand(
            80,
            &[("album", "A"), ("releasecountry", "GB"), ("media", "CD")],
        );
        let us = cand(
            80,
            &[("album", "A"), ("releasecountry", "US"), ("media", "Vinyl")],
        );
        let (a, b, c) = (
            release_score(&cluster, 0, &it, &s),
            release_score(&cluster, 0, &gb, &s),
            release_score(&cluster, 0, &us, &s),
        );
        assert!(a > b && b > c, "{a} {b} {c}");
        // Per-medium formats are averaged.
        let mut mixed = md(&[("album", "A")]);
        mixed.set_all("~mediaformats", strings(&["CD", "Vinyl"]));
        assert_eq!(
            format_preference(&s.preferred_formats, &medium_formats(&mixed)),
            Some(0.5)
        );
        let hi = cand(100, &[("album", "A")]);
        let lo = cand(20, &[("album", "A")]);
        assert!(release_score(&cluster, 0, &hi, &s) > release_score(&cluster, 0, &lo, &s));
    }

    #[test]
    fn identifiers_dominate_and_mismatches_cap_the_score() {
        let cluster = md(&[
            ("album", "Abbey Road"),
            ("albumartist", "The Beatles"),
            ("barcode", "0123456789012"),
        ]);
        let s = MetadataSettings::default();
        let same = cand(
            100,
            &[
                ("album", "Abbey Road"),
                ("albumartist", "The Beatles"),
                ("barcode", "123456789012"),
            ],
        );
        let other = cand(
            100,
            &[
                ("album", "Abbey Road"),
                ("albumartist", "The Beatles"),
                ("barcode", "999"),
            ],
        );
        let none = cand(
            100,
            &[("album", "Abbey Road"), ("albumartist", "The Beatles")],
        );
        let (a, b, c) = (
            release_score(&cluster, 0, &same, &s),
            release_score(&cluster, 0, &other, &s),
            release_score(&cluster, 0, &none, &s),
        );
        assert!(a > 0.85, "{a}");
        assert!(b <= 0.3, "{b}");
        assert!(a > c && c > b, "{a} {c} {b}");
    }

    #[test]
    fn matching_title_with_contradicting_artist_is_penalised_except_for_various_artists() {
        let s = MetadataSettings::default();
        let release = cand(
            100,
            &[
                ("album", "Greatest Hits"),
                ("albumartist", "Queen"),
                ("totaltracks", "17"),
            ],
        );
        let mine = md(&[
            ("album", "Greatest Hits"),
            ("albumartist", "Quincy Jones Trio"),
        ]);
        let va = md(&[
            ("album", "Greatest Hits"),
            ("albumartist", "Various Artists"),
        ]);
        let honest = md(&[("album", "Greatest Hits"), ("albumartist", "Queen")]);
        let penal = release_score(&mine, 17, &release, &s);
        assert!(
            penal < release_score(&honest, 17, &release, &s) * 0.8,
            "{penal}"
        );
        assert!(release_score(&va, 17, &release, &s) > penal);
        assert!(is_various_artists(
            Some("Verschiedene Interpreten"),
            None,
            &s
        ));
        assert!(!is_various_artists(
            Some("Various Artists"),
            Some("a74b1b7f-71a5-4011-9441-d0b5e4122711"),
            &s
        ));
    }

    #[test]
    fn release_score_prefers_matching_album_and_track_count() {
        let cluster = md(&[("album", "Abbey Road"), ("albumartist", "The Beatles")]);
        let s = MetadataSettings::default();
        let mk = |album: &str, n: &str| {
            cand(
                100,
                &[
                    ("album", album),
                    ("albumartist", "The Beatles"),
                    ("totaltracks", n),
                ],
            )
        };
        let (g, m, f, o) = (
            release_score(&cluster, 17, &mk("Abbey Road", "17"), &s),
            release_score(&cluster, 17, &mk("Abbey Road", "20"), &s),
            release_score(&cluster, 17, &mk("Abbey Road", "10"), &s),
            release_score(&cluster, 17, &mk("Revolver", "17"), &s),
        );
        assert!(g > 0.85, "{g}");
        assert!(g > m && m > f, "{g} {m} {f}");
        assert!(o < 0.7, "{o}");
    }

    #[test]
    fn best_recording_picks_title_and_length_match() {
        let mut file = md(&[("title", "Midnight Rain"), ("artist", "Band")]);
        file.set_length(Duration::from_secs(245));
        let mk = |title: &str, secs: u64, score: u8| {
            let mut m = md(&[("title", title), ("artist", "Band")]);
            m.set_length(Duration::from_secs(secs));
            RecordingCandidate {
                recording_id: title.into(),
                release_id: None,
                search_score: score,
                metadata: m,
            }
        };
        let c = [
            mk("Midnight Rain (Live)", 400, 100),
            mk("Midnight Rain", 246, 90),
            mk("Other", 245, 100),
        ];
        let s = MetadataSettings::default();
        let (i, _) = best_recording(&file, &c, &s, 0.4).expect("match");
        assert_eq!(i, 1);
        assert!(best_recording(&md(&[("title", "Unrelated Words")]), &c[2..], &s, 0.9).is_none());
    }

    #[test]
    fn recording_on_a_matching_release_beats_a_standalone_one_and_isrc_decides() {
        let mut file = md(&[
            ("title", "Song"),
            ("artist", "Band"),
            ("album", "Real"),
            ("tracknumber", "3"),
            ("isrc", "GB1"),
        ]);
        file.set_length(Duration::from_secs(200));
        let mk = |release: Option<&str>, number: &str, isrc: &str| {
            let mut m = md(&[
                ("title", "Song"),
                ("artist", "Band"),
                ("album", "Real"),
                ("tracknumber", number),
            ]);
            m.set_length(Duration::from_secs(200));
            if !isrc.is_empty() {
                m.set("isrc", isrc);
            }
            RecordingCandidate {
                recording_id: "r".into(),
                release_id: release.map(str::to_owned),
                search_score: 100,
                metadata: m,
            }
        };
        let s = MetadataSettings::default();
        let c = [
            mk(None, "3", ""),
            mk(Some("rel"), "9", "GB2"),
            mk(Some("rel"), "3", "GB1"),
        ];
        let (i, score) = best_recording(&file, &c, &s, 0.0).expect("match");
        assert_eq!(i, 2, "same ISRC, same track number");
        assert!(score > 0.9, "{score}");
    }

    #[test]
    fn margin_flags_ambiguous_and_rejects_below_floor() {
        assert_eq!(
            best_match_with_margin(&[0.9, 0.5], 0.25, 0.02),
            Some((0, MatchReason::Confident))
        );
        assert_eq!(
            best_match_with_margin(&[0.5, 0.51], 0.25, 0.02),
            Some((1, MatchReason::Ambiguous))
        );
        assert_eq!(best_match_with_margin(&[0.2, 0.1], 0.25, 0.02), None);
        assert_eq!(
            best_match_with_margin(&[0.6], 0.25, 0.5),
            Some((0, MatchReason::Confident)),
            "single candidate"
        );
        assert_eq!(best_match_with_margin(&[], 0.0, 0.0), None);
        assert_eq!(
            best_match_with_margin(&[0.7, 0.7], 0.0, 0.02),
            Some((0, MatchReason::Ambiguous)),
            "ties are ambiguous"
        );
    }
}
