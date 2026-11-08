//! Metadata and genre options (Picard's "Metadata" and "Genres" pages; `metadata.genres`). Serde field
//! names and defaults are stable: they are the on-disk configuration of the host applications.
//!
//! Metadata options: relationships, artist-name standardisation and translation, release
//! preferences, genres (nested), ratings and the standalone-recordings name. Defaults are
//! Picard 3.0.2's.

use serde::{Deserialize, Serialize};

/// Picard's `StandardizeArtistNames`: whether the artist's own name replaces the name a release
/// credits.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StandardizeArtistNames {
    /// Always use the credited name.
    None,
    /// Use the artist's name, except where the credit is an ended alias (a real name change).
    #[default]
    Variations,
    /// Always use the artist's name.
    All,
}

/// One entry of Picard's `script_exceptions`: names written mostly in `script` are not translated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptException {
    /// Picard script id: `LATIN`, `CYRILLIC`, `GREEK`, `ARABIC`, `HEBREW`, `CJK`, `HANGUL`,
    /// `HIRAGANA`, `KATAKANA` or `THAI`.
    pub script: String,
    /// Minimum weighted share of the name (percent) that makes the exception apply.
    pub weighting: u8,
}

/// Script ids Picard offers for exceptions, with display names (`picard/const/scripts.py`).
pub const SCRIPTS: [(&str, &str); 10] = [
    ("ARABIC", "Arabic"),
    ("CJK", "Chinese"),
    ("CYRILLIC", "Cyrillic"),
    ("GREEK", "Greek"),
    ("HANGUL", "Hangul"),
    ("HEBREW", "Hebrew"),
    ("HIRAGANA", "Hiragana"),
    ("KATAKANA", "Katakana"),
    ("LATIN", "Latin"),
    ("THAI", "Thai"),
];

/// Picard's `nat_name` default.
pub const DEFAULT_NAT_NAME: &str = "[standalone recordings]";

/// Picard's `genres_filter` default.
pub const DEFAULT_GENRES_FILTER: &str = "-seen live\n-favorites\n-fixme\n-owned";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GenresSettings {
    /// Fill `genre` from MusicBrainz genres (Picard `use_genres`).
    pub use_genres: bool,
    /// Request only the logged-in user's genres/tags (`user-genres`, `user-tags`; needs a login).
    pub only_my_genres: bool,
    /// Fall back to the album artists' genres when the release itself has none.
    pub artists_genres: bool,
    /// Use folksonomy tags instead of curated genres.
    pub folksonomy_tags: bool,
    /// Keep genres used at least this many percent as often as the top one (0..=100).
    pub min_genre_usage: u32,
    /// Maximum number of genres (0..=100).
    pub max_genres: u32,
    /// When non-empty, all genres are joined into a single value with this separator.
    pub join_genres: String,
    /// One `+`/`-` pattern per line: `-word`, `+wor*`, `-/regex/`; other lines are ignored.
    pub genres_filter: String,
}

impl Default for GenresSettings {
    fn default() -> Self {
        Self {
            use_genres: false,
            only_my_genres: false,
            artists_genres: false,
            folksonomy_tags: false,
            min_genre_usage: 90,
            max_genres: 5,
            join_genres: String::new(),
            genres_filter: DEFAULT_GENRES_FILTER.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetadataSettings {
    /// Use release (and release-group) relationships (`release_ars`).
    pub release_ars: bool,
    /// Also load track/recording relationships (`track_ars`; slower).
    pub track_ars: bool,
    pub standardize_artist_names: StandardizeArtistNames,
    /// Keep `artists`/`~artists_sort` standardised even when the credited name is used for the
    /// display string.
    pub always_standardize_multivalue_artist: bool,
    /// Use the standardised instrument names instead of the credited ones.
    pub standardize_instruments: bool,
    /// Use the standardised vocal names instead of the credited ones.
    pub standardize_vocals: bool,
    pub translate_artist_names: bool,
    /// Skip translating names that are mostly written in one of `script_exceptions`.
    pub translate_artist_names_script_exception: bool,
    /// Without a matching alias, derive a Latin name by reversing the sort name.
    pub translate_from_sortname: bool,
    pub translate_track_titles: bool,
    pub translate_album_titles: bool,
    /// Alias locales to prefer, most preferred first (`en`, `en_US`, ...).
    pub translation_locales: Vec<String>,
    /// Scripts exempt from artist-name translation (when enabled).
    pub script_exceptions: Vec<ScriptException>,
    /// Album artist name used for the "Various Artists" artist.
    pub va_name: String,
    /// Convert Unicode punctuation to ASCII.
    pub convert_punctuation: bool,
    /// Album title of the "standalone recordings" pseudo-album.
    pub nat_name: String,
    /// Request the user's ratings (`user-ratings`; needs a login) and map them to `~rating`.
    pub enable_ratings: bool,
    pub genres: GenresSettings,
    /// Preferred release countries (ISO 3166-1), most preferred first.
    pub preferred_countries: Vec<String>,
    /// Preferred medium formats (`CD`, `Digital Media`, `12" Vinyl`, ...), most preferred first.
    pub preferred_formats: Vec<String>,
    /// Preferred release types (`Album`, `Live`, ...), most preferred first.
    pub preferred_types: Vec<String>,
    /// Release types that are only chosen when nothing else matches.
    pub discouraged_types: Vec<String>,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            release_ars: true,
            track_ars: false,
            standardize_artist_names: StandardizeArtistNames::default(),
            always_standardize_multivalue_artist: false,
            standardize_instruments: true,
            standardize_vocals: true,
            translate_artist_names: false,
            translate_artist_names_script_exception: false,
            translate_from_sortname: false,
            translate_track_titles: false,
            translate_album_titles: false,
            translation_locales: vec!["en".to_owned()],
            script_exceptions: Vec::new(),
            va_name: "Various Artists".to_owned(),
            convert_punctuation: false,
            nat_name: DEFAULT_NAT_NAME.to_owned(),
            enable_ratings: false,
            genres: GenresSettings::default(),
            preferred_countries: Vec::new(),
            preferred_formats: Vec::new(),
            preferred_types: Vec::new(),
            discouraged_types: Vec::new(),
        }
    }
}

impl MetadataSettings {
    /// Whether any request needs the user's credentials (`only_my_genres` or ratings).
    pub fn requires_auth(&self) -> bool {
        (self.genres.use_genres && self.genres.only_my_genres) || self.enable_ratings
    }
}
