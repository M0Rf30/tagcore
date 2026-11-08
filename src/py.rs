//! Helpers reproducing the Python semantics Picard's script functions rely on: `str.strip`,
//! `int()`/`float()` parsing, slicing with negative indexes, `str(float)`, `isalpha` and the
//! titlecase of `picard.util`.

use unicode_general_category::{GeneralCategory as Gc, get_general_category};

/// Python's `str.isspace` for one character (Unicode whitespace plus the ASCII separators
/// `\x1c`..`\x1f`).
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `str.isalpha` for one character: Unicode categories `L*`.
pub fn is_alpha(c: char) -> bool {
    matches!(
        get_general_category(c),
        Gc::UppercaseLetter
            | Gc::LowercaseLetter
            | Gc::TitlecaseLetter
            | Gc::ModifierLetter
            | Gc::OtherLetter
    )
}

/// Python `int(text)` for decimal text: optional surrounding whitespace, sign, ASCII digits with
/// single underscores between digits. Values that do not fit `i128` are rejected.
pub fn parse_int(text: &str) -> Option<i128> {
    let t = strip(text);
    let (negative, digits) = match t.as_bytes().first()? {
        b'+' => (false, &t[1..]),
        b'-' => (true, &t[1..]),
        _ => (false, t),
    };
    if !valid_digit_run(digits) {
        return None;
    }
    let mut value: i128 = 0;
    for b in digits.bytes().filter(|b| *b != b'_') {
        value = value.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    Some(if negative { -value } else { value })
}

/// Non-empty ASCII digits where `_` may only sit between two digits.
fn valid_digit_run(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.iter().enumerate().all(|(i, &c)| {
            c.is_ascii_digit()
                || (c == b'_'
                    && i > 0
                    && i + 1 < b.len()
                    && b[i - 1].is_ascii_digit()
                    && b[i + 1].is_ascii_digit())
        })
}

/// Python `float(text)`: surrounding whitespace, sign, decimal/exponent forms, `inf`, `infinity`,
/// `nan` (any case) and underscores between digits.
pub fn parse_float(text: &str) -> Option<f64> {
    let t = strip(text);
    if t.is_empty() {
        return None;
    }
    let b = t.as_bytes();
    let mut cleaned = String::with_capacity(t.len());
    for (i, c) in t.char_indices() {
        if c == '_' {
            let ok =
                i > 0 && i + 1 < t.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit();
            if !ok {
                return None;
            }
        } else {
            cleaned.push(c);
        }
    }
    // Rust additionally accepts forms Python rejects only through the checks above; both accept
    // "inf", "infinity" and "nan" case-insensitively.
    cleaned.parse::<f64>().ok()
}

/// Python `str(float)` (`repr`): shortest round-trip digits, `1.0`, `1e+16`, `1.5e-05`, `inf`, `nan`.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exp) {
        if exp >= 0 {
            let int_len = exp as usize + 1;
            if digits.len() <= int_len {
                format!("{sign}{digits}{}.0", "0".repeat(int_len - digits.len()))
            } else {
                format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
            }
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize))
        }
    } else {
        let frac = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!(
            "{sign}{}{frac}e{esign}{:02}",
            &digits[..1],
            exp.unsigned_abs()
        )
    }
}

/// Resolves Python slice bounds (`s[start:end]`, step 1) for a sequence of `len` items.
pub fn slice_bounds(len: usize, start: Option<i64>, end: Option<i64>) -> (usize, usize) {
    let len_i = len as i64;
    let norm = |i: i64| -> usize {
        if i < 0 {
            (len_i + i).max(0) as usize
        } else {
            i.min(len_i) as usize
        }
    };
    let s = start.map_or(0, norm);
    let e = end.map_or(len, norm);
    if s >= e {
        (s.min(len), s.min(len))
    } else {
        (s, e)
    }
}

/// `text[start:end]` by characters.
pub fn slice_chars(text: &str, start: Option<i64>, end: Option<i64>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let (s, e) = slice_bounds(chars.len(), start, end);
    chars[s..e].iter().collect()
}

/// `items[start:end]`.
pub fn slice_vec<T: Clone>(items: &[T], start: Option<i64>, end: Option<i64>) -> Vec<T> {
    let (s, e) = slice_bounds(items.len(), start, end);
    items[s..e].to_vec()
}

/// Python `items[index]` with negative indexes; `None` when out of range.
pub fn index_of(len: usize, index: i128) -> Option<usize> {
    let len = len as i128;
    let i = if index < 0 { index + len } else { index };
    (0..len).contains(&i).then_some(i as usize)
}

/// `str.capitalize()` of a single character (title-case mapping).
fn title_char(c: char) -> String {
    match c {
        'ß' => "Ss".into(),
        '\u{149}' => "\u{2bc}N".into(),
        '\u{1f0}' => "J\u{30c}".into(),
        '\u{1c4}'..='\u{1c6}' => "\u{1c5}".into(),
        '\u{1c7}'..='\u{1c9}' => "\u{1c8}".into(),
        '\u{1ca}'..='\u{1cc}' => "\u{1cb}".into(),
        '\u{1f1}'..='\u{1f3}' => "\u{1f2}".into(),
        '\u{fb00}' => "Ff".into(),
        '\u{fb01}' => "Fi".into(),
        '\u{fb02}' => "Fl".into(),
        '\u{fb03}' => "Ffi".into(),
        '\u{fb04}' => "Ffl".into(),
        '\u{fb05}' | '\u{fb06}' => "St".into(),
        _ => c.to_uppercase().collect(),
    }
}

