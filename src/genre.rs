//! Genre selection: Picard's `TagGenreFilter` and `Track._genres_to_metadata`.
//!
//! MusicBrainz genres and folksonomy tags arrive as `name → vote count`. They are merged over
//! track, release, release group and (optionally) artists, filtered by the user's `+`/`-`
//! patterns, thinned by relative usage and count, title-cased and sorted.

use std::collections::BTreeMap;

use regex::Regex;

use crate::py::titlecase;
use crate::pyre;

/// `name → votes`, the equivalent of Picard's `Counter` of genres or tags.
pub type Counter = BTreeMap<String, i64>;

/// Adds `count` votes for `name` (no-op for an empty name), like `MetadataItem.add_genre`.
pub fn add(counter: &mut Counter, name: &str, count: i64) {
    if !name.is_empty() {
        *counter.entry(name.to_owned()).or_insert(0) += count;
    }
}

/// `counter += other`: counts are summed, entries that end up at or below zero are dropped
/// (Python's `Counter.__iadd__`).
pub fn merge(counter: &mut Counter, other: &Counter) {
    for (name, count) in other {
        *counter.entry(name.clone()).or_insert(0) += count;
    }
    counter.retain(|_, c| *c > 0);
}

/// Parsed genre filter lines. Lines not starting with `+` or `-` are ignored; the rest is a
/// pattern (`/regex/ims`, or a shell-like wildcard pattern), matched case-insensitively.
#[derive(Debug, Default)]
pub struct GenreFilter {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
    errors: Vec<(usize, String)>,
}

impl GenreFilter {
    /// Parses the multi-line filter text. Invalid patterns are recorded in [`Self::errors`] and
    /// otherwise ignored.
    pub fn new(filters: &str) -> Self {
        let mut out = Self::default();
        for (lineno, line) in filters.lines().enumerate() {
            let line = line.trim();
            let Some(sign) = line.chars().next().filter(|c| matches!(c, '+' | '-')) else {
                continue;
            };
            let remain = line[1..].trim();
            if remain.is_empty() {
                continue;
            }
            match pattern_as_regex(remain) {
                Ok(re) if sign == '+' => out.include.push(re),
                Ok(re) => out.exclude.push(re),
                Err(message) => out.errors.push((lineno, message)),
            }
        }
        out
    }

    /// `(0-based line number, message)` of every pattern that failed to compile.
    pub fn errors(&self) -> &[(usize, String)] {
        &self.errors
    }

    /// Whether `tag` is filtered out: an include match always keeps it, otherwise an exclude
    /// match drops it.
    pub fn skip(&self, tag: &str) -> bool {
        if self.include.is_empty() && self.exclude.is_empty() {
            return false;
        }
        if self.include.iter().any(|re| re.is_match(tag)) {
            return false;
        }
        self.exclude.iter().any(|re| re.is_match(tag))
    }

    /// Picard's `TagGenreFilter.filter`: drops skipped names, then every name used less than
    /// `minusage` percent as often as the most used remaining one.
    pub fn filter(&self, counter: &Counter, minusage: u32) -> Counter {
        let mut result: Counter = counter
            .iter()
            .filter(|(name, _)| !self.skip(name))
            .map(|(n, c)| (n.clone(), *c))
            .collect();
        let Some(&top) = result.values().max() else {
            return result;
        };
        for (name, &count) in counter {
            // Python floor division; counts are positive here.
            if 100 * count / top < i64::from(minusage) {
                result.remove(name);
            }
        }
        result
    }
}

/// Picard's `Track._genres_to_metadata`: the `genre` values for `genres`.
///
/// `limit` is the maximum number of genres (`None`: unlimited; 0 yields nothing). With a
/// non-empty `join_with` all genres are joined into one value.
pub fn genres_to_metadata(
    genres: &Counter,
    limit: Option<usize>,
    minusage: u32,
    filters: &str,
    join_with: &str,
) -> Vec<String> {
    if limit == Some(0) {
        return Vec::new();
    }
    let mut positive = genres.clone();
    positive.retain(|_, c| *c > 0);
    if positive.is_empty() {
        return Vec::new();
    }
    let filtered = GenreFilter::new(filters).filter(&positive, minusage);
    let mut ranked: Vec<(&String, &i64)> = filtered.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(limit.unwrap_or(usize::MAX));
    let mut list: Vec<String> = ranked
        .into_iter()
        .map(|(name, _)| titlecase(name))
        .collect();
    list.sort();
    if join_with.is_empty() {
        list
    } else {
        vec![list.join(join_with)]
    }
}

/// Picard's `pattern_as_regex(pattern, allow_wildcards=True, flags=re.IGNORECASE)`.
fn pattern_as_regex(pattern: &str) -> Result<Regex, String> {
    let plain = pattern.trim_end_matches(['i', 'm']);
    if plain.chars().count() > 2 && plain.starts_with('/') && plain.ends_with('/') {
        let extra = &pattern[plain.len()..];
        let mut source = String::from("(?i)");
        if extra.contains('m') {
            source.push_str("(?m)");
        }
        source.push_str(&plain[1..plain.len() - 1]);
        pyre::compile(&source).ok_or_else(|| {
            Regex::new(&source[4..]).err().map_or_else(
                || "invalid regular expression".to_owned(),
                |e| e.to_string(),
            )
        })
    } else {
        let source = format!("(?i)^{}$", wildcards_to_regex(pattern));
        Regex::new(&source).map_err(|e| e.to_string())
    }
}

