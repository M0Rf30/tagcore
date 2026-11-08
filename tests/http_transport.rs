//! The helpers over a mock [`Transport`]. The transport is process-global, so one test walks
//! through every state in order.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tagcore::Error;
use tagcore::http::{self, Response, ServiceConfig, Transport};

#[derive(Default)]
struct Mock {
    calls: AtomicU32,
    seen: Mutex<Vec<String>>,
}

impl Mock {
    fn respond(status: u16, body: &str) -> tagcore::Result<Response> {
        Ok(Response {
            status,
            content_type: "application/json".into(),
            body: body.as_bytes().to_vec(),
        })
    }
}

impl Transport for Mock {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> tagcore::Result<Response> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push(format!("GET {url} {}", headers.len()));
        match url {
            u if u.ends_with("/flaky") && n == 0 => Self::respond(503, "busy"),
            u if u.ends_with("/flaky") => Self::respond(200, r#"{"ok":true}"#),
            u if u.ends_with("/missing") => Self::respond(404, "  not\n found  "),
            u if u.ends_with("/bad") => Self::respond(200, "<html>"),
            _ => Self::respond(200, "{}"),
        }
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> tagcore::Result<Response> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("POST {url} {}", form.len()));
        Self::respond(200, r#"{"status":"ok"}"#)
    }
}

#[test]
fn helpers_go_through_the_installed_transport() {
    match http::get_json("https://x/y", &[]) {
        Err(Error::Http(msg)) => assert_eq!(msg, "no HTTP transport configured"),
        other => panic!("expected the missing-transport error, got {other:?}"),
    }

    let mock = Arc::new(Mock::default());
    http::set_transport(mock.clone());

    // 503 is retried once.
    let json = http::get_json("https://x/flaky", &[("A", "b")]).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);

    // Non-2xx surfaces as a status error with a whitespace-collapsed excerpt.
    let err = http::get_json("https://x/missing", &[]).unwrap_err();
    assert_eq!(http::status_code(&err), Some(404));
    assert_eq!(
        err.to_string(),
        "HTTP error: status 404 for https://x/missing: not found"
    );

    assert!(matches!(
        http::get_json("https://x/bad", &[]),
        Err(Error::Response(_))
    ));

    let (body, mime) = http::get_bytes("https://x/anything", &[]).unwrap();
    assert_eq!(
        (body.as_slice(), mime.as_str()),
        (b"{}".as_slice(), "application/json")
    );

    let json = http::post_form_json("https://x/post", &[("a", "1"), ("b", "2")]).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(
        mock.seen
            .lock()
            .unwrap()
            .contains(&"POST https://x/post 2".to_owned())
    );

    // Service configuration drives the base URLs.
    assert_eq!(http::ws_base(), "https://musicbrainz.org/ws/2");
    http::configure(ServiceConfig {
        server_origin: "http://mb.local:5000".into(),
        submission_origin: "https://musicbrainz.org".into(),
        mb_request_delay: std::time::Duration::ZERO,
    });
    assert_eq!(http::ws_base(), "http://mb.local:5000/ws/2");
    assert_eq!(http::submission_ws_base(), "https://musicbrainz.org/ws/2");
    assert_eq!(http::server_origin(), "http://mb.local:5000");
}
