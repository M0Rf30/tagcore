//! Picard-style multi-valued tag map plus embedded images.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Lowercases `key` only when it is not already lowercase, so the common lookup does not allocate.
fn norm(key: &str) -> Cow<'_, str> {
    if key.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(key.to_ascii_lowercase())
    } else {
        Cow::Borrowed(key)
    }
}

/// Hidden variable holding the length in milliseconds (Picard `~length`).
pub const LENGTH: &str = "~length";

/// Multi-valued tag map keyed by Picard internal tag names. Keys are lowercase. Empty value
/// lists are never stored. Keys starting with `~` are hidden and never written to files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    tags: BTreeMap<String, Vec<String>>,
    pub images: Vec<CoverImage>,
}

/// The Cover Art Archive's image types (<https://musicbrainz.org/doc/Cover_Art/Types>) plus
/// Picard's pseudo type `unknown` for images the archive left untyped.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ImageType {
    Front,
    Back,
    Booklet,
    Medium,
    Tray,
    Liner,
    Spine,
    Track,
    Other,
    Obi,
    Sticker,
    Poster,
    Watermark,
    RawUnedited,
    MatrixRunout,
    Top,
    Bottom,
    Unknown,
}

impl ImageType {
    /// Every type, in the Cover Art Archive's own order, then the `unknown` pseudo type.
    pub const ALL: [ImageType; 18] = [
        Self::Front,
        Self::Back,
        Self::Booklet,
        Self::Medium,
        Self::Obi,
        Self::Spine,
        Self::Track,
        Self::Other,
        Self::Tray,
        Self::Sticker,
        Self::Poster,
        Self::Liner,
        Self::Watermark,
        Self::RawUnedited,
        Self::MatrixRunout,
        Self::Top,
        Self::Bottom,
        Self::Unknown,
    ];

    /// The lowercase name the Cover Art Archive and Picard's settings use (`raw/unedited`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
            Self::Booklet => "booklet",
            Self::Medium => "medium",
            Self::Tray => "tray",
            Self::Liner => "liner",
            Self::Spine => "spine",
            Self::Track => "track",
            Self::Other => "other",
            Self::Obi => "obi",
            Self::Sticker => "sticker",
            Self::Poster => "poster",
            Self::Watermark => "watermark",
            Self::RawUnedited => "raw/unedited",
            Self::MatrixRunout => "matrix/runout",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Unknown => "unknown",
        }
    }

    /// The type named `name` (case-insensitive); `None` for names the archive does not define.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(name))
    }
}

/// One image of a release, a file's tags or the cover pane.
///
/// `data` is what gets embedded in tags. Everything after `description` is bookkeeping added by
/// the host's cover art pipeline; build values with [`CoverImage::new`] and set
/// what differs. Two images are equal when their bytes and main type are (Picard's rule), so a
/// downloaded image identical to the embedded one does not mark a file as changed.
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub data: Arc<[u8]>,
    /// e.g. `image/jpeg`.
    pub mime: String,
    /// The main type (Picard's `maintype`): Front for anything front-like.
    pub kind: ImageType,
    pub description: String,
    /// Every type of the image, e.g. `Front` + `Booklet`. Empty when the source reports no
    /// types (MP4 `covr`, URLs): such images are fronts and `kind` is their only type.
    pub types: Vec<ImageType>,
    /// Whether the image counts as the release's front cover (Picard's `is_front_image`).
    pub is_front: bool,
    /// Pixel size of `data`; 0 when unknown.
    pub width: u32,
    pub height: u32,
    /// Where it came from, e.g. `CAA: https://…`, `LOCAL: /path`, `URL: …`; empty for tags.
    pub source: String,
    /// `false` for images that cannot live in tags (PDFs); they can still be saved as files.
    pub taggable: bool,
    /// Version processed for external files (resize/convert-for-files settings); `None` when
    /// the files get `data` as it is.
    pub file_variant: Option<Arc<CoverImage>>,
    /// Raster thumbnail shown for images the GUI cannot decode (PDFs).
    pub preview: Option<Arc<[u8]>>,
}

impl PartialEq for CoverImage {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.data == other.data
    }
}

