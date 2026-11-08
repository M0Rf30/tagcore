//! Round trips through `file::load` / `file::save` on tiny real files (1 s of silence made once
//! with ffmpeg, see `tests/fixtures/file_io`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use tagcore::file::{self, WriteOptions};
use tagcore::metadata::{CoverImage, ImageType, Metadata};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/file_io")
        .join(name)
}

/// A copy of fixture `name` in a private temp dir.
fn copy(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "tagcore-file_io-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let to = dir.join(name);
    fs::copy(fixture(name), &to).unwrap();
    to
}

fn tagged() -> Metadata {
    let mut md = Metadata::new();
    md.set("title", "Song");
    md.set_all("artist", vec!["A".into(), "B".into()]);
    md.set("album", "LP");
    md.set(
        "musicbrainz_recordingid",
        "11111111-1111-4111-8111-111111111111",
    );
    md.images = vec![CoverImage::new(
        fs::read(fixture("cover.jpg")).unwrap(),
        "image/jpeg",
        ImageType::Front,
        "Front",
    )];
    md
}

#[test]
fn flac_and_mp3_round_trip() {
    for name in ["silence.flac", "silence.mp3"] {
        let path = copy(name);
        assert!(file::is_supported(&path));
        file::save(&path, &tagged(), &WriteOptions::default(), true).unwrap();
        let loaded = file::load(&path).unwrap();
        let md = loaded.metadata;
        assert_eq!(md.get("title"), Some("Song"), "{name}");
        assert_eq!(md.get_all("artist"), ["A", "B"], "{name}");
        assert_eq!(
            md.get("musicbrainz_recordingid"),
            Some("11111111-1111-4111-8111-111111111111"),
            "{name}"
        );
        assert_eq!(md.images.len(), 1, "{name}");
        assert!(loaded.properties.duration.is_some(), "{name}");
        assert!(md.length().is_some(), "{name}");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

#[test]
fn unsupported_files_are_rejected() {
    let path = copy("cover.jpg");
    assert!(!file::is_supported(&path));
    assert!(file::load(&path).is_err());
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
