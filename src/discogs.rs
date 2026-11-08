//! Discogs API client (`https://api.discogs.com`) and release → Picard metadata mapping.
//!
//! Requests send `Authorization: Discogs token=<token>` when a token is set
//! and go through `http::throttle("api.discogs.com", 1 s)` (2.5 s without a token: 25 req/min).
//! `/database/search` requires a token; release lookup does not.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::Value;

use crate::http;
use crate::metadata::Metadata;
use crate::release::{Artwork, Release, ReleaseCandidate, ReleaseId, ReleaseQuery, Source, Track};
use crate::settings::MetadataSettings;
use crate::{Error, Result};

pub const API_BASE: &str = "https://api.discogs.com";

const HOST: &str = "api.discogs.com";

/// `GET /database/search?type=release&release_title=&artist=` (`per_page = limit`). Without a
/// token returns `Error::Config("Discogs search needs a personal access token")`.
pub fn search_releases(
    query: &ReleaseQuery,
    limit: u32,
    token: Option<&str>,
) -> Result<Vec<ReleaseCandidate>> {
    let token = token
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Error::Config("Discogs search needs a personal access token".into()))?;

    let (album, artist) = (query.album.trim(), query.artist.trim());
    if album.is_empty() && artist.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = format!(
        "{API_BASE}/database/search?type=release&per_page={}",
        limit.clamp(1, 100)
    );
    for (key, value) in [("release_title", album), ("artist", artist)] {
        if !value.is_empty() {
            url.push_str(&format!("&{key}={}", http::encode_query(value)));
        }
    }

    let json = request(&url, Some(token))?;
    let results = json
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Response("Discogs search response has no `results`".into()))?;
    Ok(results
        .iter()
        .enumerate()
        .filter_map(|(rank, hit)| map_hit(hit, rank))
        .collect())
}

/// `GET /releases/{id}` mapped with [`map_release`].
pub fn lookup_release(
    id: &str,
    token: Option<&str>,
    settings: &MetadataSettings,
) -> Result<Release> {
    let id = id.trim();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Response(format!(
            "invalid Discogs release id `{id}`"
        )));
    }
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    let json = request(&format!("{API_BASE}/releases/{id}"), token)?;
    map_release(&json, settings)
}

fn request(url: &str, token: Option<&str>) -> Result<Value> {
    let interval = if token.is_some() {
        Duration::from_millis(1000)
    } else {
        Duration::from_millis(2500)
    };
    http::throttle(HOST, interval);
    match token {
        Some(token) => {
            let auth = format!("Discogs token={token}");
            http::get_json(url, &[("Authorization", auth.as_str())])
        }
        None => http::get_json(url, &[]),
    }
}

/// Maps one `/database/search` hit. `rank` is its 0-based position in the result list.
fn map_hit(hit: &Value, rank: usize) -> Option<ReleaseCandidate> {
    let id = json_id(hit.get("id")?)?;
    let mut md = Metadata::new();

    let title = str_of(hit, "title");
    // "Artist - Title"; the artist part can't contain " - " unambiguously, so split at the first.
    let (artist, album) = match title.split_once(" - ") {
        Some((artist, album)) => (clean_name(artist), album.trim().to_owned()),
        None => (String::new(), title.trim().to_owned()),
    };
    md.set("album", album);
    md.set("albumartist", normalize_various(artist));

    if let Some(date) = hit.get("year").and_then(|y| {
        y.as_str()
            .map(str::to_owned)
            .or_else(|| y.as_u64().map(|n| n.to_string()))
    }) {
        md.set("date", normalize_date(&date));
    }
    md.set("releasecountry", country_code(&str_of(hit, "country")));

    let formats = str_list(hit.get("format"));
    if let Some((name, descriptions)) = formats.split_first() {
        md.set("media", media_name(name, descriptions));
        md.set_all(
            "releasetype",
            release_types(descriptions.iter().map(String::as_str)),
        );
    }
    if let Some(label) = str_list(hit.get("label")).first() {
        md.set("label", clean_name(label));
    }
    md.set("catalognumber", catalog_number(&str_of(hit, "catno")));
    if let Some(barcode) = str_list(hit.get("barcode"))
        .iter()
        .find_map(|b| clean_barcode(b))
    {
        md.set("barcode", barcode);
    }
    md.set("discogs_albumid", id.clone());
    if let Some(master) = hit.get("master_id").and_then(json_id).filter(|m| m != "0") {
        md.set("discogs_masterid", master);
    }

    let search_score = 100u32.saturating_sub(3 * rank as u32).min(100) as u8;
    Some(ReleaseCandidate {
        id: ReleaseId {
            source: Source::Discogs,
            id,
        },
        search_score,
        metadata: md,
    })
}