impl CoverImage {
    /// An image with the given bytes, MIME type, main type and comment; the pixel size is
    /// read from the data when it is a decodable raster image.
    pub fn new(
        data: impl Into<Arc<[u8]>>,
        mime: impl Into<String>,
        kind: ImageType,
        description: impl Into<String>,
    ) -> Self {
        let data = data.into();
        let (width, height) = crate::image::dimensions(&data).unwrap_or((0, 0));
        Self {
            data,
            mime: mime.into(),
            kind,
            description: description.into(),
            types: Vec::new(),
            is_front: kind == ImageType::Front,
            width,
            height,
            source: String::new(),
            taggable: true,
            file_variant: None,
            preview: None,
        }
    }

    /// Picard's `is_front_image`.
    pub fn is_front_image(&self) -> bool {
        self.is_front
    }

    /// Picard's `normalized_types`: the image's types with `front` first and the rest sorted,
    /// the front type alone for a front image whose source reports no types, and nothing for
    /// an untyped non-front image.
    pub fn normalized_types(&self) -> Vec<ImageType> {
        if !self.types.is_empty() {
            let mut rest: Vec<ImageType> = self
                .types
                .iter()
                .copied()
                .filter(|t| *t != ImageType::Front)
                .collect();
            rest.sort_unstable_by_key(|t| t.name());
            rest.dedup();
            let mut types = Vec::with_capacity(rest.len() + 1);
            if self.types.contains(&ImageType::Front) {
                types.push(ImageType::Front);
            }
            types.extend(rest);
            types
        } else if self.is_front {
            vec![ImageType::Front]
        } else {
            Vec::new()
        }
    }

    /// What external files get: the file-specific version when processing made one.
    pub fn for_file(&self) -> &CoverImage {
        self.file_variant.as_deref().unwrap_or(self)
    }
}

impl Metadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// First value of `key` (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.tags
            .get(norm(key).as_ref())
            .and_then(|v| v.first())
            .map(String::as_str)
    }

    /// All values of `key` (empty slice if absent).
    pub fn get_all(&self, key: &str) -> &[String] {
        self.tags
            .get(norm(key).as_ref())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Values joined with `"; "` (Picard display convention).
    pub fn display(&self, key: &str) -> String {
        self.get_all(key).join("; ")
    }

    /// Replaces `key` with a single value; an empty string removes the key.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        let key = norm(key).into_owned();
        if value.is_empty() {
            self.tags.remove(&key);
        } else {
            self.tags.insert(key, vec![value]);
        }
    }

    /// Replaces `key` with `values` (empty strings dropped; no values removes the key).
    pub fn set_all(&mut self, key: &str, values: impl IntoIterator<Item = String>) {
        let values: Vec<String> = values.into_iter().filter(|v| !v.is_empty()).collect();
        let key = norm(key).into_owned();
        if values.is_empty() {
            self.tags.remove(&key);
        } else {
            self.tags.insert(key, values);
        }
    }

    /// Appends one value to `key`.
    pub fn add(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.tags
                .entry(norm(key).into_owned())
                .or_default()
                .push(value);
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Vec<String>> {
        self.tags.remove(norm(key).as_ref())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.tags.contains_key(norm(key).as_ref())
    }

    /// Iterates `(key, values)` in key order, hidden (`~`) keys included.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.tags.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// Keys that are written to files (not starting with `~`).
    pub fn visible_keys(&self) -> impl Iterator<Item = &str> {
        self.tags
            .keys()
            .map(String::as_str)
            .filter(|k| !k.starts_with('~'))
    }

    /// Overwrites every key present in `other` (Picard `Metadata.update`). Images are replaced
    /// only if `other` has any.
    pub fn update(&mut self, other: &Metadata) {
        for (k, v) in &other.tags {
            self.tags.insert(k.clone(), v.clone());
        }
        if !other.images.is_empty() {
            self.images = other.images.clone();
        }
    }

    pub fn length(&self) -> Option<Duration> {
        self.get(LENGTH)
            .and_then(|v| v.parse().ok())
            .map(Duration::from_millis)
    }

    pub fn set_length(&mut self, length: Duration) {
        self.set(LENGTH, length.as_millis().to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.images.is_empty()
    }

    /// Whether both hold the same tags (images are not compared).
    pub fn same_tags(&self, other: &Metadata) -> bool {
        self.tags == other.tags
    }
}
