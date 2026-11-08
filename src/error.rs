use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported file: {0}")]
    Unsupported(PathBuf),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("tag write error: {0}")]
    Tagging(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("invalid response: {0}")]
    Response(String),
    #[error("fingerprint error: {0}")]
    Fingerprint(String),
    /// A required setting (e.g. an API token) is missing.
    #[error("configuration error: {0}")]
    Config(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
