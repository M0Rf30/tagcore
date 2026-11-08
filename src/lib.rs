//! Shared music-tagging core.
//!
//! Tag names follow MusicBrainz Picard's internal variable names (`title`, `albumartist`,
//! `musicbrainz_recordingid`, ...). Names starting with `~` are hidden variables (e.g. `~length`)
//! that are never written to files.
//!
//! The crate has no HTTP client: online lookups go through the process-global
//! [`http::Transport`] the host application installs with [`http::set_transport`].

pub mod acoustid;
pub mod cluster;
pub mod discogs;
pub mod error;
pub mod file;
pub mod fingerprint;
pub mod genre;
pub mod http;
pub mod image;
pub mod keys;
pub mod matching;
pub mod mb;
pub mod metadata;
pub mod py;
pub mod pyre;
pub mod release;
pub mod settings;

pub use error::{Error, Result};
pub use metadata::{CoverImage, ImageType, Metadata};
pub use release::{Release, ReleaseCandidate, ReleaseId, ReleaseQuery, Source, Track};
