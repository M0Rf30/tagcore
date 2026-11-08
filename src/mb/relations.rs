//! Relationships → tags: a port of Picard's `_relations_to_metadata*` and `work_to_metadata`
//! (artist, work, url, series and label relationships).

use std::collections::BTreeMap;

use serde_json::Value;

use super::artist::relation_artist_name;
use super::text::amazon_asin;
use super::{add_unique, array, str_field};
use crate::metadata::Metadata;
use crate::settings::MetadataSettings;

/// Picard's `_ARTIST_REL_TYPES` (instrument, vocal and performer relationships are handled
/// separately, see [`performer_tag`]).
fn artist_rel_tag(reltype: &str) -> Option<&'static str> {
    Some(match reltype {
        "arranger" | "instrument arranger" | "orchestrator" | "vocal arranger" => "arranger",
        "audio" | "engineer" | "live sound" | "sound" => "engineer",
        "chorus master" => "performer:chorus master",
        "composer" => "composer",
        "concertmaster" => "performer:concertmaster",
        "conductor" => "conductor",
        "librettist" | "lyricist" => "lyricist",
        "mix-DJ" => "djmixer",
        "mix" => "mixer",
        "performing orchestra" => "performer:orchestra",
        "producer" => "producer",
        "remixer" => "remixer",
        "audio director" | "video director" => "director",
        "writer" => "writer",
        _ => return None,
    })
}

const PREFIX_ATTRS: [&str; 4] = ["guest", "additional", "minor", "solo"];

/// Picard's `_parse_attributes`: `"guest" "additional" ... + "a, b and c"`; relationships without
/// a noun fall back to `instruments` / `vocals`.
fn parse_attributes(attrs: &[&str], reltype: &str, credits: &Value) -> String {
    let mut prefixes: Vec<&str> = Vec::new();
    let mut nouns: Vec<&str> = Vec::new();
    for &attr in attrs {
        let attr = credits[attr].as_str().unwrap_or(attr);
        if PREFIX_ATTRS.contains(&attr) {
            prefixes.push(attr);
        } else {
            nouns.push(attr);
        }
    }
    let result = match nouns.as_slice() {
        [] => match reltype {
            "instrument" => "instruments".to_owned(),
            "vocal" => "vocals".to_owned(),
            _ => String::new(),
        },
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
    };
    [prefixes.join(" "), result].join(" ").trim().to_owned()
}

/// The `performer:<attributes>` tag of a vocal, instrument or performer relationship.
fn performer_tag(
    relation: &Value,
    reltype: &str,
    attrs: &[&str],
    settings: &MetadataSettings,
) -> String {
    let use_credits = (reltype == "instrument" && !settings.standardize_instruments)
        || (reltype == "vocal" && !settings.standardize_vocals);
    let credits = if use_credits {
        relation["attribute-credits"].clone()
    } else {
        Value::Null
    };
    format!("performer:{}", parse_attributes(attrs, reltype, &credits))
}

/// Which entity the relationships belong to; scopes the series variables (`~release_series`...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entity {
    Release,
    ReleaseGroup,
    Recording,
    Work,
}

impl Entity {
    fn series_prefix(self) -> &'static str {
        match self {
            Self::Release => "~release_",
            Self::ReleaseGroup => "~releasegroup_",
            Self::Recording => "~recording_",
            Self::Work => "~work_",
        }
    }
}

/// Picard's `_relations_to_metadata`. Returns the medium-scoped DJ mixers (`medium 2` attributes
/// of `mix-DJ` relationships): `medium position → names`; they are not added to `m`.
pub(super) fn relations_to_metadata(
    relations: &[Value],
    m: &mut Metadata,
    instrumental: bool,
    entity: Entity,
    settings: &MetadataSettings,
) -> BTreeMap<u32, Vec<String>> {
    let mut per_medium: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut series_cleared = false;
    for relation in relations {
        match relation["target-type"].as_str().unwrap_or_default() {
            "artist" => artist_relation(relation, m, instrumental, settings, &mut per_medium),
            "work" => work_relation(relation, m, settings),
            "url" => url_relation(relation, m),
            "label" => {
                if relation["type"] == "broadcast"
                    && let Some(begin) = str_field(relation, "begin")
                {
                    m.set("~broadcast_date", begin);
                }
            }
            "series" => series_relation(relation, m, entity, &mut series_cleared),
            _ => {}
        }
    }
    per_medium
}

