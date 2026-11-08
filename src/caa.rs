//! Cover Art Archive front covers (<https://musicbrainz.org/doc/Cover_Art_Archive/API>).

use crate::http::{self, status_code};
use crate::image::mime_of;
use crate::{CoverImage, Error, ImageType, Result};

const CAA_BASE: &str = "https://coverartarchive.org";
/// A 500 px thumbnail is tens of KB and a scan of the original rarely exceeds a few MB; the cap
/// only guards against a broken server.
const MAX_COVER_BYTES: usize = 20 * 1024 * 1024;

/// The release's front cover: the 500 px rendition if it exists, else the original. `Ok(None)`
/// when the release has no front cover; an error when the archive fails or answers with
/// something that is not an image (an HTML error page, say).
pub fn fetch_front_cover(release_id: &str) -> Result<Option<CoverImage>> {
    for variant in ["front-500", "front"] {
        let url = format!(
            "{CAA_BASE}/release/{}/{variant}",
            http::encode_query(release_id)
        );
        // The archive answers with a redirect to archive.org, which the transport follows.
        let (data, content_type) = match http::get_bytes(&url, &[]) {
            Ok(response) => response,
            Err(e) if status_code(&e) == Some(404) => continue,
            Err(e) => return Err(e),
        };
        if data.len() > MAX_COVER_BYTES {
            return Err(Error::Response(format!(
                "cover art larger than {MAX_COVER_BYTES} bytes"
            )));
        }
        let mime = mime_of(&content_type, &data);
        if !mime.starts_with("image/") {
            return Err(Error::Response(
                "Cover Art Archive returned something that is not an image".to_owned(),
            ));
        }
        let mut image = CoverImage::new(data, mime, ImageType::Front, "");
        image.source = format!("CAA: {url}");
        return Ok(Some(image));
    }
    Ok(None)
}
