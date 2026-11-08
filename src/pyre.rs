//! Python `re` compatibility on top of the `regex` crate: replacement templates (`\1`, `\g<name>`,
//! escapes) as used by `re.sub`, and Picard's `pattern_as_regex`.
//!
//! Patterns the `regex` crate cannot express (look-around, back-references) fail to compile; the
//! script functions treat that like Python's `re.error`.

use regex::{Regex, RegexBuilder};

/// Compiles a Python-style pattern. `\Z` (end of text in Python) becomes `\z`.
pub fn compile(pattern: &str) -> Option<Regex> {
    let mut translated = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('Z') => translated.push_str("\\z"),
                Some(next) => {
                    translated.push('\\');
                    translated.push(next);
                }
                None => translated.push('\\'),
            }
        } else {
            translated.push(c);
        }
    }
    RegexBuilder::new(&translated)
        .size_limit(10 * (1 << 20))
        .build()
        .ok()
}

/// Port of Picard's `pattern_as_regex(pattern, allow_wildcards=False)`: `/regex/flags` (flags
/// `i`, `m`) is a regular expression, anything else matches literally.
pub fn pattern_as_regex(pattern: &str) -> Option<Regex> {
    let plain = pattern.trim_end_matches(['i', 'm']);
    let inner = plain
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'));
    match inner {
        Some(inner) if plain.chars().count() > 2 => {
            let flags = &pattern[plain.len()..];
            let mut prefix = String::new();
            if flags.contains('i') {
                prefix.push_str("(?i)");
            }
            if flags.contains('m') {
                prefix.push_str("(?m)");
            }
            compile(&format!("{prefix}{inner}"))
        }
        _ => Regex::new(&regex::escape(pattern)).ok(),
    }
}

#[derive(Debug)]
enum Piece {
    Literal(String),
    Group(usize),
}

fn is_octal(c: char) -> bool {
    ('0'..='7').contains(&c)
}

/// Parses a `re.sub` replacement string against `re`. `None` when Python would raise `re.error`
/// (bad escape, unknown or out-of-range group).
fn parse_template(repl: &str, re: &Regex) -> Option<Vec<Piece>> {
    let groups = re.captures_len(); // includes group 0
    let chars: Vec<char> = repl.chars().collect();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    let flush = |literal: &mut String, pieces: &mut Vec<Piece>| {
        if !literal.is_empty() {
            pieces.push(Piece::Literal(std::mem::take(literal)));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' {
            literal.push(c);
            continue;
        }
        let esc = *chars.get(i)?;
        i += 1;
        match esc {
            'g' => {
                if chars.get(i) != Some(&'<') {
                    return None;
                }
                let end = chars[i + 1..].iter().position(|&c| c == '>')? + i + 1;
                let name: String = chars[i + 1..end].iter().collect();
                i = end + 1;
                let index = if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
                    name.parse::<usize>().ok()?
                } else {
                    let valid_ident = name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_')
                        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                    if !valid_ident {
                        return None;
                    }
                    re.capture_names().position(|n| n == Some(name.as_str()))?
                };
                if index >= groups {
                    return None;
                }
                flush(&mut literal, &mut pieces);
                pieces.push(Piece::Group(index));
            }
            '0' => {
                let mut value = 0u32;
                let mut taken = 0;
                while taken < 2 && chars.get(i).is_some_and(|&c| is_octal(c)) {
                    value = value * 8 + chars[i].to_digit(8)?;
                    i += 1;
                    taken += 1;
                }
                literal.push(char::from_u32(value & 0xff)?);
            }
            '1'..='9' => {
                let mut digits = String::from(esc);
                if chars.get(i).is_some_and(char::is_ascii_digit) {
                    digits.push(chars[i]);
                    i += 1;
                    if is_octal(esc)
                        && is_octal(digits.chars().nth(1)?)
                        && chars.get(i).is_some_and(|&c| is_octal(c))
                    {
                        digits.push(chars[i]);
                        i += 1;
                        let value = u32::from_str_radix(&digits, 8).ok()?;
                        if value > 0o377 {
                            return None;
                        }
                        literal.push(char::from_u32(value)?);
                        continue;
                    }
                }
                let index: usize = digits.parse().ok()?;
                if index >= groups {
                    return None;
                }
                flush(&mut literal, &mut pieces);
                pieces.push(Piece::Group(index));
            }
            'a' => literal.push('\u{7}'),
            'b' => literal.push('\u{8}'),
            'f' => literal.push('\u{c}'),
            'n' => literal.push('\n'),
            'r' => literal.push('\r'),
            't' => literal.push('\t'),
            'v' => literal.push('\u{b}'),
            '\\' => literal.push('\\'),
            other if other.is_ascii_alphabetic() => return None,
            other => {
                literal.push('\\');
                literal.push(other);
            }
        }
    }
    flush(&mut literal, &mut pieces);
    Some(pieces)
}

/// `re.sub(pattern, repl, text)` (all matches). `None` on a bad replacement template.
pub fn sub(re: &Regex, repl: &str, text: &str) -> Option<String> {
    let pieces = parse_template(repl, re)?;
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let whole = caps.get(0)?;
        out.push_str(&text[last..whole.start()]);
        for piece in &pieces {
            match piece {
                Piece::Literal(s) => out.push_str(s),
                Piece::Group(n) => out.push_str(caps.get(*n).map_or("", |m| m.as_str())),
            }
        }
        last = whole.end();
    }
    out.push_str(&text[last..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub_str(pattern: &str, repl: &str, text: &str) -> Option<String> {
        sub(&compile(pattern)?, repl, text)
    }

    #[test]
    fn templates() {
        assert_eq!(
            sub_str(r"(\w+) (\w+)", r"\2 \1", "hello world").as_deref(),
            Some("world hello")
        );
        assert_eq!(
            sub_str(r"(?P<a>x)", r"[\g<a>\g<0>\g<1>]", "x").as_deref(),
            Some("[xxx]")
        );
        assert_eq!(sub_str("a", r"\n\t\\", "a").as_deref(), Some("\n\t\\"));
        assert_eq!(sub_str("a", r"\-", "a").as_deref(), Some("\\-"));
        assert_eq!(sub_str("a", r"\101", "a").as_deref(), Some("A"));
        assert_eq!(sub_str("a", r"\0", "a").as_deref(), Some("\0"));
        // Unmatched optional groups expand to nothing.
        assert_eq!(sub_str(r"(a)|(b)", r"[\2]", "a").as_deref(), Some("[]"));
        // re.error cases.
        assert_eq!(sub_str("a", r"\q", "a"), None);
        assert_eq!(sub_str("a", r"\1", "a"), None);
        assert_eq!(sub_str("a", r"\g<x>", "a"), None);
        assert_eq!(sub_str("a", "x\\", "a"), None);
        assert_eq!(sub_str("(", "x", "a"), None);
    }

    #[test]
    fn pattern_forms() {
        let r = pattern_as_regex("/^guitars?$/i").expect("regex form");
        assert!(r.is_match("Guitars") && r.is_match("guitar") && !r.is_match("bass guitar"));
        let lit = pattern_as_regex("a.b").expect("literal form");
        assert!(lit.is_match("xa.by") && !lit.is_match("axb"));
        // Too short to be /re/ form: literal.
        assert!(pattern_as_regex("//").expect("literal").is_match("a//b"));
        assert!(pattern_as_regex("/(/").is_none());
    }

    #[test]
    fn python_end_anchor_is_translated() {
        let r = compile(r"a\Z").expect("compiles");
        assert!(r.is_match("ba") && !r.is_match("ab"));
    }
}