fn artist_relation(
    relation: &Value,
    m: &mut Metadata,
    instrumental: bool,
    settings: &MetadataSettings,
    per_medium: &mut BTreeMap<u32, Vec<String>>,
) {
    let artist = &relation["artist"];
    if !artist.is_object() {
        return;
    }
    let alias = relation_artist_name(relation, artist, settings);
    let reltype = relation["type"].as_str().unwrap_or_default();
    let attrs: Vec<&str> = array(relation, "attributes")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let name = if matches!(reltype, "vocal" | "instrument" | "performer") {
        performer_tag(relation, reltype, &attrs, settings)
    } else if reltype == "mix-DJ" && !attrs.is_empty() {
        for attr in attrs.iter().filter(|a| a.starts_with("medium")) {
            match attr
                .split_whitespace()
                .nth(1)
                .and_then(|n| n.parse::<u32>().ok())
            {
                Some(medium) => {
                    let names = per_medium.entry(medium).or_default();
                    if !names.contains(&alias.name) {
                        names.push(alias.name.clone());
                    }
                }
                None => log::warn!("Invalid medium number in mix-DJ attribute: {attr}"),
            }
        }
        return;
    } else if let Some(tag) = artist_rel_tag(reltype) {
        tag.to_owned()
    } else {
        return;
    };
    if instrumental && name == "lyricist" {
        return;
    }
    add_unique(m, &name, &alias.name);
    match name.as_str() {
        "composer" => {
            add_unique(m, "composersort", &alias.sort_name);
            if let Some(id) = str_field(artist, "id") {
                add_unique(m, "musicbrainz_composerid", id);
            }
        }
        "lyricist" => add_unique(m, "~lyricistsort", &alias.sort_name),
        "writer" => add_unique(m, "~writersort", &alias.sort_name),
        _ => {}
    }
}

fn work_relation(relation: &Value, m: &mut Metadata, settings: &MetadataSettings) {
    if relation["type"] != "performance" {
        return;
    }
    let attributes: Vec<&str> = array(relation, "attributes")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for attribute in &attributes {
        add_unique(m, "~performance_attributes", attribute);
    }
    work_to_metadata(
        &relation["work"],
        m,
        attributes.contains(&"instrumental"),
        settings,
    );
}

/// Picard's `work_to_metadata`.
pub(super) fn work_to_metadata(
    work: &Value,
    m: &mut Metadata,
    instrumental: bool,
    settings: &MetadataSettings,
) {
    if let Some(id) = str_field(work, "id") {
        add_unique(m, "musicbrainz_workid", id);
    }
    if instrumental {
        add_unique(m, "language", "zxx"); // no lyrics
    } else if work.get("languages").is_some_and(Value::is_array) {
        for language in array(work, "languages").iter().filter_map(Value::as_str) {
            add_unique(m, "language", language);
        }
    } else if let Some(language) = str_field(work, "language") {
        add_unique(m, "language", language);
    }
    if let Some(title) = str_field(work, "title") {
        add_unique(m, "work", title);
    }
    if let Some(comment) = str_field(work, "disambiguation") {
        add_unique(m, "~workcomment", comment);
    }
    relations_to_metadata(
        array(work, "relations"),
        m,
        instrumental,
        Entity::Work,
        settings,
    );
    for iswc in array(work, "iswcs").iter().filter_map(Value::as_str) {
        m.add("~iswc", iswc);
    }
}

fn url_relation(relation: &Value, m: &mut Metadata) {
    let resource = relation["url"]["resource"].as_str().unwrap_or_default();
    match relation["type"].as_str().unwrap_or_default() {
        "amazon asin" if !m.contains("asin") => {
            if let Some(asin) = amazon_asin(resource) {
                m.set("asin", asin);
            }
        }
        "license" => m.add("license", resource),
        _ => {}
    }
}

