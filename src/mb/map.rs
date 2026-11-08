//! MusicBrainz JSON → Picard metadata. A port of Picard's `picard/mbjson.py`
//! (`release_to_metadata`, `release_group_to_metadata`, `medium_to_metadata`,
//! `track_to_metadata`, `recording_to_metadata`) plus the track assembly done in
//! `Album._load_tracks` and `Track._customize_metadata`.
//!
//! Artist names, aliases and translation are in [`super::artist`], relationships in
//! [`super::relations`], genres in [`super::folksonomy`].
//!
//! Deviations from Picard: `tracknumber` is the track's position on its medium; `~length` is in
//! milliseconds (Picard stores `m:ss`); a release's `text-representation` language only ends up
//! in `~releaselanguage`, as in Picard; track metadata carries no album `~length`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde_json::Value;

use super::artist::{VARIOUS_ARTISTS_ID, artist_credit_to_metadata, translate_title};
use super::folksonomy::{AlbumVotes, TrackVotes, Votes, apply_genres};
use super::relations::{Entity, relations_to_metadata};
use super::text::asciipunct;
use super::{RecordingCandidate, add_unique, array, str_field};
use crate::metadata::{LENGTH, Metadata};
use crate::release::{Artwork, Release, ReleaseCandidate, ReleaseId, Source, Track};
use crate::settings::{MetadataSettings, StandardizeArtistNames};
use crate::{Error, Result};

/// Placeholder track titles MusicBrainz uses (Picard `DATA_TRACK_TITLE`, `SILENCE_TRACK_TITLE`).
const DATA_TRACK_TITLE: &str = "[data track]";
const SILENCE_TRACK_TITLE: &str = "[silence]";

