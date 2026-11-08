//! Genre and folksonomy-tag votes of MusicBrainz entities, and Picard's `Track` logic that turns
//! them into `genre`, `~genres` and `~folksonomy_tags`.

use serde_json::Value;

use super::array;
use crate::genre::{self, Counter};
use crate::metadata::Metadata;
use crate::settings::MetadataSettings;

/// Votes of one entity: curated genres and free-form folksonomy tags.
#[derive(Debug, Clone, Default)]
pub(super) struct Votes {
    pub genres: Counter,
    pub tags: Counter,
}

impl Votes {
    /// Picard's `add_genres_from_node`: `tags`/`genres` carry vote counts, the user's own
    /// `user-tags`/`user-genres` count as one vote each.
    pub fn add_from_node(&mut self, node: &Value) {
        let count = |entry: &Value| entry["count"].as_i64().unwrap_or(1);
        for tag in array(node, "tags") {
            genre::add(
                &mut self.tags,
                tag["name"].as_str().unwrap_or_default(),
                count(tag),
            );
        }
        for tag in array(node, "user-tags") {
            genre::add(&mut self.tags, tag["name"].as_str().unwrap_or_default(), 1);
        }
        for tag in array(node, "genres") {
            genre::add(
                &mut self.genres,
                tag["name"].as_str().unwrap_or_default(),
                count(tag),
            );
        }
        for tag in array(node, "user-genres") {
            genre::add(
                &mut self.genres,
                tag["name"].as_str().unwrap_or_default(),
                1,
            );
        }
    }

    pub fn of(node: &Value) -> Self {
        let mut votes = Self::default();
        votes.add_from_node(node);
        votes
    }
}

/// Votes collected while mapping an album: release, release group and album artists.
#[derive(Debug, Default)]
pub(super) struct AlbumVotes {
    pub release: Votes,
    pub release_group: Votes,
    pub artists: Vec<Votes>,
}

/// Votes of one track: the recording and its artists.
#[derive(Debug, Default)]
pub(super) struct TrackVotes {
    pub recording: Votes,
    pub artists: Vec<Votes>,
}

/// Picard's `_merge_folksonomy_tags`: track + release + release group; when that is empty and
/// `artists_genres` is set, the artists' votes (the track's own artists for a Various Artists
/// release, else the album artists').
fn merged(
    pick: fn(&Votes) -> &Counter,
    track: &TrackVotes,
    album: &AlbumVotes,
    various_artists: bool,
    settings: &MetadataSettings,
) -> Counter {
    let mut all = Counter::new();
    for votes in [&track.recording, &album.release, &album.release_group] {
        genre::merge(&mut all, pick(votes));
    }
    if all.is_empty() && settings.genres.artists_genres {
        let artists = if various_artists {
            &track.artists
        } else {
            &album.artists
        };
        for artist in artists {
            genre::merge(&mut all, pick(artist));
        }
    }
    all
}