/// Maps a `/releases/{id}` JSON. Pure; unit-testable. Artist names have Discogs disambiguation
/// suffixes (`Name (2)`) and `*` ANV markers removed; `artists[].join` builds the credit string
/// (ANV used unless `settings.standardize_artist_names` is `All`). Positions like `A1`, `B2`, `1-3`, `CD2-05`
/// map to discnumber/tracknumber (vinyl sides numbered sequentially on one disc); `index` tracks'
/// `sub_tracks` are flattened, `heading` tracks dropped. `duration` `m:ss`/`h:mm:ss` → `~length`.
/// Sets `artwork` to `Artwork::Discogs(images)`.
pub fn map_release(json: &Value, settings: &MetadataSettings) -> Result<Release> {
    let release_id = json
        .get("id")
        .and_then(json_id)
        .ok_or_else(|| Error::Response("Discogs release has no `id`".into()))?;
    let standardize =
        settings.standardize_artist_names == crate::settings::StandardizeArtistNames::All;

    let mut md = Metadata::new();
    md.set("album", str_of(json, "title").trim());

    let album_credit = Credit::from_artists(json.get("artists"), standardize);
    md.set("albumartist", album_credit.text.clone());
    let artists_sort = normalize_various(clean_name(&str_of(json, "artists_sort")));
    md.set("albumartistsort", artists_sort.clone());
    md.set_all("discogs_artistid", album_credit.ids.clone());

    let date = match str_of(json, "released").as_str() {
        "" => json
            .get("year")
            .and_then(Value::as_u64)
            .filter(|y| *y > 0)
            .map(|y| y.to_string())
            .unwrap_or_default(),
        released => released.to_owned(),
    };
    md.set("date", normalize_date(&date));
    md.set("releasecountry", country_code(&str_of(json, "country")));

    // Labels.
    let labels = json
        .get("labels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut label_names = Vec::new();
    let mut catnos = Vec::new();
    let mut label_ids = Vec::new();
    for label in labels {
        push_unique(&mut label_names, clean_name(&str_of(label, "name")));
        push_unique(&mut catnos, catalog_number(&str_of(label, "catno")));
        if let Some(id) = label.get("id").and_then(json_id) {
            push_unique(&mut label_ids, id);
        }
    }
    md.set_all("label", label_names);
    md.set_all("catalognumber", catnos);
    md.set_all("discogs_labelid", label_ids);

    // Formats.
    let formats = json
        .get("formats")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let primary = formats
        .iter()
        .find(|f| !matches!(str_of(f, "name").as_str(), "Box Set" | "All Media" | ""))
        .or_else(|| formats.first());
    if let Some(format) = primary {
        md.set(
            "media",
            media_name(
                &str_of(format, "name"),
                &str_list(format.get("descriptions")),
            ),
        );
    }
    let descriptions: Vec<String> = formats
        .iter()
        .flat_map(|f| str_list(f.get("descriptions")))
        .collect();
    md.set_all(
        "releasetype",
        release_types(descriptions.iter().map(String::as_str)),
    );

    // Genres and styles.
    let mut genres = Vec::new();
    for key in ["genres", "styles"] {
        for genre in str_list(json.get(key)) {
            push_unique(&mut genres, genre);
        }
    }
    md.set_all("genre", genres);

    if let Some(barcode) = json
        .get("identifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|i| str_of(i, "type").eq_ignore_ascii_case("barcode"))
        .find_map(|i| clean_barcode(&str_of(i, "value")))
    {
        md.set("barcode", barcode);
    }

    md.set("discogs_albumid", release_id.clone());
    if let Some(master) = json.get("master_id").and_then(json_id).filter(|m| m != "0") {
        md.set("discogs_masterid", master);
    }

    // Tracks.
    let release_extras = json
        .get("extraartists")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut entries = Vec::new();
    for item in json
        .get("tracklist")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        flatten_entry(item, "", &mut entries);
    }

    let mut staged = Vec::with_capacity(entries.len());
    let mut current_disc = 1u32;
    for entry in &entries {
        let raw_position = entry.position.clone();
        let parsed = parse_position(&raw_position);
        if let Some(position) = &parsed {
            if let Some(disc) = position.disc {
                current_disc = disc;
            } else if let Some(side) = position.side {
                current_disc = (u32::from(side) - u32::from(b'A')) / 2 + 1;
            }
        }
        staged.push((current_disc, raw_position, entry));
    }
    // Releases list positions in order already; the stable sort only guards against odd data.
    staged.sort_by_key(|(disc, ..)| *disc);

    let mut totals: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    for (disc, ..) in &staged {
        *totals.entry(*disc).or_default() += 1;
    }
    let total_discs = totals.len().max(1);
    // Discs are renumbered densely in case the data skips one (e.g. a lone `2-1` position).
    let disc_numbers: std::collections::BTreeMap<u32, usize> = totals
        .keys()
        .enumerate()
        .map(|(i, d)| (*d, i + 1))
        .collect();
    md.set("totaldiscs", total_discs.to_string());

    let mut seen_ids = HashSet::new();
    let mut counters: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    let mut tracks = Vec::with_capacity(staged.len());
    for (disc, raw_position, entry) in staged {
        let number = {
            let c = counters.entry(disc).or_default();
            *c += 1;
            *c
        };
        let mut tmd = md.clone();
        tmd.set("title", str_of(entry.item, "title").trim());
        tmd.set("tracknumber", number.to_string());
        tmd.set("totaltracks", totals[&disc].to_string());
        tmd.set("discnumber", disc_numbers[&disc].to_string());

        let override_credit = Credit::from_artists(entry.item.get("artists"), standardize);
        if override_credit.text.is_empty() {
            tmd.set("artist", album_credit.text.clone());
            tmd.set_all("artists", album_credit.names.clone());
            tmd.set("artistsort", artists_sort.clone());
        } else {
            tmd.set("artist", override_credit.text.clone());
            tmd.set_all("artists", override_credit.names.clone());
            tmd.set_all("discogs_artistid", override_credit.ids.clone());
        }

        if let Some(ms) = parse_duration_ms(&str_of(entry.item, "duration")) {
            tmd.set("~length", ms.to_string());
        }
        apply_credits(&mut tmd, entry.item, release_extras, &raw_position);

        let base_id = if raw_position.trim().is_empty() {
            format!("{release_id}:{disc}-{number}")
        } else {
            format!("{release_id}:{}", raw_position.trim())
        };
        let mut id = base_id.clone();
        let mut dup = 1;
        while !seen_ids.insert(id.clone()) {
            dup += 1;
            id = format!("{base_id}#{dup}");
        }
        tracks.push(Track { id, metadata: tmd });
    }

    // Picard rule: with more than one distinct track artist credit, every track is multi-artist.
    let distinct: HashSet<&str> = tracks
        .iter()
        .filter_map(|t| t.metadata.get("artist"))
        .collect();
    if distinct.len() > 1 {
        for track in &mut tracks {
            track.metadata.set("~multiartist", "1");
        }
    }

    let images = json
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|img| {
            let uri = img
                .get("uri")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())?;
            Some((uri.to_owned(), str_of(img, "type") == "primary"))
        })
        .collect::<Vec<_>>();
    let artwork = if images.is_empty() {
        Artwork::None
    } else {
        Artwork::Discogs(images)
    };

    Ok(Release {
        id: ReleaseId {
            source: Source::Discogs,
            id: release_id,
        },
        metadata: md,
        tracks,
        artwork,
    })
}