fn series_relation(relation: &Value, m: &mut Metadata, entity: Entity, cleared: &mut bool) {
    if relation["type"] != "part of" {
        return;
    }
    let series = &relation["series"];
    let prefix = entity.series_prefix();
    let (name, mbid, comment, number) = (
        format!("{prefix}series"),
        format!("{prefix}seriesid"),
        format!("{prefix}seriescomment"),
        format!("{prefix}seriesnumber"),
    );
    if !*cleared {
        // Clear first so reloading does not accumulate identical values (PICARD-2700).
        for key in [&name, &mbid, &comment, &number] {
            m.remove(key);
        }
        *cleared = true;
    }
    m.add(&name, series["name"].as_str().unwrap_or_default());
    m.add(&mbid, series["id"].as_str().unwrap_or_default());
    m.add(
        &comment,
        series["disambiguation"].as_str().unwrap_or_default(),
    );
    m.add(
        &number,
        relation["attribute-values"]["number"]
            .as_str()
            .unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(relations: &Value, settings: &MetadataSettings) -> Metadata {
        let mut m = Metadata::new();
        relations_to_metadata(
            relations.as_array().expect("array"),
            &mut m,
            false,
            Entity::Recording,
            settings,
        );
        m
    }

    fn artist_rel(reltype: &str, attrs: Value, credits: Value) -> Value {
        json!({"target-type": "artist", "type": reltype, "attributes": attrs, "attribute-credits": credits,
               "artist": {"id": "a", "name": "Jane", "sort-name": "Doe, Jane"}})
    }

    #[test]
    fn performer_attributes_use_prefixes_nouns_and_blank_fallbacks() {
        let rels = json!([
            artist_rel("instrument", json!(["guest", "bass guitar"]), json!({})),
            artist_rel(
                "instrument",
                json!(["piano", "guitar", "solo", "drums"]),
                json!({})
            ),
            artist_rel("instrument", json!([]), json!({})),
            artist_rel("vocal", json!(["additional"]), json!({})),
            artist_rel("performer", json!(["strings"]), json!({})),
        ]);
        let m = run(&rels, &MetadataSettings::default());
        assert_eq!(m.get_all("performer:guest bass guitar"), ["Jane"]);
        assert_eq!(
            m.get_all("performer:solo piano, guitar and drums"),
            ["Jane"]
        );
        assert_eq!(m.get_all("performer:instruments"), ["Jane"]);
        assert_eq!(m.get_all("performer:additional vocals"), ["Jane"]);
        assert_eq!(m.get_all("performer:strings"), ["Jane"]);
    }

    #[test]
    fn credited_instrument_names_only_when_not_standardising() {
        let rel = artist_rel("instrument", json!(["guitar"]), json!({"guitar": "axe"}));
        let rels = json!([rel]);
        assert_eq!(
            run(&rels, &MetadataSettings::default()).get_all("performer:guitar"),
            ["Jane"]
        );
        let credited = MetadataSettings {
            standardize_instruments: false,
            ..MetadataSettings::default()
        };
        assert_eq!(run(&rels, &credited).get_all("performer:axe"), ["Jane"]);
        let vocal = json!([artist_rel(
            "vocal",
            json!(["soprano"]),
            json!({"soprano": "high voice"})
        )]);
        let cv = MetadataSettings {
            standardize_vocals: false,
            ..MetadataSettings::default()
        };
        assert_eq!(run(&vocal, &cv).get_all("performer:high voice"), ["Jane"]);
    }

    #[test]
    fn composer_lyricist_writer_sort_tags_and_instrumental_skips_lyricist() {
        let rels = json!([
            artist_rel("composer", json!([]), json!({})),
            artist_rel("lyricist", json!([]), json!({})),
            artist_rel("writer", json!([]), json!({})),
        ]);
        let m = run(&rels, &MetadataSettings::default());
        assert_eq!(m.get_all("composersort"), ["Doe, Jane"]);
        assert_eq!(m.get_all("musicbrainz_composerid"), ["a"]);
        assert_eq!(m.get_all("~lyricistsort"), ["Doe, Jane"]);
        assert_eq!(m.get_all("~writersort"), ["Doe, Jane"]);
        let mut inst = Metadata::new();
        relations_to_metadata(
            rels.as_array().expect("array"),
            &mut inst,
            true,
            Entity::Work,
            &MetadataSettings::default(),
        );
        assert!(!inst.contains("lyricist") && !inst.contains("~lyricistsort"));
    }

    #[test]
    fn series_url_and_label_relationships() {
        let rels = json!([
            {"target-type": "series", "type": "part of", "attribute-values": {"number": "4"},
             "series": {"id": "s1", "name": "Now!", "disambiguation": "UK"}},
            {"target-type": "series", "type": "part of", "attribute-values": {},
             "series": {"id": "s2", "name": "Other", "disambiguation": ""}},
            {"target-type": "url", "type": "amazon asin", "url": {"resource": "https://www.amazon.co.uk/gp/product/B000002UAL"}},
            {"target-type": "url", "type": "license", "url": {"resource": "https://creativecommons.org/licenses/by/4.0/"}},
            {"target-type": "label", "type": "broadcast", "begin": "2015-04-27"},
        ]);
        let mut m = Metadata::new();
        m.set("~release_series", "stale");
        relations_to_metadata(
            rels.as_array().expect("array"),
            &mut m,
            false,
            Entity::Release,
            &MetadataSettings::default(),
        );
        assert_eq!(
            m.get_all("~release_series"),
            ["Now!", "Other"],
            "stale value cleared once"
        );
        assert_eq!(m.get_all("~release_seriesid"), ["s1", "s2"]);
        assert_eq!(m.get_all("~release_seriescomment"), ["UK"]);
        assert_eq!(m.get_all("~release_seriesnumber"), ["4"]);
        assert_eq!(m.get("asin"), Some("B000002UAL"));
        assert_eq!(
            m.get("license"),
            Some("https://creativecommons.org/licenses/by/4.0/")
        );
        assert_eq!(m.get("~broadcast_date"), Some("2015-04-27"));
        // An existing ASIN is kept.
        let mut has = Metadata::new();
        has.set("asin", "B111111111");
        relations_to_metadata(
            &rels.as_array().expect("array")[2..3],
            &mut has,
            false,
            Entity::Release,
            &MetadataSettings::default(),
        );
        assert_eq!(has.get("asin"), Some("B111111111"));
    }

    #[test]
    fn work_mapping_adds_comment_iswc_languages_and_attributes() {
        let rels = json!([{
            "target-type": "work", "type": "performance", "attributes": ["live", "partial"],
            "work": {"id": "w", "title": "Song", "disambiguation": "the long one", "languages": ["eng", "fra"],
                     "iswcs": ["T-123.456.789-0"], "relations": []}
        }]);
        let m = run(&rels, &MetadataSettings::default());
        assert_eq!(m.get_all("~performance_attributes"), ["live", "partial"]);
        assert_eq!(m.get("~workcomment"), Some("the long one"));
        assert_eq!(m.get("~iswc"), Some("T-123.456.789-0"));
        assert_eq!(m.get_all("language"), ["eng", "fra"]);
        assert_eq!(m.get("work"), Some("Song"));
    }

    #[test]
    fn medium_scoped_dj_mixers_are_returned_not_added() {
        let rels = json!([
            artist_rel("mix-DJ", json!(["medium 2"]), json!({})),
            artist_rel("mix-DJ", json!(["medium x"]), json!({})),
            artist_rel("mix-DJ", json!([]), json!({})),
        ]);
        let mut m = Metadata::new();
        let per = relations_to_metadata(
            rels.as_array().expect("array"),
            &mut m,
            false,
            Entity::Release,
            &MetadataSettings::default(),
        );
        assert_eq!(per[&2], ["Jane"]);
        assert_eq!(
            m.get_all("djmixer"),
            ["Jane"],
            "unscoped mix-DJ applies to the whole release"
        );
    }
}
