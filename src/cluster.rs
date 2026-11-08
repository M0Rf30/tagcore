//! Grouping unmatched files into album clusters (Picard "Cluster").

use crate::matching::{normalize, string_similarity};
use crate::metadata::Metadata;

const ALBUM_SIMILARITY: f32 = 0.85;
const VARIOUS_ARTISTS: &str = "Various Artists";

/// A proposed cluster: indices into the input slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterGroup {
    /// Most common album title among members (original spelling).
    pub album: String,
    /// Most common album artist (falls back to artist; "Various Artists" when members disagree
    /// and no album artist is set).
    pub artist: String,
    pub members: Vec<usize>,
}

struct Entry {
    index: usize,
    album: String,
    albumartist: Option<String>,
    artist: Option<String>,
}

struct Group {
    entries: Vec<Entry>,
}

impl Group {
    fn accepts(&self, e: &Entry) -> bool {
        self.entries.iter().any(|m| {
            let album_ok = normalize(&m.album) == normalize(&e.album)
                || string_similarity(&m.album, &e.album) >= ALBUM_SIMILARITY;
            // Album artists only separate groups when both sides state one; track artists never
            // do (compilations).
            let artist_ok = match (&m.albumartist, &e.albumartist) {
                (Some(a), Some(b)) => string_similarity(a, b) >= ALBUM_SIMILARITY,
                _ => true,
            };
            album_ok && artist_ok
        })
    }
}

/// Most frequent string by normalised form, original spelling of its first occurrence; ties go
/// to the earliest.
fn most_common<'a>(values: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut counts: Vec<(String, &str, usize)> = Vec::new();
    for v in values {
        let key = normalize(v);
        match counts.iter_mut().find(|(k, ..)| *k == key) {
            Some(c) => c.2 += 1,
            None => counts.push((key, v, 1)),
        }
    }
    let mut best: Option<&(String, &str, usize)> = None;
    for c in &counts {
        if best.is_none_or(|b| c.2 > b.2) {
            best = Some(c);
        }
    }
    best.map(|c| c.1.to_owned())
}

fn last_component(dir: &str) -> &str {
    dir.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
}

/// Groups files by normalised album title (case/whitespace/punctuation/diacritics-insensitive,
/// fuzzy within a similarity of 0.85) and by album artist. Files without an album tag fall back
/// to their parent directory name. Files that cannot be grouped are omitted.
pub fn cluster(files: &[&Metadata], dirs: &[Option<&str>]) -> Vec<ClusterGroup> {
    let mut groups: Vec<Group> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let album = file
            .get("album")
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .or_else(|| {
                dirs.get(index)
                    .copied()
                    .flatten()
                    .map(last_component)
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
            });
        let Some(album) = album else { continue };
        let entry = Entry {
            index,
            album: album.to_owned(),
            albumartist: file.get("albumartist").map(str::to_owned),
            artist: file.get("artist").map(str::to_owned),
        };
        match groups.iter_mut().find(|g| g.accepts(&entry)) {
            Some(g) => g.entries.push(entry),
            None => groups.push(Group {
                entries: vec![entry],
            }),
        }
    }

    let mut out: Vec<ClusterGroup> = groups
        .into_iter()
        .map(|g| {
            let album = most_common(g.entries.iter().map(|e| e.album.as_str())).unwrap_or_default();
            let artist = most_common(g.entries.iter().filter_map(|e| e.albumartist.as_deref()))
                .or_else(|| {
                    let mut distinct: Vec<String> = g
                        .entries
                        .iter()
                        .filter_map(|e| e.artist.as_deref())
                        .map(normalize)
                        .collect();
                    distinct.sort_unstable();
                    distinct.dedup();
                    if distinct.len() > 1 {
                        Some(VARIOUS_ARTISTS.to_owned())
                    } else {
                        most_common(g.entries.iter().filter_map(|e| e.artist.as_deref()))
                    }
                })
                .unwrap_or_default();
            ClusterGroup {
                album,
                artist,
                members: g.entries.iter().map(|e| e.index).collect(),
            }
        })
        .collect();
    out.sort_by_cached_key(|g| {
        (
            g.artist.to_lowercase(),
            g.album.to_lowercase(),
            g.members.first().copied(),
        )
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(pairs: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::new();
        for (k, v) in pairs {
            m.set(k, *v);
        }
        m
    }

    #[test]
    fn groups_two_albums_despite_case_and_spacing_and_dir_fallback() {
        let files = [
            md(&[
                ("album", "Dark Side of the Moon"),
                ("albumartist", "Pink Floyd"),
            ]),
            md(&[("album", "Greatest Hits"), ("albumartist", "Queen")]),
            md(&[
                ("album", "dark side  of the moon!"),
                ("albumartist", "pink floyd"),
            ]),
            md(&[
                ("album", "Dark Side of the Moon"),
                ("albumartist", "Pink Floyd"),
            ]),
            md(&[("album", "Greatest  Hits"), ("albumartist", "Queen")]),
            md(&[("title", "untagged")]),
            md(&[("title", "nothing")]),
        ];
        let dirs = [
            None,
            None,
            None,
            None,
            None,
            Some("/music/Greatest Hits"),
            None,
        ];
        let refs: Vec<&Metadata> = files.iter().collect();
        let g = cluster(&refs, &dirs);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].artist, "Pink Floyd");
        assert_eq!(g[0].album, "Dark Side of the Moon");
        assert_eq!(g[0].members, vec![0, 2, 3]);
        assert_eq!(g[1].artist, "Queen");
        assert_eq!(g[1].members, vec![1, 4, 5]);
    }

    #[test]
    fn different_album_artists_stay_apart_and_compilations_merge() {
        let files = [
            md(&[("album", "Best Of"), ("albumartist", "Abba")]),
            md(&[("album", "Best Of"), ("albumartist", "Blondie")]),
            md(&[("album", "Now 1"), ("artist", "A")]),
            md(&[("album", "Now 1"), ("artist", "B")]),
        ];
        let refs: Vec<&Metadata> = files.iter().collect();
        let g = cluster(&refs, &[None; 4]);
        assert_eq!(g.len(), 3);
        let now = g.iter().find(|g| g.album == "Now 1").expect("now");
        assert_eq!(now.artist, "Various Artists");
        assert_eq!(now.members, vec![2, 3]);
    }

    #[test]
    fn artist_falls_back_to_track_artist() {
        let files = [
            md(&[("album", "X"), ("artist", "Solo")]),
            md(&[("album", "X"), ("artist", "solo")]),
        ];
        let refs: Vec<&Metadata> = files.iter().collect();
        let g = cluster(&refs, &[]);
        assert_eq!(
            g,
            vec![ClusterGroup {
                album: "X".into(),
                artist: "Solo".into(),
                members: vec![0, 1]
            }]
        );
    }
}