/// A tracklist leaf: an ordinary track, or a sub-track of an `index` entry.
struct Entry<'a> {
    item: &'a Value,
    /// Raw Discogs position (inherited from the `index` entry when the sub-track has none).
    position: String,
}

/// Drops `heading`s and replaces `index` entries by their `sub_tracks` (recursively). A sub-track
/// without its own position inherits the index entry's.
fn flatten_entry<'a>(item: &'a Value, inherited: &str, out: &mut Vec<Entry<'a>>) {
    let own = str_of(item, "position");
    let position = if own.trim().is_empty() {
        inherited.to_owned()
    } else {
        own
    };
    match str_of(item, "type_").as_str() {
        "heading" => {}
        "index" => {
            for sub in item
                .get("sub_tracks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                flatten_entry(sub, &position, out);
            }
        }
        _ => out.push(Entry { item, position }),
    }
}

/// Composer/lyricist from track-level `extraartists`, plus release-level ones whose `tracks`
/// field is empty (whole release) or lists this track's position.
fn apply_credits(md: &mut Metadata, item: &Value, release_extras: &[Value], position: &str) {
    let mut composers = Vec::new();
    let mut lyricists = Vec::new();
    let track_extras = item
        .get("extraartists")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let release_scoped = release_extras.iter().filter(|e| {
        let tracks = str_of(e, "tracks");
        let tracks = tracks.trim();
        tracks.is_empty()
            || (!position.is_empty() && tracks.split(',').any(|t| t.trim() == position.trim()))
    });
    for extra in track_extras.iter().chain(release_scoped) {
        let name = clean_name(&str_of(extra, "name"));
        for role in str_of(extra, "role").split(',') {
            // Roles may carry a bracketed qualifier: `Written-By [Lyrics]`.
            let role = role
                .split('[')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            match role.as_str() {
                "written-by" | "composed by" | "music by" => {
                    push_unique(&mut composers, name.clone())
                }
                "lyrics by" | "words by" => push_unique(&mut lyricists, name.clone()),
                _ => {}
            }
        }
    }
    md.set_all("composer", composers);
    md.set_all("lyricist", lyricists);
}

/// Artist credit built from a Discogs `artists` array.
struct Credit {
    text: String,
    names: Vec<String>,
    ids: Vec<String>,
}