/// Search-server `score`: an integer in current responses, a string in older ones.
fn search_score(hit: &Value) -> u8 {
    let score = &hit["score"];
    let raw = score
        .as_u64()
        .or_else(|| score.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    u8::try_from(raw.min(100)).unwrap_or(100)
}

/// Settings for mapping search hits: they have no aliases or user settings to apply, so credited
/// names are used.
fn search_settings() -> MetadataSettings {
    MetadataSettings {
        standardize_artist_names: StandardizeArtistNames::None,
        ..MetadataSettings::default()
    }
}

fn is_various_artists(m: &Metadata, key: &str) -> bool {
    m.get_all(key) == [VARIOUS_ARTISTS_ID]
}

/// Picard's `Metadata.apply_func(asciipunct)`.
fn convert_punctuation(m: &mut Metadata) {
    let converted: Vec<(String, Vec<String>)> = m
        .iter()
        .filter(|(key, _)| *key != LENGTH)
        .map(|(key, values)| {
            (
                key.to_owned(),
                values.iter().map(|v| asciipunct(v)).collect::<Vec<_>>(),
            )
        })
        .filter(|(key, values)| m.get_all(key) != values.as_slice())
        .collect();
    for (key, values) in converted {
        m.set_all(&key, values);
    }
}

// ---- artist node sharing ------------------------------------------------------------------

/// Artist nodes by id, from the release's own artist credit: they carry the aliases, genres and
/// tags that track-level credits lack (Picard `_copy_artist_nodes`).
type ArtistNodes = HashMap<String, Value>;

fn artist_nodes(credits: &[Value]) -> ArtistNodes {
    credits
        .iter()
        .filter_map(|c| {
            Some((
                str_field(&c["artist"], "id")?.to_owned(),
                c["artist"].clone(),
            ))
        })
        .collect()
}

/// `credits` with every artist replaced by the richer node of `source` where ids match.
fn with_artist_nodes(credits: &[Value], source: &ArtistNodes) -> Vec<Value> {
    credits
        .iter()
        .map(
            |credit| match str_field(&credit["artist"], "id").and_then(|id| source.get(id)) {
                Some(node) => {
                    let mut credit = credit.clone();
                    credit["artist"] = node.clone();
                    credit
                }
                None => credit.clone(),
            },
        )
        .collect()
}

// ---- entity mappings ----------------------------------------------------------------------

/// Picard's `release_group_to_metadata`.
fn release_group_to_metadata(
    rg: &Value,
    m: &mut Metadata,
    settings: &MetadataSettings,
    votes: &mut Votes,
) {
    if !rg.is_object() {
        return;
    }
    if let Some(id) = str_field(rg, "id") {
        add_unique(m, "musicbrainz_releasegroupid", id);
    }
    for (key, tag) in [
        ("disambiguation", "~releasegroupcomment"),
        ("first-release-date", "~releasegroup_firstreleasedate"),
        ("title", "~releasegroup"),
    ] {
        if let Some(value) = str_field(rg, key) {
            m.set(tag, value);
        }
    }
    if let Some(primary) = str_field(rg, "primary-type") {
        m.set("~primaryreleasetype", primary.to_lowercase());
    }
    for secondary in array(rg, "secondary-types")
        .iter()
        .filter_map(Value::as_str)
    {
        add_unique(m, "~secondaryreleasetype", &secondary.to_lowercase());
    }
    if settings.release_ars {
        relations_to_metadata(
            array(rg, "relations"),
            m,
            false,
            Entity::ReleaseGroup,
            settings,
        );
    }
    votes.add_from_node(rg);
    if let Some(date) = m.get("~releasegroup_firstreleasedate").map(str::to_owned) {
        m.set("originalyear", date.chars().take(4).collect::<String>());
        m.set("originaldate", date);
    }
    let types: Vec<String> = m
        .get_all("~primaryreleasetype")
        .iter()
        .chain(m.get_all("~secondaryreleasetype"))
        .cloned()
        .collect();
    m.set_all("releasetype", types);
}

/// Picard's `release_to_metadata`. Also accepts search-result release hits. Returns the
/// medium-scoped DJ mixers found in the release relationships.
fn release_to_metadata(
    node: &Value,
    m: &mut Metadata,
    settings: &MetadataSettings,
    votes: Option<&mut AlbumVotes>,
) -> BTreeMap<u32, Vec<String>> {
    if let Some(id) = str_field(node, "id") {
        add_unique(m, "musicbrainz_albumid", id);
    }
    for (key, tag) in [
        ("annotation", "~releaseannotation"),
        ("asin", "asin"),
        ("barcode", "barcode"),
        ("country", "releasecountry"),
        ("date", "date"),
        ("disambiguation", "~releasecomment"),
        ("title", "album"),
    ] {
        if let Some(value) = str_field(node, key) {
            m.set(tag, value);
        }
    }
    if let Some(status) = str_field(node, "status") {
        m.set("releasestatus", status.to_lowercase());
    }
    let mut per_medium = BTreeMap::new();
    let mut album_votes = votes;
    if let Some(credits) = node["artist-credit"].as_array() {
        artist_credit_to_metadata(credits, m, true, settings);
        if let Some(votes) = album_votes.as_deref_mut() {
            votes
                .artists
                .extend(credits.iter().map(|c| Votes::of(&c["artist"])));
        }
    }
    if settings.release_ars {
        per_medium = relations_to_metadata(
            array(node, "relations"),
            m,
            false,
            Entity::Release,
            settings,
        );
    }
    let mut labels: Vec<&str> = Vec::new();
    let mut catalog_numbers: Vec<&str> = Vec::new();
    for label_info in array(node, "label-info") {
        if let Some(label) = str_field(&label_info["label"], "name")
            && !labels.contains(&label)
        {
            labels.push(label);
        }
        if let Some(catalog_number) = str_field(label_info, "catalog-number")
            && !catalog_numbers.contains(&catalog_number)
        {
            catalog_numbers.push(catalog_number);
        }
    }
    m.set_all("label", labels.into_iter().map(str::to_owned));
    m.set_all(
        "catalognumber",
        catalog_numbers.into_iter().map(str::to_owned),
    );
    let text = &node["text-representation"];
    if let Some(language) = str_field(text, "language") {
        m.set("~releaselanguage", language);
    }
    if let Some(script) = str_field(text, "script") {
        m.set("script", script);
    }

    let countries = release_countries(node);
    m.set_all("~releasecountries", countries.clone());
    // The web service puts the first release country in `country`; prefer the user's choice
    // if the release was issued there.
    if let Some(country) = settings
        .preferred_countries
        .iter()
        .find(|c| countries.contains(c))
    {
        m.set("releasecountry", country.as_str());
    }
    if settings.translate_album_titles
        && let Some(title) =
            translate_title(str_field(node, "title").unwrap_or_default(), node, settings)
    {
        m.set("album", title);
    }
    if let Some(votes) = album_votes {
        votes.release.add_from_node(node);
    }
    per_medium
}

/// Picard's `countries_from_node`: the ISO code of every release event's area, sorted.
pub(super) fn release_countries(node: &Value) -> Vec<String> {
    let mut countries: Vec<String> = array(node, "release-events")
        .iter()
        .filter_map(|e| e["area"]["iso-3166-1-codes"][0].as_str().map(str::to_owned))
        .collect();
    countries.sort();
    countries
}

/// Disc number of a medium: its `position`, or its 1-based index if missing.
fn medium_position(medium: &Value, index: usize) -> u32 {
    medium["position"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| u32::try_from(index + 1).unwrap_or(u32::MAX))
}

/// Picard's `medium_to_metadata`: `media`, `discnumber`, `discsubtitle`, `totaltracks` (the
/// medium's track count, plus one for a hidden pregap track).
fn medium_to_metadata(medium: &Value, m: &mut Metadata, index: usize) {
    if let Some(format) = str_field(medium, "format") {
        m.set("media", format);
    }
    m.set("discnumber", medium_position(medium, index).to_string());
    if let Some(title) = str_field(medium, "title") {
        m.set("discsubtitle", title);
    }
    let pregap = u64::from(medium.get("pregap").is_some_and(Value::is_object));
    let count = medium["track-count"]
        .as_u64()
        .unwrap_or_else(|| array(medium, "tracks").len() as u64);
    if count + pregap > 0 {
        m.set("totaltracks", (count + pregap).to_string());
    }
}

/// Picard's `recording_to_metadata`. `track_title` (the release-specific track title) wins over
/// the recording title. `credits` replaces the recording's own `artist-credit` (see
/// [`with_artist_nodes`]).
pub(super) fn recording_to_metadata(
    rec: &Value,
    m: &mut Metadata,
    track_title: Option<&str>,
    credits: Option<&[Value]>,
    settings: &MetadataSettings,
    votes: &mut TrackVotes,
) {
    if let Some(id) = str_field(rec, "id") {
        m.set("musicbrainz_recordingid", id);
    }
    for (key, tag) in [
        ("disambiguation", "~recordingcomment"),
        ("first-release-date", "~recording_firstreleasedate"),
    ] {
        if let Some(value) = str_field(rec, key) {
            m.set(tag, value);
        }
    }
    if let Some(rating) = rec["user-rating"]["value"].as_f64() {
        m.set("~rating", format!("{rating}"));
    }
    if let Some(ms) = rec["length"].as_u64().filter(|&ms| ms > 0) {
        m.set_length(Duration::from_millis(ms));
    }
    let own_credits = rec["artist-credit"].as_array().map(Vec::as_slice);
    if let Some(credits) = credits.or(own_credits).filter(|c| !c.is_empty()) {
        artist_credit_to_metadata(credits, m, false, settings);
        votes
            .artists
            .extend(credits.iter().map(|c| Votes::of(&c["artist"])));
    }
    relations_to_metadata(
        array(rec, "relations"),
        m,
        false,
        Entity::Recording,
        settings,
    );
    for isrc in array(rec, "isrcs") {
        // Lookup responses list plain strings, search responses `{"id": ...}` objects.
        if let Some(isrc) = isrc.as_str().or_else(|| isrc["id"].as_str()) {
            add_unique(m, "isrc", isrc);
        }
    }
    if rec["video"].as_bool().unwrap_or(false) {
        m.set("~video", "1");
    }
    votes.recording.add_from_node(rec);

    let mut recording_title = str_field(rec, "title").map(str::to_owned);
    let mut track_title = track_title.map(str::to_owned);
    if settings.translate_track_titles
        && let Some(alias) = super::artist::find_localized_alias(
            array(rec, "aliases"),
            &settings.translation_locales,
        )
    {
        if let Some(title) = recording_title.as_deref()
            && !super::artist::skip_translation_due_to_scripts(title, settings)
        {
            recording_title = Some(alias.name.clone());
        }
        if let Some(title) = track_title.as_deref()
            && !super::artist::skip_translation_due_to_scripts(title, settings)
        {
            track_title = Some(alias.name);
        }
    }
    if let Some(title) = track_title.as_deref().or(recording_title.as_deref()) {
        m.set("title", title);
    }
    if let Some(title) = recording_title {
        m.set("~recordingtitle", title);
    }
}

/// Picard's `Track._customize_metadata`: Various Artists name, placeholder titles, genres and
/// punctuation.
fn customize_track(
    m: &mut Metadata,
    track_votes: &TrackVotes,
    album_votes: &AlbumVotes,
    album_is_va: bool,
    settings: &MetadataSettings,
) {
    if is_various_artists(m, "musicbrainz_artistid") {
        m.set("artist", settings.va_name.clone());
        m.set("artistsort", settings.va_name.clone());
    }
    match m.get("title") {
        Some(DATA_TRACK_TITLE) => m.set("~datatrack", "1"),
        Some(SILENCE_TRACK_TITLE) => m.set("~silence", "1"),
        _ => {}
    }
    apply_genres(m, track_votes, album_votes, album_is_va, settings);
    if settings.convert_punctuation {
        convert_punctuation(m);
    }
}

/// Picard's `track_to_metadata` plus `_customize_metadata`: recording data first, then what the
/// release says about this particular track. Returns the release track id.
fn track_to_metadata(
    node: &Value,
    m: &mut Metadata,
    release_artists: &ArtistNodes,
    album_votes: &AlbumVotes,
    album_is_va: bool,
    settings: &MetadataSettings,
) -> Result<String> {
    let rec = node.get("recording").filter(|r| r.is_object()).ok_or_else(|| {
        Error::Response("MusicBrainz track without recording (was the release fetched with inc=recordings?)".into())
    })?;
    // Track credits and recording credits both take their artist nodes from the release; the
    // recording's also from the track's.
    let track_credits = with_artist_nodes(array(node, "artist-credit"), release_artists);
    let recording_credits = with_artist_nodes(
        &with_artist_nodes(array(rec, "artist-credit"), release_artists),
        &artist_nodes(&track_credits),
    );
    let mut votes = TrackVotes::default();
    recording_to_metadata(
        rec,
        m,
        str_field(node, "title"),
        Some(&recording_credits),
        settings,
        &mut votes,
    );

    let track_id = str_field(node, "id")
        .or_else(|| str_field(rec, "id"))
        .ok_or_else(|| Error::Response("MusicBrainz track without id".into()))?;
    add_unique(m, "musicbrainz_trackid", track_id);
    if let Some(number) = str_field(node, "number") {
        m.set("~musicbrainz_tracknumber", number);
    }
    if let Some(position) = node["position"].as_u64() {
        m.set("tracknumber", position.to_string());
    }
    if let Some(ms) = node["length"].as_u64().filter(|&ms| ms > 0) {
        m.set_length(Duration::from_millis(ms));
    }
    if !track_credits.is_empty() {
        artist_credit_to_metadata(&track_credits, m, false, settings);
    }
    customize_track(m, &votes, album_votes, album_is_va, settings);
    Ok(track_id.to_owned())
}

/// Picard's rule (`Album._load_tracks`): when the release has more than one distinct track artist
/// credit, every track gets `~multiartist = 1`, so file names stay consistent across the album.
fn mark_multiartist(tracks: &mut [Track]) {
    let mut credits: Vec<&str> = tracks
        .iter()
        .filter_map(|t| t.metadata.get("artist"))
        .collect();
    credits.sort_unstable();
    credits.dedup();
    if credits.len() > 1 {
        for track in tracks {
            track.metadata.set("~multiartist", "1");
        }
    }
}

// ---- public entry points (re-exported through `mb`) ---------------------------------------

/// See [`super::map_release`]. `discids` are the disc ids the album was looked up with; the
/// medium that has one of them gets `musicbrainz_discid`.
pub(super) fn release(
    json: &Value,
    settings: &MetadataSettings,
    discids: &[String],
) -> Result<Release> {
    let id = str_field(json, "id")
        .ok_or_else(|| Error::Response("MusicBrainz release without id".into()))?;

    // Like Picard: release-group tags first, release tags on top.
    let mut album = Metadata::new();
    let mut votes = AlbumVotes::default();
    let release_group = &json["release-group"];
    release_group_to_metadata(
        release_group,
        &mut album,
        settings,
        &mut votes.release_group,
    );
    let medium_mixers = release_to_metadata(json, &mut album, settings, Some(&mut votes));

    // `_release_metadata_customization`.
    let album_is_va = is_various_artists(&album, "musicbrainz_albumartistid");
    if album_is_va {
        album.set("albumartist", settings.va_name.clone());
        album.set("albumartistsort", settings.va_name.clone());
    }
    if settings.convert_punctuation {
        convert_punctuation(&mut album);
    }
    let media = array(json, "media");
    if !media.is_empty() {
        album.set("totaldiscs", media.len().to_string());
    }

    let release_artists = artist_nodes(array(json, "artist-credit"));
    let mut tracks = Vec::new();
    let mut formats: Vec<&str> = Vec::new();
    let mut all_discids: Vec<String> = Vec::new();
    let mut absolute = 0u32;
    for (index, medium) in media.iter().enumerate() {
        let mut medium_meta = album.clone();
        medium_to_metadata(medium, &mut medium_meta, index);
        if let Some(format) = str_field(medium, "format")
            && !formats.contains(&format)
        {
            formats.push(format);
        }
        let position = medium_position(medium, index);
        for name in medium_mixers.get(&position).into_iter().flatten() {
            add_unique(&mut medium_meta, "djmixer", name);
        }
        if album_is_va {
            medium_meta.set("compilation", "1");
        } else {
            medium_meta.remove("compilation");
        }
        if medium.get("discs").is_some_and(Value::is_array) {
            let medium_discids: Vec<String> = array(medium, "discs")
                .iter()
                .filter_map(|d| str_field(d, "id"))
                .map(str::to_owned)
                .collect();
            medium_meta.set_all(
                "musicbrainz_discid",
                discids
                    .iter()
                    .filter(|d| medium_discids.contains(d))
                    .cloned(),
            );
            all_discids.extend(medium_discids.iter().cloned());
            medium_meta.set_all("~musicbrainz_discids", medium_discids);
        }

        let pregap = medium.get("pregap").filter(|p| p.is_object());
        if pregap.is_some() {
            medium_meta.set("~discpregap", "1");
        }
        let mut push = |node: &Value, extra: &[(&str, &str)]| -> Result<()> {
            let mut metadata = medium_meta.clone();
            let track_id = track_to_metadata(
                node,
                &mut metadata,
                &release_artists,
                &votes,
                album_is_va,
                settings,
            )?;
            absolute += 1;
            metadata.set("~absolutetracknumber", absolute.to_string());
            for (key, value) in extra {
                metadata.set(key, *value);
            }
            tracks.push(Track {
                id: track_id,
                metadata,
            });
            Ok(())
        };
        if let Some(pregap) = pregap {
            push(pregap, &[("~pregap", "1")])?;
        }
        for node in array(medium, "tracks") {
            push(node, &[])?;
        }
        for node in array(medium, "data-tracks") {
            push(node, &[("~datatrack", "1")])?;
        }
    }
    // Album-level `media` lists each distinct format once (tracks keep their own medium's).
    album.set("media", formats.join(" / "));
    album.set("~totalalbumtracks", absolute.to_string());
    album.set_all("~musicbrainz_discids", all_discids);
    if album_is_va {
        album.set("compilation", "1");
    }
    for track in &mut tracks {
        track
            .metadata
            .set("~totalalbumtracks", absolute.to_string());
    }
    mark_multiartist(&mut tracks);

    Ok(Release {
        id: ReleaseId {
            source: Source::MusicBrainz,
            id: id.to_owned(),
        },
        metadata: album,
        tracks,
        artwork: Artwork::from_musicbrainz(json, str_field(release_group, "id").map(str::to_owned)),
    })
}

/// Maps a recording lookup into a standalone ("non-album") track, like Picard's
/// `NonAlbumTrack._parse_recording`: the album is `settings.nat_name` and `originaldate` is the
/// recording's first release date.
pub(super) fn standalone_recording(json: &Value, settings: &MetadataSettings) -> Result<Track> {
    let id = str_field(json, "id")
        .ok_or_else(|| Error::Response("MusicBrainz recording without id".into()))?;
    let mut m = Metadata::new();
    m.set("album", settings.nat_name.clone());
    let mut votes = TrackVotes::default();
    recording_to_metadata(json, &mut m, None, None, settings, &mut votes);
    customize_track(&mut m, &votes, &AlbumVotes::default(), false, settings);
    m.set("album", settings.nat_name.clone());
    if let Some(date) = m.get("~recording_firstreleasedate").map(str::to_owned) {
        m.set("originalyear", date.chars().take(4).collect::<String>());
        m.set("originaldate", date);
    }
    Ok(Track {
        id: id.to_owned(),
        metadata: m,
    })
}

/// "2×CD + DVD-Video" style summary (Picard's `media_formats_from_node`).
pub(super) fn media_formats(media: &[Value]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for medium in media {
        let format = str_field(medium, "format").unwrap_or("(unknown)");
        match counts.iter_mut().find(|(name, _)| *name == format) {
            Some((_, count)) => *count += 1,
            None => counts.push((format, 1)),
        }
    }
    let parts: Vec<String> = counts
        .iter()
        .map(|&(name, count)| {
            if count > 1 {
                format!("{count}×{name}")
            } else {
                name.to_owned()
            }
        })
        .collect();
    parts.join(" + ")
}

/// One `/release` search hit → candidate. Artist credits use credited names (search has no
/// access to the user's settings).
pub(super) fn release_candidate(hit: &Value) -> Option<ReleaseCandidate> {
    let id = str_field(hit, "id")?;
    let settings = search_settings();
    let mut m = Metadata::new();
    release_group_to_metadata(
        &hit["release-group"],
        &mut m,
        &settings,
        &mut Votes::default(),
    );
    release_to_metadata(hit, &mut m, &settings, None);
    let media = array(hit, "media");
    if !media.is_empty() {
        m.set("totaldiscs", media.len().to_string());
        m.set("media", media_formats(media));
        m.set_all(
            "~mediaformats",
            media
                .iter()
                .filter_map(|medium| str_field(medium, "format"))
                .map(str::to_owned),
        );
    }
    if let Some(count) = hit["track-count"].as_u64() {
        m.set("totaltracks", count.to_string());
    }
    Some(ReleaseCandidate {
        id: ReleaseId {
            source: Source::MusicBrainz,
            id: id.to_owned(),
        },
        search_score: search_score(hit),
        metadata: m,
    })
}

/// One `/recording` search hit → one candidate per release it appears on (or a single
/// release-less candidate).
pub(super) fn recording_candidates(hit: &Value) -> Vec<RecordingCandidate> {
    let Some(recording_id) = str_field(hit, "id") else {
        return Vec::new();
    };
    let search_score = search_score(hit);
    let settings = search_settings();
    let mut base = Metadata::new();
    recording_to_metadata(
        hit,
        &mut base,
        None,
        None,
        &settings,
        &mut TrackVotes::default(),
    );

    let releases = array(hit, "releases");
    if releases.is_empty() {
        return vec![RecordingCandidate {
            recording_id: recording_id.to_owned(),
            release_id: None,
            search_score,
            metadata: base,
        }];
    }
    releases
        .iter()
        .map(|release| {
            let mut m = base.clone();
            release_group_to_metadata(
                &release["release-group"],
                &mut m,
                &settings,
                &mut Votes::default(),
            );
            release_to_metadata(release, &mut m, &settings, None);
            // Search results only list the medium holding the recording, with `track-offset` its
            // 0-based index there.
            if let Some(medium) = array(release, "media").first() {
                medium_to_metadata(medium, &mut m, 0);
                let position = medium["track-offset"]
                    .as_u64()
                    .map(|offset| offset + 1)
                    .or_else(|| {
                        array(medium, "track")
                            .first()
                            .and_then(|t| str_field(t, "number"))
                            .and_then(|n| n.parse().ok())
                    });
                if let Some(position) = position {
                    m.set("tracknumber", position.to_string());
                }
            }
            RecordingCandidate {
                recording_id: recording_id.to_owned(),
                release_id: str_field(release, "id").map(str::to_owned),
                search_score,
                metadata: m,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
