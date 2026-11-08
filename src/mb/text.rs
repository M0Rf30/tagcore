//! Text helpers used by the MusicBrainz mapping, ported from Picard's `util` and
//! `util/textencoding.py`: Unicode punctuation → ASCII, sort-name reversal, script detection and
//! Amazon ASIN extraction.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use unicode_script::{Script, UnicodeScript};

use crate::py::is_alpha;

/// Picard's `_ADDITIONAL_COMPATIBILITY` (applied before NFKC).
fn additional_compatibility(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{0276}' => "Œ",
        '\u{1d00}' => "A",
        '\u{1d01}' => "Æ",
        '\u{1d04}' => "C",
        '\u{1d05}' => "D",
        '\u{1d07}' => "E",
        '\u{1d0a}' => "J",
        '\u{1d0b}' => "K",
        '\u{1d0d}' => "M",
        '\u{1d0f}' => "O",
        '\u{1d18}' => "P",
        '\u{1d1b}' => "T",
        '\u{1d1c}' => "U",
        '\u{1d20}' => "V",
        '\u{1d21}' => "W",
        '\u{1d22}' => "Z",
        '\u{3007}' => "0",
        '\u{00a0}' | '\u{3000}' => " ",
        '\u{2033}' => "”",
        '\u{ff0f}' => "/",
        _ => return None,
    })
}

/// Picard's `_SIMPLIFY_PUNCTUATION`.
fn simplify_punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{013f}' => "L",
        '\u{0140}' => "l",
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' | '\u{301d}'
        | '\u{301e}' => "\"",
        '\u{00ab}' | '\u{300a}' | '\u{fe3d}' | '\u{226a}' => "<<",
        '\u{00bb}' | '\u{300b}' | '\u{fe3e}' | '\u{226b}' => ">>",
        '\u{2039}' | '\u{3008}' | '\u{fe3f}' => "<",
        '\u{203a}' | '\u{3009}' | '\u{fe40}' => ">",
        '\u{00ad}' | '\u{200b}' => "",
        '\u{2010}'..='\u{2015}' | '\u{fe58}' | '\u{2212}' | '\u{2022}' => "-",
        '\u{fe31}' | '\u{fe32}' | '\u{2223}' => "|",
        '\u{2016}' | '\u{2225}' => "||",
        '\u{2044}' | '\u{2215}' => "/",
        '\u{2045}' | '\u{3014}' | '\u{3018}' | '\u{301a}' | '\u{fe39}' | '\u{fe5d}' => "[",
        '\u{2046}' | '\u{3015}' | '\u{3019}' | '\u{301b}' | '\u{fe3a}' | '\u{fe5e}' => "]",
        '\u{204e}' => "*",
        '\u{fe11}' | '\u{fe51}' | '\u{ff64}' => ",",
        '\u{fe12}' | '\u{ff61}' => ".",
        '\u{ff5f}' | '\u{2985}' => "((",
        '\u{ff60}' | '\u{2986}' => "))",
        '\u{2216}' => "\\",
        _ => return None,
    })
}

/// Picard's `asciipunct`: compatibility characters (NFKC plus a few extras), then Unicode
/// punctuation mapped to its ASCII look-alike.
pub fn asciipunct(text: &str) -> String {
    let mut interim = String::with_capacity(text.len());
    for c in text.chars() {
        match additional_compatibility(c) {
            Some(s) => interim.push_str(s),
            None => interim.push(c),
        }
    }
    let mut out = String::with_capacity(interim.len());
    for c in interim.nfkc() {
        match simplify_punctuation(c) {
            Some(s) => out.push_str(s),
            None => out.push(c),
        }
    }
    out
}

/// Picard's `_reverse_sortname`: `"Last, First"` → `"First Last"` (and 3- and 4-part forms).
fn reverse_sortname(sortname: &str) -> String {
    let chunks: Vec<&str> = sortname.split(',').map(str::trim).collect();
    match chunks.len() {
        2 => format!("{} {}", chunks[1], chunks[0]),
        3 => format!("{} {} {}", chunks[2], chunks[1], chunks[0]),
        4 => format!("{} {}, {} {}", chunks[1], chunks[0], chunks[3], chunks[2]),
        _ => sortname.trim().to_owned(),
    }
}

/// Picard's `translate_from_sortname`: when `name` contains a letter outside the Latin script,
/// reverse the sort name (per collaboration part) to get a Latin-script name; otherwise `name`.
pub fn translate_from_sortname(name: &str, sortname: &str) -> String {
    if !name
        .chars()
        .any(|c| is_alpha(c) && c.script() != Script::Latin)
    {
        return name.to_owned();
    }
    for separator in [" & ", "; ", " and ", " vs. ", " with ", " y "] {
        if sortname.contains(separator) {
            return sortname
                .split(separator)
                .map(reverse_sortname)
                .collect::<Vec<_>>()
                .join(separator);
        }
    }
    reverse_sortname(sortname)
}

