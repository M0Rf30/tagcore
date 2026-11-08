//! Cross-format contract: for every writable container, `load(save(metadata)) == metadata` with
//! the full Picard field set, and saving an empty `Metadata` removes every field and picture.
//! Fixtures are tiny files in `tests/fixtures/file_io`.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use tagcore::file::{self, WriteOptions};
use tagcore::keys;
use tagcore::metadata::{CoverImage, ImageType, Metadata};

fn copy(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "tagcore-round_trip-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let to = dir.join(name);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/file_io")
            .join(name),
        &to,
    )
    .unwrap();
    to
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 30]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn rich() -> Metadata {
    let mut m = Metadata::new();
    m.set(keys::TITLE, "Tïtle ✓");
    m.set_all(keys::ARTIST, ["Artist A".into(), "Artist B".into()]);
    m.set(keys::ALBUM, "Albüm");
    m.set(keys::ALBUMARTIST, "Album Artist");
    m.set(keys::DATE, "2001-05-03");
    m.set(keys::ORIGINALDATE, "1999-01-02");
    m.set_all(keys::GENRE, ["Electronic".into(), "Ambient".into()]);
    m.set(keys::TRACKNUMBER, "3");
    m.set(keys::TOTALTRACKS, "12");
    m.set(keys::DISCNUMBER, "1");
    m.set(keys::TOTALDISCS, "2");
    m.set(keys::COMPOSER, "Composer");
    m.set(keys::COMMENT, "A comment");
    m.set(keys::LYRICS, "La la la");
    m.set(keys::LABEL, "Label");
    m.set(keys::CATALOGNUMBER, "CAT-001");
    m.set(keys::BARCODE, "0123456789012");
    m.set(keys::ISRC, "DEABC0100001");
    m.set(keys::MEDIA, "CD");
    m.set(keys::RELEASECOUNTRY, "DE");
    m.set(keys::COMPILATION, "1");
    m.set(
        keys::MUSICBRAINZ_RECORDINGID,
        "11111111-1111-1111-1111-111111111111",
    );
    m.set(
        keys::MUSICBRAINZ_TRACKID,
        "22222222-2222-2222-2222-222222222222",
    );
    m.set(
        keys::MUSICBRAINZ_ALBUMID,
        "33333333-3333-3333-3333-333333333333",
    );
    m.set_all(
        keys::MUSICBRAINZ_ARTISTID,
        [
            "44444444-4444-4444-4444-444444444444".into(),
            "55555555-5555-5555-5555-555555555555".into(),
        ],
    );
    m.set(
        keys::MUSICBRAINZ_ALBUMARTISTID,
        "66666666-6666-6666-6666-666666666666",
    );
    m.set(
        keys::MUSICBRAINZ_RELEASEGROUPID,
        "77777777-7777-7777-7777-777777777777",
    );
    m.set(keys::ACOUSTID_ID, "88888888-8888-8888-8888-888888888888");
    m.set(keys::REPLAYGAIN_TRACK_GAIN, "-6.50 dB");
    m.set("custom_field", "custom value");
    m.images.push(CoverImage::new(
        png(),
        "image/png",
        ImageType::Front,
        String::new(),
    ));
    m
}

/// `(key, values)` pairs that differ between `want` and `got`, hidden keys ignored.
fn diff(want: &Metadata, got: &Metadata) -> Vec<String> {
    let mut out = Vec::new();
    let mut names: Vec<&str> = want.visible_keys().chain(got.visible_keys()).collect();
    names.sort_unstable();
    names.dedup();
    for k in names {
        if want.get_all(k) != got.get_all(k) {
            out.push(format!(
                "{k}: want {:?}, got {:?}",
                want.get_all(k),
                got.get_all(k)
            ));
        }
    }
    let pics = |m: &Metadata| {
        m.images
            .iter()
            .map(|p| (p.kind, p.mime.clone(), p.data.to_vec()))
            .collect::<Vec<_>>()
    };
    if pics(want) != pics(got) {
        out.push(format!(
            "pictures: want {} got {}",
            want.images.len(),
            got.images.len()
        ));
    }
    out
}

fn round_trip(name: &str) {
    let path = copy(name);
    let md = rich();
    let options = WriteOptions::default();
    file::save(&path, &md, &options, false).unwrap_or_else(|e| panic!("{name}: save: {e}"));
    let back = file::load(&path)
        .unwrap_or_else(|e| panic!("{name}: load: {e}"))
        .metadata;
    let d = diff(&md, &back);
    assert!(d.is_empty(), "{name} round trip:\n{}", d.join("\n"));

    file::save(&path, &Metadata::new(), &options, false)
        .unwrap_or_else(|e| panic!("{name}: clear: {e}"));
    let cleared = file::load(&path).unwrap().metadata;
    let d = diff(&Metadata::new(), &cleared);
    assert!(d.is_empty(), "{name} clear:\n{}", d.join("\n"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn ogg_vorbis() {
    round_trip("plain.ogg");
}

#[test]
fn ogg_opus() {
    round_trip("tagged.opus");
}

#[test]
fn mp4() {
    round_trip("tagged.m4a");
}

#[test]
fn wav() {
    round_trip("plain.wav");
}

#[test]
fn aiff() {
    round_trip("plain.aiff");
}

#[test]
fn wavpack() {
    round_trip("plain.wv");
}

/// Hidden `~` keys (e.g. `~length`, set by `load`) are never written.
#[test]
fn hidden_keys_are_not_written() {
    let path = copy("plain.wv");
    let mut md = Metadata::new();
    md.set(keys::TITLE, "T");
    md.set("~secret", "x");
    file::save(&path, &md, &WriteOptions::default(), false).unwrap();
    let back = file::load(&path).unwrap().metadata;
    assert_eq!(back.get("~secret"), None);
    assert_eq!(back.get(keys::TITLE), Some("T"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