/// Word boundary test of `picard.util.iswbound`: space separators, modifier symbols, punctuation.
fn is_word_boundary(c: char) -> bool {
    matches!(
        get_general_category(c),
        Gc::SpaceSeparator
            | Gc::ModifierSymbol
            | Gc::ConnectorPunctuation
            | Gc::DashPunctuation
            | Gc::OpenPunctuation
            | Gc::ClosePunctuation
            | Gc::InitialPunctuation
            | Gc::FinalPunctuation
            | Gc::OtherPunctuation
    )
}

/// `picard.util.titlecase`: capitalizes the first letter after each word boundary, keeping the
/// rest of every word as is. An apostrophe directly after a letter does not start a new word.
pub fn titlecase(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let Some(&first) = chars.first() else {
        return String::new();
    };
    let mut out = title_char(first);
    let mut capital = false;
    for i in 1..chars.len() {
        let t = chars[i];
        if matches!(t, '’' | '\'') && is_alpha(chars[i - 1]) {
            capital = false;
            out.push(t);
        } else if is_word_boundary(t) {
            capital = true;
            out.push(t);
        } else if capital && is_alpha(t) {
            capital = false;
            out.push_str(&title_char(t));
        } else {
            capital = false;
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_parsing_follows_python() {
        assert_eq!(parse_int(" 42 "), Some(42));
        assert_eq!(parse_int("+7"), Some(7));
        assert_eq!(parse_int("-0"), Some(0));
        assert_eq!(parse_int("1_000"), Some(1000));
        assert_eq!(parse_int("007"), Some(7));
        for bad in [
            "", "1__0", "_1", "1_", "1.0", "0x10", "1e3", "-", "+-1", "a",
        ] {
            assert_eq!(parse_int(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn float_parsing_and_repr() {
        assert_eq!(parse_float(" 1.5 "), Some(1.5));
        assert_eq!(parse_float("1e3"), Some(1000.0));
        assert_eq!(parse_float(".5"), Some(0.5));
        assert_eq!(parse_float("5."), Some(5.0));
        assert_eq!(parse_float("1_0.5"), Some(10.5));
        assert!(parse_float("-INF").is_some_and(|f| f.is_infinite() && f < 0.0));
        assert!(parse_float("NaN").is_some_and(f64::is_nan));
        for bad in ["", "1_", "1._5", "e5", "1,5", "abc", "0x1p3"] {
            assert_eq!(parse_float(bad), None, "{bad:?}");
        }
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(-2.5), "-2.5");
        assert_eq!(float_repr(100.0), "100.0");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1.5e16), "1.5e+16");
        assert_eq!(float_repr(123_456_789_012_345.0), "123456789012345.0");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(1.5e-7), "1.5e-07");
        assert_eq!(float_repr(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(float_repr(f64::INFINITY), "inf");
        assert_eq!(float_repr(f64::NAN), "nan");
        assert_eq!(float_repr(-0.0), "-0.0");
    }

    #[test]
    fn slicing_matches_python() {
        assert_eq!(slice_chars("abcdef", None, Some(-2)), "abcd");
        assert_eq!(slice_chars("abcdef", Some(-2), None), "ef");
        assert_eq!(slice_chars("abcdef", Some(-0), None), "abcdef");
        assert_eq!(slice_chars("abcdef", Some(2), Some(100)), "cdef");
        assert_eq!(slice_chars("abcdef", Some(4), Some(2)), "");
        assert_eq!(slice_chars("abcdef", Some(-100), Some(2)), "ab");
        assert_eq!(slice_chars("äöü", Some(1), Some(2)), "ö");
        assert_eq!(slice_vec(&[1, 2, 3, 4], Some(1), Some(-1)), vec![2, 3]);
        assert_eq!(index_of(3, -1), Some(2));
        assert_eq!(index_of(3, 3), None);
        assert_eq!(index_of(3, -4), None);
    }

    #[test]
    fn titlecase_follows_picard() {
        assert_eq!(titlecase("hello world"), "Hello World");
        assert_eq!(titlecase("children's music"), "Children's Music");
        assert_eq!(titlecase("don’t stop"), "Don’t Stop");
        assert_eq!(titlecase("rock-n-roll (live)"), "Rock-N-Roll (Live)");
        assert_eq!(titlecase("mIxEd cASE"), "MIxEd CASE");
        assert_eq!(titlecase("ß test"), "Ss Test");
        assert_eq!(titlecase(""), "");
        // The first character never starts a capitalised word, only later boundaries do.
        assert_eq!(titlecase("'tis the season"), "'tis The Season");
        assert_eq!(titlecase("a.b"), "A.B");
    }

    #[test]
    fn whitespace_matches_python() {
        assert_eq!(strip("\u{1c} x \u{a0}"), "x");
        assert_eq!(rstrip("x \t\n"), "x");
        assert!(is_alpha('é') && is_alpha('Ω') && !is_alpha('1') && !is_alpha('_'));
    }
}