fn script_id(script: Script) -> String {
    match script {
        Script::Han => "CJK".to_owned(),
        other => format!("{other:?}").to_uppercase(),
    }
}

/// Picard's `SCRIPT_WEIGHTING_FACTORS`: scripts that need fewer or more characters than Latin.
fn weighting_factor(script_id: &str) -> f64 {
    match script_id {
        "CYRILLIC" => 1.02,
        "GREEK" => 0.92,
        "ARABIC" => 1.08,
        "HEBREW" => 0.85,
        "CJK" => 2.5,
        "HANGUL" => 0.92,
        "HIRAGANA" | "KATAKANA" => 1.77,
        "THAI" => 1.69,
        _ => 1.0,
    }
}

/// Picard's `detect_script_weighted`: share (0..=1, weighted by script) of every script among the
/// letters of `text`, keyed by Picard script id (`LATIN`, `CYRILLIC`, `CJK`, ...).
pub fn detect_script_weighted(text: &str) -> BTreeMap<String, f64> {
    let mut scripts: BTreeMap<String, f64> = BTreeMap::new();
    let mut total = 0.0;
    for c in text.chars().filter(|&c| is_alpha(c)) {
        let id = script_id(c.script());
        let weight = weighting_factor(&id);
        *scripts.entry(id).or_insert(0.0) += weight;
        total += weight;
    }
    for share in scripts.values_mut() {
        *share /= total;
    }
    scripts
}

/// Picard's `_matches_script_exception`: whether any `(script id, threshold percent)` is met.
pub fn matches_script_exception(
    detected: &BTreeMap<String, f64>,
    exceptions: &[(&str, u8)],
) -> bool {
    exceptions.iter().any(|&(script, weighting)| {
        detected
            .get(script)
            .is_some_and(|&share| share >= f64::from(weighting) / 100.0)
    })
}

static AMAZON_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:www.)?(?P<host>.*?)(?::[0-9]+)?/.*/(?P<asin>[0-9B][0-9A-Z]{9})(?:[^0-9A-Z]|$)")
        .expect("static pattern")
});

/// Picard's `parse_amazon_url`: the ASIN of an Amazon product URL.
pub fn amazon_asin(url: &str) -> Option<&str> {
    AMAZON_URL.captures(url)?.name("asin").map(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asciipunct_simplifies_quotes_dashes_and_compatibility_forms() {
        assert_eq!(
            asciipunct("Don’t “stop” – ever…"),
            "Don't \"stop\" - ever..."
        );
        assert_eq!(asciipunct("«a» ‹b› \u{00a0}c\u{200b}"), "<<a>> <b>  c");
        assert_eq!(asciipunct("Ｆｕｌｌ１２"), "Full12");
        assert_eq!(asciipunct("plain ASCII - stays"), "plain ASCII - stays");
    }

    #[test]
    fn sortname_reversal_only_for_non_latin_names() {
        assert_eq!(
            translate_from_sortname("宇多田ヒカル", "Utada, Hikaru"),
            "Hikaru Utada"
        );
        assert_eq!(
            translate_from_sortname("Радио & Ко", "Radio, A & Ko, B"),
            "A Radio & B Ko"
        );
        assert_eq!(translate_from_sortname("Björk", "Björk"), "Björk");
        assert_eq!(
            translate_from_sortname("Ак", "One, Two, Three"),
            "Three Two One"
        );
        assert_eq!(translate_from_sortname("Ак", "A, B, C, D"), "B A, D C");
        assert_eq!(translate_from_sortname("Ак", "Single"), "Single");
    }

    #[test]
    fn script_detection_is_weighted_per_script() {
        let d = detect_script_weighted("Aa Ыы");
        assert!((d["LATIN"] - 2.0 / 4.04).abs() < 1e-9, "{d:?}");
        assert!(d.contains_key("CYRILLIC"));
        let cjk = detect_script_weighted("東京ab");
        assert!(cjk["CJK"] > 0.7, "{cjk:?}");
        assert!(matches_script_exception(&d, &[("CYRILLIC", 50)]));
        assert!(!matches_script_exception(
            &d,
            &[("CYRILLIC", 60), ("GREEK", 1)]
        ));
        assert!(detect_script_weighted("123 !?").is_empty());
    }

    #[test]
    fn amazon_asin_is_extracted_from_product_urls() {
        assert_eq!(
            amazon_asin("https://www.amazon.co.uk/gp/product/B000002UAL"),
            Some("B000002UAL")
        );
        assert_eq!(
            amazon_asin("http://www.amazon.com/dp/product/0123456789?tag=x"),
            Some("0123456789")
        );
        assert_eq!(amazon_asin("https://example.com/"), None);
    }
}