impl Credit {
    fn from_artists(artists: Option<&Value>, standardize: bool) -> Self {
        let artists = artists
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut credit = Credit {
            text: String::new(),
            names: Vec::new(),
            ids: Vec::new(),
        };
        let mut pending_join = String::new();
        for artist in artists {
            let anv = str_of(artist, "anv");
            let raw = if standardize || anv.trim().is_empty() {
                str_of(artist, "name")
            } else {
                anv
            };
            let name = normalize_various(clean_name(&raw));
            if name.is_empty() {
                continue;
            }
            if !credit.text.is_empty() {
                credit.text.push_str(&pending_join);
            }
            credit.text.push_str(&name);
            credit.names.push(name);
            if let Some(id) = artist.get("id").and_then(json_id).filter(|i| i != "0") {
                push_unique(&mut credit.ids, id);
            }
            pending_join = match str_of(artist, "join").trim() {
                "" | "," => ", ".to_owned(),
                join => format!(" {join} "),
            };
        }
        credit
    }
}

/// Removes the trailing `*` ANV marker and the ` (N)` disambiguation number.
fn clean_name(name: &str) -> String {
    let mut name = name.trim();
    loop {
        let before = name;
        name = name.trim_end_matches('*').trim_end();
        if let Some(stripped) = name.strip_suffix(')')
            && let Some((head, number)) = stripped.rsplit_once(" (")
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
        {
            name = head.trim_end();
        }
        if name == before {
            return name.to_owned();
        }
    }
}

fn normalize_various(name: String) -> String {
    if name == "Various" {
        "Various Artists".to_owned()
    } else {
        name
    }
}

/// Keeps `YYYY`, `YYYY-MM` or `YYYY-MM-DD`, dropping `-00` components. Garbage yields "".
fn normalize_date(raw: &str) -> String {
    let mut parts = raw.trim().split('-');
    let year = match parts.next() {
        Some(y) if y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()) && y != "0000" => y,
        _ => return String::new(),
    };
    let mut out = year.to_owned();
    for part in parts.take(2) {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_digit()) || part == "00" {
            break;
        }
        out.push('-');
        out.push_str(part);
    }
    out
}

/// `none` (any case) and blank catalogue numbers are dropped.
fn catalog_number(raw: &str) -> String {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("none") {
        String::new()
    } else {
        raw.to_owned()
    }
}

/// Barcode digits only (Discogs writes `5 012394 144777`); values with letters are rejected.
fn clean_barcode(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    (!cleaned.is_empty() && cleaned.bytes().all(|b| b.is_ascii_digit())).then_some(cleaned)
}

/// MusicBrainz-style medium format from a Discogs format name and its descriptions.
fn media_name(name: &str, descriptions: &[String]) -> String {
    match name {
        "Vinyl" => match descriptions.iter().find(|d| is_vinyl_size(d)) {
            Some(size) => format!("{size} Vinyl"),
            None if descriptions.iter().any(|d| d == "LP") => "12\" Vinyl".to_owned(),
            None => "Vinyl".to_owned(),
        },
        "File" => "Digital Media".to_owned(),
        "CDr" => "CD-R".to_owned(),
        "Minidisc" => "MiniDisc".to_owned(),
        other => other.to_owned(),
    }
}

fn is_vinyl_size(desc: &str) -> bool {
    matches!(desc.strip_suffix('"'), Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Picard release types from Discogs format descriptions, lowercase, in order, deduplicated.
fn release_types<'a>(descriptions: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = Vec::new();
    for desc in descriptions {
        let kind = match desc {
            "Album" | "LP" => "album",
            "Single" | "Maxi-Single" => "single",
            "EP" => "ep",
            "Compilation" => "compilation",
            _ => continue,
        };
        push_unique(&mut out, kind.to_owned());
    }
    out
}

/// ISO 3166-1 code for the country names Discogs uses most; unknown values pass through.
fn country_code(name: &str) -> String {
    const TABLE: &[(&str, &str)] = &[
        ("US", "US"),
        ("UK", "GB"),
        ("Germany", "DE"),
        ("France", "FR"),
        ("Italy", "IT"),
        ("Japan", "JP"),
        ("Canada", "CA"),
        ("Netherlands", "NL"),
        ("Sweden", "SE"),
        ("Spain", "ES"),
        ("Australia", "AU"),
        ("Brazil", "BR"),
        ("Belgium", "BE"),
        ("Switzerland", "CH"),
        ("Austria", "AT"),
        ("Denmark", "DK"),
        ("Norway", "NO"),
        ("Finland", "FI"),
        ("Ireland", "IE"),
        ("Poland", "PL"),
        ("Portugal", "PT"),
        ("Greece", "GR"),
        ("Mexico", "MX"),
        ("Argentina", "AR"),
        ("New Zealand", "NZ"),
        ("South Africa", "ZA"),
        ("Czech Republic", "CZ"),
        ("Hungary", "HU"),
        ("Russia", "RU"),
        ("Turkey", "TR"),
        ("Israel", "IL"),
        ("India", "IN"),
        ("China", "CN"),
        ("South Korea", "KR"),
        ("Taiwan", "TW"),
        ("Europe", "XE"),
        ("Worldwide", "XW"),
    ];
    let name = name.trim();
    TABLE
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| name.to_owned(), |(_, code)| (*code).to_owned())
}