/// Picard's `Track._convert_folksonomy_tags_to_genre`, `_add_folksonomy_tags_variable` and
/// `_add_genres_variable`; a no-op unless `use_genres`.
pub(super) fn apply_genres(
    m: &mut Metadata,
    track: &TrackVotes,
    album: &AlbumVotes,
    various_artists: bool,
    settings: &MetadataSettings,
) {
    let g = &settings.genres;
    if !g.use_genres {
        return;
    }
    let genres = merged(|v| &v.genres, track, album, various_artists, settings);
    let tags = merged(|v| &v.tags, track, album, various_artists, settings);
    let source = if g.folksonomy_tags { &tags } else { &genres };
    m.set_all(
        "genre",
        genre::genres_to_metadata(
            source,
            Some(g.max_genres as usize),
            g.min_genre_usage,
            &g.genres_filter,
            &g.join_genres,
        ),
    );
    // `tags - genres`: only the votes tags have in excess of genres.
    let only_tags = tags
        .iter()
        .filter(|(name, count)| **count > genres.get(*name).copied().unwrap_or(0))
        .map(|(n, _)| n.clone());
    m.set_all("~folksonomy_tags", only_tags);
    m.set_all("~genres", genres.keys().cloned());
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings() -> MetadataSettings {
        let mut s = MetadataSettings::default();
        s.genres.use_genres = true;
        s
    }

    #[test]
    fn votes_from_all_four_node_kinds() {
        let v = Votes::of(&json!({
            "tags": [{"name": "rock", "count": 3}], "user-tags": [{"name": "mine"}],
            "genres": [{"name": "alternative rock", "count": 2}], "user-genres": [{"name": "art rock"}]
        }));
        assert_eq!(v.tags["rock"], 3);
        assert_eq!(v.tags["mine"], 1);
        assert_eq!(v.genres["alternative rock"], 2);
        assert_eq!(v.genres["art rock"], 1);
    }

    #[test]
    fn genre_is_merged_over_track_release_and_group() {
        let track = TrackVotes {
            recording: Votes::of(&json!({"genres": [{"name": "rock", "count": 5}]})),
            artists: vec![],
        };
        let album = AlbumVotes {
            release: Votes::of(
                &json!({"genres": [{"name": "rock", "count": 5}, {"name": "pop", "count": 9}]}),
            ),
            release_group: Votes::of(&json!({"genres": [{"name": "jazz", "count": 1}]})),
            artists: vec![],
        };
        let mut m = Metadata::new();
        apply_genres(&mut m, &track, &album, false, &settings());
        // rock 10, pop 9, jazz 1 → 90 % floor keeps rock and pop.
        assert_eq!(m.get_all("genre"), ["Pop", "Rock"]);
        assert_eq!(m.get_all("~genres"), ["jazz", "pop", "rock"]);
    }

    #[test]
    fn folksonomy_tags_replace_genres_and_the_hidden_tag_lists_the_excess() {
        let album = AlbumVotes {
            release: Votes::of(&json!({
                "genres": [{"name": "rock", "count": 2}],
                "tags": [{"name": "rock", "count": 2}, {"name": "seen live", "count": 7}, {"name": "90s", "count": 2}]})),
            ..AlbumVotes::default()
        };
        let mut s = settings();
        s.genres.folksonomy_tags = true;
        s.genres.min_genre_usage = 0;
        let mut m = Metadata::new();
        apply_genres(&mut m, &TrackVotes::default(), &album, false, &s);
        assert_eq!(
            m.get_all("genre"),
            ["90s", "Rock"],
            "'seen live' is filtered by the default filter"
        );
        assert_eq!(m.get_all("~folksonomy_tags"), ["90s", "seen live"]);
    }

    #[test]
    fn artist_genres_are_a_fallback_and_va_uses_track_artists() {
        let album = AlbumVotes {
            artists: vec![Votes::of(
                &json!({"genres": [{"name": "folk", "count": 1}]}),
            )],
            ..AlbumVotes::default()
        };
        let track = TrackVotes {
            artists: vec![Votes::of(
                &json!({"genres": [{"name": "punk", "count": 1}]}),
            )],
            ..TrackVotes::default()
        };
        let mut s = settings();
        let mut m = Metadata::new();
        apply_genres(&mut m, &track, &album, false, &s);
        assert!(!m.contains("genre"), "fallback off");
        s.genres.artists_genres = true;
        apply_genres(&mut m, &track, &album, false, &s);
        assert_eq!(m.get_all("genre"), ["Folk"]);
        apply_genres(&mut m, &track, &album, true, &s);
        assert_eq!(m.get_all("genre"), ["Punk"]);
    }

    #[test]
    fn disabled_genres_leave_metadata_untouched() {
        let mut m = Metadata::new();
        let track = TrackVotes {
            recording: Votes::of(&json!({"genres": [{"name": "rock", "count": 1}]})),
            artists: vec![],
        };
        apply_genres(
            &mut m,
            &track,
            &AlbumVotes::default(),
            false,
            &MetadataSettings::default(),
        );
        assert!(m.is_empty());
    }
}
