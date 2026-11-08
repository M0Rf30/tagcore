//! Front-cover fetch over a mock transport (the transport is process-global, so one test walks
//! through every case in order).

use std::io::Cursor;
use std::sync::Arc;

use tagcore::http::{self, Response, Transport};
use tagcore::{ImageType, caa};

struct Mock;

fn png() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(3, 2));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> tagcore::Result<Response> {
    Ok(Response {
        status,
        content_type: content_type.into(),
        body,
    })
}

impl Transport for Mock {
    fn get(&self, url: &str, _headers: &[(&str, &str)]) -> tagcore::Result<Response> {
        match url {
            // No 500 px rendition: the original is fetched.
            u if u.contains("/aaaa/front-500") => reply(404, "", Vec::new()),
            u if u.contains("/aaaa/front") => reply(200, "application/octet-stream", png()),
            u if u.contains("/bbbb/front-500") => reply(200, "image/png", png()),
            u if u.contains("/cccc/") => reply(404, "", Vec::new()),
            _ => reply(200, "text/html", b"<html>".to_vec()),
        }
    }

    fn post_form(&self, _url: &str, _form: &[(&str, &str)]) -> tagcore::Result<Response> {
        unreachable!()
    }
}

#[test]
fn front_cover_prefers_500_falls_back_and_validates() {
    http::set_transport(Arc::new(Mock));

    let thumb = caa::fetch_front_cover("bbbb").unwrap().unwrap();
    assert_eq!((thumb.kind, thumb.mime.as_str()), (ImageType::Front, "image/png"));
    assert_eq!((thumb.width, thumb.height), (3, 2));

    let full = caa::fetch_front_cover("aaaa").unwrap().unwrap();
    assert_eq!(full.mime, "image/png");

    assert!(caa::fetch_front_cover("cccc").unwrap().is_none());
    assert!(caa::fetch_front_cover("dddd").is_err());
}