/// A parsed Discogs track position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    /// Explicit disc (`1-3`, `CD2-05`, `2.05`, `CD2`).
    disc: Option<u32>,
    /// Vinyl/cassette side, uppercase (`A1` → `A`; `AA1` → `A`).
    side: Option<u8>,
    /// Explicit track number, when the position carries one (`A` has none).
    track: Option<u32>,
    /// Trailing sub-track letter (`1a`).
    sub: Option<char>,
}

/// Parses `1`, `01`, `A`, `A1`, `AA2`, `1-3`, `CD2-05`, `2.05`, `1a`. Unrecognised → `None`.
fn parse_position(raw: &str) -> Option<Position> {
    let s = raw.trim();
    let letters = s.bytes().take_while(u8::is_ascii_alphabetic).count();
    let (word, rest) = s.split_at(letters);
    let mut p = Position {
        disc: None,
        side: None,
        track: None,
        sub: None,
    };

    if word.is_empty() {
        let (first, rest) = take_number(rest)?;
        if rest.is_empty() {
            p.track = Some(first);
        } else if let Some(sub) = single_letter(rest) {
            p.track = Some(first);
            p.sub = Some(sub);
        } else {
            let rest = rest.strip_prefix(['-', '.', ':', '/'])?;
            let (track, rest) = take_number(rest)?;
            p.disc = Some(first);
            p.track = Some(track);
            p.sub = parse_sub(rest)?;
        }
        return Some(p);
    }

    let bytes = word.as_bytes();
    let doubled_side = bytes.len() > 1 && bytes.iter().all(|b| b.eq_ignore_ascii_case(&bytes[0]));
    if bytes.len() == 1 || doubled_side {
        p.side = Some(bytes[0].to_ascii_uppercase());
        let rest = rest.trim_start_matches(['-', '.', ' ']);
        if rest.is_empty() {
            return Some(p);
        }
        let (track, rest) = take_number(rest)?;
        p.track = Some(track);
        p.sub = parse_sub(rest)?;
        return Some(p);
    }

    // `CD2`, `CD2-05`, `Disc 1-3`
    let (disc, rest) = take_number(rest.trim_start())?;
    p.disc = Some(disc);
    if rest.is_empty() {
        return Some(p);
    }
    let (track, rest) = take_number(rest.strip_prefix(['-', '.', ':', '/'])?)?;
    p.track = Some(track);
    p.sub = parse_sub(rest)?;
    Some(p)
}

fn take_number(s: &str) -> Option<(u32, &str)> {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    Some((s[..end].parse().ok()?, &s[end..]))
}

fn single_letter(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// `""` → `Some(None)`, `"a"` → `Some(Some('a'))`, `".2"` (as in `A1.2`) → `Some(None)`, anything
/// else → `None`.
fn parse_sub(rest: &str) -> Option<Option<char>> {
    if rest.is_empty() {
        return Some(None);
    }
    if let Some(digits) = rest.strip_prefix('.')
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
    {
        return Some(None);
    }
    single_letter(rest).map(Some)
}

/// `m:ss` or `h:mm:ss` → milliseconds. Empty or malformed → `None`.
fn parse_duration_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut total = 0u64;
    let mut parts = 0;
    for part in raw.split(':') {
        total = total
            .checked_mul(60)?
            .checked_add(part.trim().parse::<u64>().ok()?)?;
        parts += 1;
    }
    (parts <= 3 && total > 0).then_some(total * 1000)
}

