//! Artist names: alias lookup and translation, standardisation of credited names, and the
//! artist-credit tags (`artist`, `artists`, `~artists_sort`, ...). A port of the corresponding
//! parts of Picard's `mbjson.py`.

use std::collections::HashMap;

use serde_json::Value;

use super::text::{detect_script_weighted, matches_script_exception, translate_from_sortname};
use super::{array, str_field};
use crate::metadata::Metadata;
use crate::settings::{MetadataSettings, StandardizeArtistNames};

/// MusicBrainz alias type ids (`picard/const/__init__.py`).
const ALIAS_TYPE_ARTIST_NAME: &str = "894afba6-2816-3c24-8072-eadb66bd04bc";
const ALIAS_TYPE_LEGAL_NAME: &str = "d4dcd0c0-b341-3612-a332-c0ce797b25cf";
const ALIAS_TYPE_RECORDING_NAME: &str = "5d564c8f-97de-3572-94bb-7f40ad661499";

/// The special "Various Artists" artist.
pub const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

/// A name with its sort name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Alias {
    pub name: String,
    pub sort_name: String,
}

// ---- alias selection ----------------------------------------------------------------------

/// Picard's `_locales_from_aliases`: best primary alias per locale (and per root language of a
/// regional locale), scored by locale specificity and alias type.
fn locales_from_aliases(aliases: &[Value]) -> HashMap<String, (f64, Alias)> {
    let mut locales: HashMap<String, (f64, Alias)> = HashMap::new();
    for alias in aliases {
        if !alias["primary"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(full_locale) = str_field(alias, "locale") else {
            continue;
        };
        let root_locale = full_locale.split('_').next().unwrap_or(full_locale);
        let locale_score = if full_locale.contains('_') { 0.4 } else { 0.8 };
        let type_score = match str_field(alias, "type-id") {
            Some(ALIAS_TYPE_ARTIST_NAME | ALIAS_TYPE_RECORDING_NAME) => 0.8,
            Some(ALIAS_TYPE_LEGAL_NAME) => 0.5,
            _ => 0.0,
        };
        let value = Alias {
            name: alias["name"].as_str().unwrap_or_default().to_owned(),
            sort_name: alias["sort-name"].as_str().unwrap_or_default().to_owned(),
        };
        // `linear_combination_of_weights([(v0, 5), (v1, 5)])`.
        let full = (0.8 * 5.0 + type_score * 5.0) / 10.0;
        let root = (locale_score * 5.0 + type_score * 5.0) / 10.0;
        for (locale, score) in [(full_locale, full), (root_locale, root)] {
            if locales.get(locale).is_none_or(|(best, _)| score > *best) {
                locales.insert(locale.to_owned(), (score, value.clone()));
            }
        }
    }
    locales
}

/// Picard's `_find_localized_alias_name`: the alias for the first preferred locale that has one,
/// then the first root language of a regional preferred locale (`en_US` → `en`).
pub(super) fn find_localized_alias(
    aliases: &[Value],
    preferred_locales: &[String],
) -> Option<Alias> {
    if aliases.is_empty() {
        return None;
    }
    let locales = locales_from_aliases(aliases);
    let lookup = |key: &str| locales.get(key).map(|(_, a)| a.clone());
    preferred_locales
        .iter()
        .find_map(|l| lookup(l))
        .or_else(|| {
            preferred_locales
                .iter()
                .filter(|l| l.contains('_'))
                .filter_map(|l| l.split('_').next())
                .find_map(lookup)
        })
}

/// Picard's `_should_skip_translation_due_to_scripts`.
pub(super) fn skip_translation_due_to_scripts(text: &str, settings: &MetadataSettings) -> bool {
    if text.is_empty() || !settings.translate_artist_names_script_exception {
        return false;
    }
    let detected = detect_script_weighted(text);
    if detected.is_empty() || settings.script_exceptions.is_empty() {
        return false;
    }
    let exceptions: Vec<(&str, u8)> = settings
        .script_exceptions
        .iter()
        .map(|e| (e.script.as_str(), e.weighting))
        .collect();
    matches_script_exception(&detected, &exceptions)
}

/// Picard's `_translate_artist_node`.
fn translate_artist(artist: &Value, settings: &MetadataSettings) -> Alias {
    let name = artist["name"].as_str().unwrap_or_default();
    let sort_name = artist["sort-name"].as_str().unwrap_or_default();
    if settings.translate_artist_names && !skip_translation_due_to_scripts(name, settings) {
        if let Some(alias) =
            find_localized_alias(array(artist, "aliases"), &settings.translation_locales)
        {
            return alias;
        }
        let translated = if settings.translate_from_sortname {
            translate_from_sortname(name, sort_name)
        } else {
            name.to_owned()
        };
        return Alias {
            name: translated,
            sort_name: sort_name.to_owned(),
        };
    }
    Alias {
        name: name.to_owned(),
        sort_name: sort_name.to_owned(),
    }
}

/// Picard's `_select_sort_name_from_aliases`: the sort name of an alias spelled `name`
/// (preferring the translation locales when translating), else the artist's own sort name, else
/// `name`.
fn select_sort_name(artist: &Value, name: &str, settings: &MetadataSettings) -> String {
    let aliases = array(artist, "aliases");
    if !aliases.is_empty() {
        let candidates: Vec<Value> = aliases
            .iter()
            .filter(|a| a["name"].as_str() == Some(name) && str_field(a, "sort-name").is_some())
            .cloned()
            .collect();
        if settings.translate_artist_names
            && let Some(alias) = find_localized_alias(&candidates, &settings.translation_locales)
        {
            return alias.sort_name;
        }
        if let Some(first) = candidates.first() {
            return first["sort-name"].as_str().unwrap_or_default().to_owned();
        }
    }
    str_field(artist, "sort-name").unwrap_or(name).to_owned()
}

/// Picard's `should_standardize_artist_name`.
pub(super) fn should_standardize(
    mode: StandardizeArtistNames,
    credited: &str,
    artist: &Value,
) -> bool {
    match mode {
        StandardizeArtistNames::None => return false,
        StandardizeArtistNames::All => return true,
        StandardizeArtistNames::Variations => {}
    }
    // A credit that is an ended alias is a real name change, not a variation.
    !array(artist, "aliases").iter().any(|alias| {
        let type_ok = match alias.get("type-id") {
            None | Some(Value::Null) => true,
            Some(id) => id.as_str() == Some(ALIAS_TYPE_ARTIST_NAME),
        };
        type_ok
            && alias["ended"].as_bool().unwrap_or(false)
            && (alias["name"].as_str() == Some(credited)
                || alias["sort-name"].as_str() == Some(credited))
    })
}

/// The name (and sort name) a relationship credits `artist` with. Picard's logic in
/// `_relations_to_metadata_target_type_artist`.
pub(super) fn relation_artist_name(
    relation: &Value,
    artist: &Value,
    settings: &MetadataSettings,
) -> Alias {
    let translated = translate_artist(artist, settings);
    let has_translation = translated.name != artist["name"].as_str().unwrap_or_default();
    if !has_translation && let Some(credited) = relation["target-credit"].as_str() {
        let use_credited = !should_standardize(settings.standardize_artist_names, credited, artist);
        if !credited.is_empty() && use_credited && Some(credited) != artist["name"].as_str() {
            return Alias {
                name: credited.to_owned(),
                sort_name: select_sort_name(artist, credited, settings),
            };
        }
    }
    translated
}

// ---- artist credits -----------------------------------------------------------------------

/// Picard's `ArtistCreditInfo`.
pub(super) struct ArtistCredit {
    /// Display name with join phrases (`"Gorillaz feat. Mos Def"`).
    pub name: String,
    pub sort_name: String,
    /// One entry per credited artist, without join phrases.
    pub names: Vec<String>,
    pub sort_names: Vec<String>,
    pub ids: Vec<String>,
    /// ISO country of each artist that has an id, `XX` when unknown.
    pub countries: Vec<String>,
    /// Disambiguation comment of each artist that has an id.
    pub comments: Vec<String>,
}

/// Picard's `artist_credit_from_node`.
pub(super) fn artist_credit_from_node(
    credits: &[Value],
    settings: &MetadataSettings,
) -> ArtistCredit {
    let mut out = ArtistCredit {
        name: String::new(),
        sort_name: String::new(),
        names: Vec::new(),
        sort_names: Vec::new(),
        ids: Vec::new(),
        countries: Vec::new(),
        comments: Vec::new(),
    };
    for credit in credits {
        let artist = &credit["artist"];
        let artist_name = artist["name"].as_str().unwrap_or_default();
        let credited = credit["name"].as_str();
        let use_credited = !should_standardize(
            settings.standardize_artist_names,
            credited.unwrap_or(artist_name),
            artist,
        );
        if let Some(id) = str_field(artist, "id") {
            out.ids.push(id.to_owned());
            out.countries
                .push(str_field(artist, "country").unwrap_or("XX").to_owned());
            out.comments.push(
                artist["disambiguation"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        let translated = translate_artist(artist, settings);
        let has_translation = translated.name != artist_name;
        let (name, sort_name) = match credited {
            Some(credited) if !has_translation && use_credited => (
                credited.to_owned(),
                select_sort_name(artist, credited, settings),
            ),
            _ => (translated.name.clone(), translated.sort_name.clone()),
        };
        let join = credit["joinphrase"].as_str().unwrap_or_default();
        out.name.push_str(&name);
        out.name.push_str(join);
        out.sort_name.push_str(&sort_name);
        out.sort_name.push_str(join);
        if settings.always_standardize_multivalue_artist {
            out.names.push(translated.name);
            out.sort_names.push(translated.sort_name);
        } else {
            out.names.push(name);
            out.sort_names.push(sort_name);
        }
    }
    out
}

/// Picard's `artist_credit_to_metadata`: release-level credits become `albumartist*`, track and
/// recording credits `artist*`. Returns the credit for further use (ids, comments).
pub(super) fn artist_credit_to_metadata(
    credits: &[Value],
    m: &mut Metadata,
    release: bool,
    settings: &MetadataSettings,
) {
    let ids: Vec<String> = credits
        .iter()
        .filter_map(|c| c["artist"]["id"].as_str().map(str::to_owned))
        .collect();
    let credit = artist_credit_from_node(credits, settings);
    let comment = if credit.comments.len() == 1 {
        credit.comments[0].clone()
    } else {
        String::new()
    };
    if release {
        m.set_all("musicbrainz_albumartistid", ids);
        m.set("albumartist", credit.name);
        m.set_all("albumartists", credit.names.clone());
        m.set("albumartistsort", credit.sort_name);
        m.set_all("~albumartists", credit.names);
        m.set_all("~albumartists_sort", credit.sort_names);
        m.set_all("~albumartists_countries", credit.countries);
        m.set_all("~albumartists_comments", credit.comments);
        m.set("~albumartistcomment", comment);
    } else {
        m.set_all("musicbrainz_artistid", ids);
        m.set("artist", credit.name);
        m.set("artistsort", credit.sort_name);
        m.set_all("artists", credit.names);
        m.set_all("~artists_sort", credit.sort_names);
        m.set_all("~artists_countries", credit.countries);
        m.set_all("~artists_comments", credit.comments);
        m.set("~artistcomment", comment);
    }
}

/// Picard's translation of a recording, release or work title: the title of the preferred
/// locale's alias, unless a script exception (artist names setting) applies to `title`.
pub(super) fn translate_title(
    title: &str,
    node: &Value,
    settings: &MetadataSettings,
) -> Option<String> {
    let alias = find_localized_alias(array(node, "aliases"), &settings.translation_locales)?;
    (!skip_translation_due_to_scripts(title, settings)).then_some(alias.name)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings() -> MetadataSettings {
        MetadataSettings::default()
    }

    fn alias(name: &str, sort: &str, locale: &str, primary: bool, type_id: &str) -> Value {
        json!({"name": name, "sort-name": sort, "locale": locale, "primary": primary, "type-id": type_id, "ended": false})
    }

    #[test]
    fn localized_alias_prefers_exact_locale_then_root_and_needs_primary() {
        let aliases = vec![
            alias("Beyonce", "Beyonce", "en", true, ALIAS_TYPE_ARTIST_NAME),
            alias(
                "Beyoncé US",
                "Beyoncé US",
                "en_US",
                true,
                ALIAS_TYPE_ARTIST_NAME,
            ),
            alias("Not primary", "x", "fr", false, ALIAS_TYPE_ARTIST_NAME),
        ];
        let en_us = find_localized_alias(&aliases, &["en_US".into()]).expect("exact");
        assert_eq!(en_us.name, "Beyoncé US");
        let en = find_localized_alias(&aliases, &["en".into()]).expect("root");
        assert_eq!(en.name, "Beyonce");
        assert_eq!(
            find_localized_alias(&aliases, &["en_GB".into()])
                .expect("falls back to root of en_GB")
                .name,
            "Beyonce"
        );
        assert_eq!(
            find_localized_alias(&aliases, &["fr".into()]),
            None,
            "non-primary aliases are ignored"
        );
        assert_eq!(find_localized_alias(&[], &["en".into()]), None);
    }

    #[test]
    fn better_alias_type_wins_for_the_same_locale() {
        let aliases = vec![
            alias("Legal", "Legal", "ja", true, ALIAS_TYPE_LEGAL_NAME),
            alias("Artist", "Artist", "ja", true, ALIAS_TYPE_ARTIST_NAME),
        ];
        assert_eq!(
            find_localized_alias(&aliases, &["ja".into()])
                .expect("alias")
                .name,
            "Artist"
        );
    }

    #[test]
    fn variations_mode_keeps_ended_alias_credits() {
        let artist = json!({"id": "a", "name": "Prince", "sort-name": "Prince", "aliases": [
            {"name": "The Artist", "sort-name": "Artist, The", "type-id": ALIAS_TYPE_ARTIST_NAME, "ended": true}
        ]});
        assert!(
            !should_standardize(StandardizeArtistNames::Variations, "The Artist", &artist),
            "name change"
        );
        assert!(
            should_standardize(StandardizeArtistNames::Variations, "Prince!", &artist),
            "variation"
        );
        assert!(should_standardize(
            StandardizeArtistNames::All,
            "The Artist",
            &artist
        ));
        assert!(!should_standardize(
            StandardizeArtistNames::None,
            "Prince!",
            &artist
        ));
    }

    #[test]
    fn credit_uses_credited_name_with_alias_sort_name_or_translation() {
        let credits = vec![
            json!({
                "name": "宇多田ヒカル", "joinphrase": " & ",
                "artist": {"id": "a1", "name": "Utada Hikaru", "sort-name": "Utada, Hikaru", "country": "JP",
                           "disambiguation": "singer", "aliases": [
                               {"name": "宇多田ヒカル", "sort-name": "宇多田, ヒカル", "locale": "ja", "primary": true,
                                "type-id": ALIAS_TYPE_ARTIST_NAME, "ended": false},
                               {"name": "Hikki", "sort-name": "Hikki", "locale": "en", "primary": true,
                                "type-id": ALIAS_TYPE_ARTIST_NAME, "ended": false}]}
            }),
            json!({"name": "Other", "joinphrase": "", "artist": {"id": "a2", "name": "Other", "sort-name": "Other"}}),
        ];
        // Credited names: the credited name is kept, with the sort name of the alias spelled like it.
        let credited = MetadataSettings {
            standardize_artist_names: StandardizeArtistNames::None,
            ..settings()
        };
        let c = artist_credit_from_node(&credits, &credited);
        assert_eq!(c.name, "宇多田ヒカル & Other");
        assert_eq!(c.sort_name, "宇多田, ヒカル & Other");
        assert_eq!(c.names, ["宇多田ヒカル", "Other"]);
        assert_eq!(c.countries, ["JP", "XX"]);
        assert_eq!(c.comments, ["singer", ""]);
        assert_eq!(c.ids, ["a1", "a2"]);
        // Translating to English picks the "Hikki" alias.
        let t = MetadataSettings {
            translate_artist_names: true,
            ..credited.clone()
        };
        let c = artist_credit_from_node(&credits, &t);
        assert_eq!(c.name, "Hikki & Other");
        assert_eq!(c.sort_name, "Hikki & Other");
        // Script exception: Japanese names written mostly in CJK are left alone.
        let skip = MetadataSettings {
            translate_artist_names: true,
            translate_artist_names_script_exception: true,
            script_exceptions: vec![crate::settings::ScriptException {
                script: "CJK".into(),
                weighting: 10,
            }],
            ..credited.clone()
        };
        let mut japanese = credits.clone();
        japanese[0]["artist"]["name"] = json!("宇多田ヒカル");
        assert_eq!(
            artist_credit_from_node(&japanese, &skip).name,
            "宇多田ヒカル & Other",
            "exception applies to the artist's name"
        );
        assert_eq!(
            artist_credit_from_node(&credits, &skip).name,
            "Hikki & Other",
            "a Latin-script name is still translated"
        );
        // Standardising everything uses the artist's own name; `always_standardize_multivalue_artist`
        // keeps `artists` standard even when the display name is credited.
        let std_all = MetadataSettings {
            standardize_artist_names: StandardizeArtistNames::All,
            ..settings()
        };
        assert_eq!(
            artist_credit_from_node(&credits, &std_all).name,
            "Utada Hikaru & Other"
        );
        assert_eq!(
            artist_credit_from_node(&credits, &settings()).name,
            "Utada Hikaru & Other",
            "variations"
        );
        let multi = MetadataSettings {
            standardize_artist_names: StandardizeArtistNames::None,
            always_standardize_multivalue_artist: true,
            ..settings()
        };
        let c = artist_credit_from_node(&credits, &multi);
        assert_eq!(c.name, "宇多田ヒカル & Other");
        assert_eq!(c.names, ["Utada Hikaru", "Other"]);
    }

    #[test]
    fn translate_from_sortname_applies_without_alias() {
        let artist = json!({"id": "a", "name": "宇多田ヒカル", "sort-name": "Utada, Hikaru"});
        let s = MetadataSettings {
            translate_artist_names: true,
            translate_from_sortname: true,
            ..settings()
        };
        assert_eq!(translate_artist(&artist, &s).name, "Hikaru Utada");
        let off = MetadataSettings {
            translate_artist_names: true,
            ..settings()
        };
        assert_eq!(translate_artist(&artist, &off).name, "宇多田ヒカル");
    }

    #[test]
    fn credit_tags_cover_release_and_track_variants() {
        let credits = vec![json!({"name": "Solo", "joinphrase": "", "artist": {
            "id": "a1", "name": "Solo", "sort-name": "Solo", "country": "GB", "disambiguation": "uk"}})];
        let mut m = Metadata::new();
        artist_credit_to_metadata(&credits, &mut m, true, &settings());
        assert_eq!(m.get("albumartists"), Some("Solo"));
        assert_eq!(m.get("~albumartists_sort"), Some("Solo"));
        assert_eq!(m.get("~albumartists_countries"), Some("GB"));
        assert_eq!(m.get("~albumartists_comments"), Some("uk"));
        assert_eq!(m.get("~albumartistcomment"), Some("uk"));
        artist_credit_to_metadata(&credits, &mut m, false, &settings());
        assert_eq!(m.get("~artists_countries"), Some("GB"));
        assert_eq!(m.get("~artistcomment"), Some("uk"));
        let two = vec![
            credits[0].clone(),
            json!({"name": "B", "artist": {"id": "a2", "name": "B", "sort-name": "B"}}),
        ];
        artist_credit_to_metadata(&two, &mut m, false, &settings());
        assert!(
            !m.contains("~artistcomment"),
            "only set for a single credited artist"
        );
    }
}