/// Picard's `wildcards_to_regex_pattern`: `*`, `?`, `[...]` and backslash escapes.
fn wildcards_to_regex(pattern: &str) -> String {
    let mut regex = String::new();
    let mut group: Option<String> = None;
    let mut escape = false;
    for c in pattern.chars() {
        let part: String;
        if let Some(g) = group.as_mut() {
            let mut piece = c.to_string();
            let mut literal_close = c == ']';
            if escape {
                if matches!(c, '\\' | '[' | ']') {
                    piece = format!("\\{c}");
                    literal_close = false;
                } else {
                    g.push_str("\\\\");
                }
                escape = false;
            }
            if literal_close {
                g.push(']');
                part = group.take().unwrap_or_default();
            } else if piece == "\\" {
                escape = true;
                continue;
            } else {
                g.push_str(&piece);
                continue;
            }
        } else if escape {
            part = if matches!(c, '*' | '?' | '\\' | '[' | ']') {
                format!("\\{c}")
            } else {
                regex::escape(&format!("\\{c}"))
            };
            escape = false;
        } else if c == '\\' {
            escape = true;
            continue;
        } else if c == '[' {
            group = Some("[".to_owned());
            continue;
        } else if c == '*' {
            part = ".*".to_owned();
        } else if c == '?' {
            part = ".".to_owned();
        } else {
            part = regex::escape(&c.to_string());
        }
        regex.push_str(&part);
    }
    // An unclosed `[` is a literal bracket; the rest is interpreted again.
    if let Some(g) = group {
        regex.push_str("\\[");
        regex.push_str(&wildcards_to_regex(&g[1..]));
    }
    regex
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "-seen live\n-favorites\n-fixme\n-owned";

    fn counter(items: &[(&str, i64)]) -> Counter {
        items.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect()
    }

    #[test]
    fn default_filter_skips_listed_tags_case_insensitively() {
        let f = GenreFilter::new(DEFAULT);
        assert!(f.skip("Seen Live"));
        assert!(f.skip("owned"));
        assert!(!f.skip("rock"));
        assert!(f.errors().is_empty());
    }

    #[test]
    fn wildcards_regex_and_include_precedence() {
        let f = GenreFilter::new(
            "# comment\n!comment\ncomment\n-*word\n+wor?\n-w*rd\n-/^x.y+$/\n+ \n-",
        );
        assert!(f.skip("sword"), "ends with word");
        assert!(!f.skip("word"), "+wor? includes word first");
        assert!(f.skip("weird"), "-w*rd");
        assert!(f.skip("xyyy"), "regex");
        assert!(!f.skip("comment"), "ignored lines do nothing");
        let g = GenreFilter::new("-wor[dk]\n-fo\\?");
        assert!(g.skip("work") && g.skip("word") && !g.skip("wore"));
        assert!(g.skip("fo?") && !g.skip("foo"), "escaped ?");
        let h = GenreFilter::new("-a[bc");
        assert!(h.skip("a[bc"), "unclosed group is literal");
    }

    #[test]
    fn invalid_regex_is_reported_with_its_line() {
        let f = GenreFilter::new("-ok\n-/(unclosed/");
        assert_eq!(f.errors().len(), 1);
        assert_eq!(f.errors()[0].0, 1);
        assert!(f.skip("ok"));
    }

    #[test]
    fn minimum_usage_is_relative_to_the_top_genre() {
        let c = counter(&[("rock", 10), ("pop", 9), ("jazz", 8), ("seen live", 100)]);
        // top (after dropping "seen live") is 10; floor 90 % keeps rock (100) and pop (90).
        assert_eq!(
            genres_to_metadata(&c, None, 90, DEFAULT, ""),
            ["Pop", "Rock"]
        );
        assert_eq!(
            genres_to_metadata(&c, None, 80, DEFAULT, ""),
            ["Jazz", "Pop", "Rock"]
        );
    }

    #[test]
    fn limit_prefers_votes_then_name_and_output_is_sorted_and_titlecased() {
        let c = counter(&[
            ("hip hop", 5),
            ("rock", 5),
            ("alternative rock", 5),
            ("pop", 4),
        ]);
        assert_eq!(
            genres_to_metadata(&c, Some(2), 0, "", ""),
            ["Alternative Rock", "Hip Hop"]
        );
        assert_eq!(
            genres_to_metadata(&c, Some(0), 0, "", ""),
            Vec::<String>::new()
        );
        assert_eq!(
            genres_to_metadata(&c, None, 0, "", " / "),
            ["Alternative Rock / Hip Hop / Pop / Rock"]
        );
    }

    #[test]
    fn non_positive_counts_are_ignored_and_merge_drops_them() {
        let c = counter(&[("rock", 0), ("pop", -2)]);
        assert!(genres_to_metadata(&c, None, 0, "", "").is_empty());
        let mut a = counter(&[("rock", 2)]);
        merge(&mut a, &counter(&[("rock", 3), ("pop", 1), ("bad", -1)]));
        assert_eq!(a, counter(&[("rock", 5), ("pop", 1)]));
        let mut n = Counter::new();
        add(&mut n, "", 5);
        add(&mut n, "x", 1);
        add(&mut n, "x", 1);
        assert_eq!(n, counter(&[("x", 2)]));
    }
}