// ---- JSON helpers -------------------------------------------------------------------------

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Discogs ids are numbers in most payloads but strings in some.
fn json_id(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !value.is_empty() && !list.contains(&value) {
        list.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings() -> MetadataSettings {
        MetadataSettings::default()
    }

    /// `(disc, side, track, sub-track)`.
    type Pos = (Option<u32>, Option<char>, Option<u32>, Option<char>);

    fn pos(raw: &str) -> Option<Pos> {
        parse_position(raw).map(|p| (p.disc, p.side.map(char::from), p.track, p.sub))
    }

    #[test]
    fn position_table() {
        let cases: &[(&str, Option<Pos>)] = &[
            ("1", Some((None, None, Some(1), None))),
            ("01", Some((None, None, Some(1), None))),
            ("12", Some((None, None, Some(12), None))),
            ("A", Some((None, Some('A'), None, None))),
            ("A1", Some((None, Some('A'), Some(1), None))),
            ("B2", Some((None, Some('B'), Some(2), None))),
            ("D", Some((None, Some('D'), None, None))),
            ("AA1", Some((None, Some('A'), Some(1), None))),
            ("1-3", Some((Some(1), None, Some(3), None))),
            ("1-1", Some((Some(1), None, Some(1), None))),
            ("CD1-3", Some((Some(1), None, Some(3), None))),
            ("CD2-05", Some((Some(2), None, Some(5), None))),
            ("2.05", Some((Some(2), None, Some(5), None))),
            ("Disc 2-1", Some((Some(2), None, Some(1), None))),
            ("CD2", Some((Some(2), None, None, None))),
            ("1a", Some((None, None, Some(1), Some('a')))),
            ("1-2b", Some((Some(1), None, Some(2), Some('b')))),
            (" B3 ", Some((None, Some('B'), Some(3), None))),
            ("", None),
            ("-", None),
            ("1-", None),
            ("A1x1", None),
            ("1ab", None),
            ("CD", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(&pos(raw), expected, "position `{raw}`");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("3:32"), Some(212_000));
        assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms(""), None);
        assert_eq!(parse_duration_ms("n/a"), None);
    }

    #[test]
    fn names_and_dates() {
        assert_eq!(clean_name("E.B.E. (2)"), "E.B.E.");
        assert_eq!(clean_name("Madonna*"), "Madonna");
        assert_eq!(clean_name("Name (12)*"), "Name");
        assert_eq!(clean_name("Name (Live)"), "Name (Live)");
        assert_eq!(normalize_date("1987-07-00"), "1987-07");
        assert_eq!(normalize_date("1987-00-00"), "1987");
        assert_eq!(normalize_date("1987-07-14"), "1987-07-14");
        assert_eq!(normalize_date("1987"), "1987");
        assert_eq!(normalize_date("0000-00-00"), "");
        assert_eq!(normalize_date("Unknown"), "");
    }

    #[test]
    fn artist_credit_joins_and_anv() {
        let artists = json!([
            {"name": "Rick Astley", "anv": "R. Astley", "join": "&", "id": 1},
            {"name": "Kylie (3)", "anv": "", "join": ",", "id": 2},
            {"name": "Various", "anv": "", "join": "", "id": 3}
        ]);
        let c = Credit::from_artists(Some(&artists), false);
        assert_eq!(c.text, "R. Astley & Kylie, Various Artists");
        assert_eq!(c.names, ["R. Astley", "Kylie", "Various Artists"]);
        assert_eq!(c.ids, ["1", "2", "3"]);
        let c = Credit::from_artists(Some(&artists), true);
        assert_eq!(c.text, "Rick Astley & Kylie, Various Artists");
    }

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/tests/fixtures/discogs/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn maps_vinyl_with_sides_and_index_track() {
        // Billy Cobham – Total Eclipse (LP): A1 is an `index` entry with sub-tracks A1a..A1e.
        let rel = map_release(&fixture("vinyl_sides_index"), &settings()).unwrap();
        assert_eq!(
            rel.id,
            ReleaseId {
                source: Source::Discogs,
                id: "472043".into()
            }
        );
        let m = &rel.metadata;
        assert_eq!(m.get("album"), Some("Total Eclipse"));
        assert_eq!(m.get("albumartist"), Some("Billy Cobham"));
        assert_eq!(m.get("date"), Some("1974"));
        assert_eq!(m.get("releasecountry"), Some("US"));
        assert_eq!(m.get("media"), Some("12\" Vinyl"));
        assert_eq!(m.get_all("releasetype"), ["album"]);
        assert_eq!(m.get_all("genre"), ["Jazz", "Fusion", "Jazz-Rock"]);
        assert_eq!(m.get("label"), Some("Atlantic"));
        assert_eq!(m.get("catalognumber"), Some("SD 18121"));
        assert_eq!(m.get("discogs_masterid"), Some("47691"));
        assert_eq!(m.get("totaldiscs"), Some("1"));
        assert!(!m.contains("barcode"));

        let ids: Vec<_> = rel.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "472043:A1a",
                "472043:A1b",
                "472043:A1c",
                "472043:A1d",
                "472043:A1e",
                "472043:A2",
                "472043:A3",
                "472043:A4",
                "472043:B1",
                "472043:B2",
                "472043:B3",
                "472043:B4"
            ]
        );
        for (i, t) in rel.tracks.iter().enumerate() {
            assert_eq!(t.metadata.get("discnumber"), Some("1"));
            assert_eq!(
                t.metadata.get("tracknumber"),
                Some((i + 1).to_string().as_str())
            );
            assert_eq!(t.metadata.get("totaltracks"), Some("12"));
            assert_eq!(t.metadata.get("artist"), Some("Billy Cobham"));
            assert!(!t.metadata.contains("~multiartist"));
        }
        assert_eq!(rel.tracks[1].metadata.get("title"), Some("Second Phase"));
        assert_eq!(
            rel.tracks[1].metadata.length().map(|d| d.as_secs()),
            Some(103)
        );
        match &rel.artwork {
            Artwork::Discogs(images) => assert!(
                images
                    .iter()
                    .any(|(uri, primary)| *primary && uri.starts_with("https://"))
            ),
            other => panic!("unexpected artwork {other:?}"),
        }
    }

    #[test]
    fn vinyl_sides_c_and_d_become_disc_two() {
        // The Persuader – Stockholm, 2×12": A, B1, B2 / C1, C2, D.
        let rel = map_release(&fixture("vinyl_two_lps"), &settings()).unwrap();
        assert_eq!(rel.metadata.get("totaldiscs"), Some("2"));
        assert_eq!(rel.metadata.get("media"), Some("12\" Vinyl"));
        assert_eq!(rel.metadata.get("releasecountry"), Some("SE"));
        assert_eq!(rel.metadata.get("label"), Some("Svek"));
        let got: Vec<_> = rel
            .tracks
            .iter()
            .map(|t| {
                let m = &t.metadata;
                (
                    t.id.as_str(),
                    m.get("discnumber").unwrap(),
                    m.get("tracknumber").unwrap(),
                    m.get("totaltracks").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("1:A", "1", "1", "3"),
                ("1:B1", "1", "2", "3"),
                ("1:B2", "1", "3", "3"),
                ("1:C1", "2", "1", "3"),
                ("1:C2", "2", "2", "3"),
                ("1:D", "2", "3", "3")
            ]
        );
    }

    #[test]
    fn maps_multi_cd_with_disc_dash_track_positions() {
        // Zenith – Goliath: 2×CD, positions `1-01`..`1-14`, `2-01`..`2-14`.
        let rel = map_release(&fixture("multi_cd"), &settings()).unwrap();
        let m = &rel.metadata;
        assert_eq!(m.get("totaldiscs"), Some("2"));
        assert_eq!(m.get("media"), Some("CD"));
        assert_eq!(m.get("date"), Some("2006-11-19"));
        assert_eq!(m.get("releasecountry"), Some("IT"));
        assert_eq!(m.get("label"), Some("Atlantis"));
        assert_eq!(m.get("barcode"), Some("8032484015739"));
        assert_eq!(m.get_all("releasetype"), ["compilation"]);
        assert_eq!(rel.tracks.len(), 28);

        let first_of_disc2 = &rel.tracks[14];
        assert_eq!(first_of_disc2.id, "1142542:2-01");
        assert_eq!(first_of_disc2.metadata.get("discnumber"), Some("2"));
        assert_eq!(first_of_disc2.metadata.get("tracknumber"), Some("1"));
        assert_eq!(first_of_disc2.metadata.get("totaltracks"), Some("14"));
        let last = &rel.tracks[27].metadata;
        assert_eq!(
            (last.get("discnumber"), last.get("tracknumber")),
            (Some("2"), Some("14"))
        );

        // Per-track artists differ from the album artist.
        let t3 = &rel.tracks[2].metadata;
        assert_eq!(t3.get_all("artists"), ["The Beholder", "DJ Zany"]);
        assert_eq!(t3.get("~multiartist"), Some("1"));
        assert_eq!(t3.get_all("discogs_artistid").len(), 2);
        assert_eq!(rel.tracks[3].metadata.get("artist"), Some("Lowriders"));
    }

    #[test]
    fn maps_various_artists_compilation() {
        // Allstar Bootymix: `Various` album artist, `Name (2)` track artists and join words.
        let rel = map_release(&fixture("va_compilation"), &settings()).unwrap();
        let m = &rel.metadata;
        assert_eq!(m.get("albumartist"), Some("Various Artists"));
        assert_eq!(m.get("albumartistsort"), Some("Various Artists"));
        assert_eq!(m.get("discogs_artistid"), Some("194"));
        assert_eq!(m.get("discogs_masterid"), Some("1385776"));
        assert_eq!(m.get("barcode"), Some("015095033928"));
        assert_eq!(m.get_all("releasetype"), ["compilation"]);
        assert_eq!(m.get("releasecountry"), Some("USA & Canada"));

        let t1 = &rel.tracks[0].metadata;
        // The credited name (ANV) is used by default and the canonical one when standardising.
        assert_eq!(t1.get("artist"), Some("Kooley \"C\" And The G.O.Q."));
        assert_eq!(t1.get_all("artists"), ["Kooley \"C\"", "The G.O.Q."]);
        let standard = MetadataSettings {
            standardize_artist_names: crate::settings::StandardizeArtistNames::All,
            ..MetadataSettings::default()
        };
        let std_rel = map_release(&fixture("va_compilation"), &standard).unwrap();
        assert_eq!(
            std_rel.tracks[0].metadata.get("artist"),
            Some("Kooley \"C\" And Gods Of Quad")
        );
        assert_eq!(t1.get("albumartist"), Some("Various Artists"));
        assert_eq!(t1.length().map(|d| d.as_millis()), Some(223_000));
        let t3 = &rel.tracks[2].metadata;
        assert_eq!(t3.get("artist"), Some("Invisible Men Featuring C.D."));
        assert_eq!(t3.get_all("artists"), ["Invisible Men", "C.D."]);
        for t in &rel.tracks {
            assert_eq!(t.metadata.get("~multiartist"), Some("1"));
            assert!(
                !t.metadata.contains("composer"),
                "Producer roles are not composers"
            );
        }
    }

    #[test]
    fn maps_rick_astley_single_with_bare_side_positions() {
        let rel = map_release(&fixture("rick_astley_7inch"), &settings()).unwrap();
        let m = &rel.metadata;
        assert_eq!(m.get("album"), Some("Never Gonna Give You Up"));
        assert_eq!(m.get("albumartist"), Some("Rick Astley"));
        assert_eq!(m.get("date"), Some("1987-07"));
        assert_eq!(m.get("media"), Some("7\" Vinyl"));
        assert_eq!(m.get_all("releasetype"), ["single"]);
        assert_eq!(m.get("discogs_artistid"), Some("72872"));
        assert_eq!(rel.tracks.len(), 2);
        assert_eq!(rel.tracks[1].metadata.get("tracknumber"), Some("2"));
        assert_eq!(
            rel.tracks[1].metadata.get("title"),
            Some("Never Gonna Give You Up (Instrumental)")
        );
        assert_eq!(rel.tracks[1].id, "249504:B");
    }

    #[test]
    fn extra_artist_credits_and_standardized_names() {
        let json = json!({
            "id": 7, "title": "T", "artists": [{"name": "Main (2)", "anv": "The Main*", "join": "", "id": 9}],
            "extraartists": [{"name": "Whole", "role": "Written-By", "tracks": ""}],
            "tracklist": [
                {"position": "1", "type_": "heading", "title": "Disc one"},
                {"position": "1", "type_": "track", "title": "One", "duration": "",
                 "extraartists": [{"name": "Lyr", "role": "Lyrics By"}, {"name": "Comp (3)", "role": "Composed By, Producer"}]},
                {"position": "2", "type_": "track", "title": "Two", "duration": "0:00",
                 "artists": [{"name": "Guest", "join": "", "id": 5}]}
            ],
            "images": [{"uri": "https://i/a.jpg", "type": "secondary"}, {"uri": "https://i/b.jpg", "type": "primary"}]
        });
        let rel = map_release(&json, &settings()).unwrap();
        assert_eq!(rel.metadata.get("albumartist"), Some("The Main"));
        let t1 = &rel.tracks[0].metadata;
        assert_eq!(t1.get_all("composer"), ["Comp", "Whole"]);
        assert_eq!(t1.get_all("lyricist"), ["Lyr"]);
        assert!(!t1.contains("~length"));
        assert_eq!(
            t1.get("~multiartist"),
            Some("1"),
            "two distinct track artists: flagged on every track"
        );
        let t2 = &rel.tracks[1].metadata;
        assert_eq!(t2.get("artist"), Some("Guest"));
        assert_eq!(t2.get("~multiartist"), Some("1"));
        assert_eq!(t2.get_all("composer"), ["Whole"]);
        assert!(!t2.contains("~length"));
        assert_eq!(
            rel.artwork,
            Artwork::Discogs(vec![
                ("https://i/a.jpg".into(), false),
                ("https://i/b.jpg".into(), true)
            ])
        );

        let standard = MetadataSettings {
            standardize_artist_names: crate::settings::StandardizeArtistNames::All,
            ..MetadataSettings::default()
        };
        assert_eq!(
            map_release(&json, &standard)
                .unwrap()
                .metadata
                .get("albumartist"),
            Some("Main")
        );
    }

    #[test]
    fn search_hit_mapping() {
        let hit = json!({
            "id": 249504, "title": "Rick Astley - Never Gonna Give You Up", "year": "1987", "country": "UK",
            "format": ["Vinyl", "7\"", "45 RPM", "Single"], "label": ["RCA (4)", "Other"], "catno": "PB 41447",
            "barcode": ["5 012394 144777", "x"], "master_id": 96559
        });
        let c = map_hit(&hit, 2).unwrap();
        assert_eq!(c.search_score, 94);
        assert_eq!(
            c.id,
            ReleaseId {
                source: Source::Discogs,
                id: "249504".into()
            }
        );
        let m = &c.metadata;
        assert_eq!(m.get("album"), Some("Never Gonna Give You Up"));
        assert_eq!(m.get("albumartist"), Some("Rick Astley"));
        assert_eq!(m.get("date"), Some("1987"));
        assert_eq!(m.get("releasecountry"), Some("GB"));
        assert_eq!(m.get("media"), Some("7\" Vinyl"));
        assert_eq!(m.get("label"), Some("RCA"));
        assert_eq!(m.get("catalognumber"), Some("PB 41447"));
        assert_eq!(m.get("barcode"), Some("5012394144777"));
        assert_eq!(m.get("discogs_albumid"), Some("249504"));
        assert_eq!(m.get_all("releasetype"), ["single"]);

        assert_eq!(map_hit(&hit, 40).unwrap().search_score, 0);
        let va = json!({"id": 1, "title": "Various - Hits", "catno": "none"});
        let m = map_hit(&va, 0).unwrap().metadata;
        assert_eq!(m.get("albumartist"), Some("Various Artists"));
        assert!(!m.contains("catalognumber"));
    }

    #[test]
    fn search_requires_token() {
        let q = ReleaseQuery {
            album: "x".into(),
            ..ReleaseQuery::default()
        };
        for token in [None, Some(""), Some("  ")] {
            match search_releases(&q, 5, token) {
                Err(Error::Config(msg)) => {
                    assert_eq!(msg, "Discogs search needs a personal access token")
                }
                other => panic!("expected Config error, got {other:?}"),
            }
        }
    }
}
