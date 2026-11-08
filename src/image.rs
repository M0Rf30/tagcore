//! Image identification: format, pixel size and MIME type of cover-art bytes.

use std::io::Cursor;

use image::ImageReader;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Image formats (Picard's `ImageFormat`). Only [`ImageFormat::CONVERTIBLE`] ones can be a
/// conversion target; GIF and PDF are recognised sources only.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    #[default]
    Jpeg,
    Png,
    /// Encoded lossless: no pure-Rust lossy WebP encoder exists, so `quality` does not apply.
    WebP,
    Tiff,
    Gif,
    Pdf,
}

impl ImageFormat {
    /// Formats images can be converted to, in the order the options page lists them.
    pub const CONVERTIBLE: [ImageFormat; 4] = [Self::Jpeg, Self::Png, Self::WebP, Self::Tiff];

    pub fn title(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::WebP => "WebP",
            Self::Tiff => "TIFF",
            Self::Gif => "GIF",
            Self::Pdf => "PDF",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Tiff => "image/tiff",
            Self::Gif => "image/gif",
            Self::Pdf => "application/pdf",
        }
    }

    /// File extension without the dot (Picard's first extension of the format).
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Tiff => "tiff",
            Self::Gif => "gif",
            Self::Pdf => "pdf",
        }
    }

    /// The format a MIME type names.
    pub fn from_mime(mime: &str) -> Option<Self> {
        let mime = mime.trim().to_ascii_lowercase();
        [
            Self::Jpeg,
            Self::Png,
            Self::WebP,
            Self::Tiff,
            Self::Gif,
            Self::Pdf,
        ]
        .into_iter()
        .find(|f| {
            f.mime() == mime
                || (*f == Self::Jpeg && mime == "image/jpg")
                || (*f == Self::Tiff && mime == "image/tif")
        })
    }

    pub fn can_convert(self) -> bool {
        Self::CONVERTIBLE.contains(&self)
    }

    /// Whether `quality` is used when encoding.
    pub fn uses_quality(self) -> bool {
        matches!(self, Self::Jpeg)
    }
}

/// What [`identify`] learns from the bytes of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    /// 0 for PDFs, whose pages are not measured.
    pub width: u32,
    pub height: u32,
}

/// The format and pixel size of `data`; PDFs are recognised by their signature.
pub fn identify(data: &[u8]) -> Result<ImageInfo> {
    if data.starts_with(b"%PDF-") {
        return Ok(ImageInfo {
            format: ImageFormat::Pdf,
            width: 0,
            height: 0,
        });
    }
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(Error::Io)?;
    let format = match reader.format() {
        Some(other) => from_image_format(other)
            .ok_or_else(|| Error::Decode(format!("unsupported image format {other:?}")))?,
        None => return Err(Error::Decode("not a recognised image".into())),
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(ImageInfo {
        format,
        width,
        height,
    })
}

fn from_image_format(format: image::ImageFormat) -> Option<ImageFormat> {
    match format {
        image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
        image::ImageFormat::Png => Some(ImageFormat::Png),
        image::ImageFormat::Gif => Some(ImageFormat::Gif),
        image::ImageFormat::WebP => Some(ImageFormat::WebP),
        image::ImageFormat::Tiff => Some(ImageFormat::Tiff),
        _ => None,
    }
}

/// Pixel size of a raster image; `None` for anything [`identify`] rejects or PDFs.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    identify(data)
        .ok()
        .filter(|i| i.width > 0 && i.height > 0)
        .map(|i| (i.width, i.height))
}

/// The MIME type of `data`: the response's `image/*` Content-Type when there is one, else the
/// type sniffed from the bytes (`application/octet-stream` when unknown).
pub fn mime_of(content_type: &str, data: &[u8]) -> String {
    let declared = content_type.trim().to_ascii_lowercase();
    if declared.starts_with("image/") || declared == "application/pdf" {
        return declared;
    }
    if let Ok(info) = identify(data) {
        return info.format.mime().to_owned();
    }
    // A truncated image still has its magic bytes.
    match image::guess_format(data).ok().and_then(from_image_format) {
        Some(format) => format.mime().to_owned(),
        None if declared.is_empty() => "application/octet-stream".to_owned(),
        None => declared,
    }
}

/// File extension (without the dot) for a MIME type; `jpg` for anything unknown.
pub fn extension_for_mime(mime: &str) -> &'static str {
    ImageFormat::from_mime(mime).map_or("jpg", ImageFormat::extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid-colour image of the given size, encoded as `format`.
    fn sample(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([200, 30, 30]),
        ));
        let target = match format {
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::WebP => image::ImageFormat::WebP,
            ImageFormat::Tiff => image::ImageFormat::Tiff,
            ImageFormat::Gif => image::ImageFormat::Gif,
            ImageFormat::Pdf => unreachable!("no pixels in a PDF"),
        };
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, target).unwrap();
        out.into_inner()
    }

    #[test]
    fn identify_reads_format_and_size() {
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Tiff,
        ] {
            let info = identify(&sample(40, 30, format)).unwrap();
            assert_eq!(
                (info.format, info.width, info.height),
                (format, 40, 30),
                "{format:?}"
            );
        }
        assert_eq!(identify(b"%PDF-1.4 rest").unwrap().format, ImageFormat::Pdf);
        assert!(identify(b"<html>not an image</html>").is_err());
        assert_eq!(dimensions(&sample(7, 9, ImageFormat::Png)), Some((7, 9)));
        assert_eq!(dimensions(b"%PDF-1.4"), None);
    }

    #[test]
    fn mime_prefers_an_image_content_type_then_the_bytes() {
        let png = sample(4, 4, ImageFormat::Png);
        assert_eq!(mime_of("image/jpeg", b"whatever"), "image/jpeg");
        assert_eq!(mime_of("Image/PNG", b""), "image/png");
        assert_eq!(mime_of("application/octet-stream", &png), "image/png");
        assert_eq!(mime_of("", &png), "image/png");
        assert_eq!(mime_of("", b"<html>"), "application/octet-stream");
        assert_eq!(mime_of("", b"\xFF\xD8\xFFtruncated"), "image/jpeg");
        assert_eq!(mime_of("text/html", b"<html>"), "text/html");
        assert_eq!(mime_of("application/pdf", b"%PDF-1.7"), "application/pdf");
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
        assert_eq!(extension_for_mime("application/pdf"), "pdf");
        assert_eq!(extension_for_mime("what/ever"), "jpg");
    }
}
