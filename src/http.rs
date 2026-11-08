//! Transport-agnostic HTTP helpers: retry, status errors, JSON parsing and per-host rate limiting.
//!
//! tagcore links no HTTP client. The host application implements [`Transport`] over its own
//! client and installs it once with [`set_transport`]; every online module (MusicBrainz, AcoustID,
//! Discogs, cover art) then goes through the helpers below. Without a transport every request
//! fails with `Error::Http("no HTTP transport configured")`.

use std::sync::{Arc, LazyLock, PoisonError, RwLock};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

use crate::{Error, Result};

/// Extra request headers, e.g. `[("Authorization", "Discogs token=...")]`.
pub type Headers<'a> = &'a [(&'a str, &'a str)];

/// Upper bound for a single response body (full-size cover art can be tens of MiB). Transport
/// implementations enforce it while reading.
pub const MAX_BODY: u64 = 128 * 1024 * 1024;

/// Prefix of the message of an `Error::Http` built from a non-2xx response; [`status_code`] reads
/// the status back from it.
pub const STATUS_PREFIX: &str = "status ";

#[cfg(not(target_arch = "wasm32"))]
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// Bytes of an error response body quoted in `Error::Http`.
const ERROR_EXCERPT: usize = 200;

/// A complete HTTP response. Non-2xx statuses are responses, not errors, at this level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Media type without parameters (`image/jpeg`); empty when the server sent none.
    pub content_type: String,
    pub body: Vec<u8>,
}

/// What tagcore needs from an HTTP client. Implementations must set their own `User-Agent`,
/// follow redirects, apply their timeout and proxy, and cap bodies at [`MAX_BODY`].
pub trait Transport: Send + Sync {
    /// Performs a GET with extra headers; a non-2xx status is returned as a [`Response`], only
    /// transport failures are errors (`Error::Http`).
    fn get(&self, url: &str, headers: Headers<'_>) -> Result<Response>;
    /// POSTs `application/x-www-form-urlencoded` `form`.
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response>;
}

/// Where the MusicBrainz web service lives and how fast it may be called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// `scheme://host[:port]` of the MusicBrainz server (web pages, lookups).
    pub server_origin: String,
    /// `scheme://host[:port]` data (disc IDs, ISRCs, ratings...) is submitted to.
    pub submission_origin: String,
    /// Minimum delay between two requests to the server; zero disables the throttle.
    pub mb_request_delay: Duration,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            server_origin: "https://musicbrainz.org".to_owned(),
            submission_origin: "https://musicbrainz.org".to_owned(),
            mb_request_delay: Duration::from_secs(1),
        }
    }
}

static TRANSPORT: RwLock<Option<Arc<dyn Transport>>> = RwLock::new(None);
static CONFIG: LazyLock<RwLock<ServiceConfig>> = LazyLock::new(Default::default);

/// Installs the process-global transport, replacing any previous one. Requests already in flight
/// keep the transport they started with.
pub fn set_transport(transport: Arc<dyn Transport>) {
    *TRANSPORT.write().unwrap_or_else(PoisonError::into_inner) = Some(transport);
}

/// Sets the MusicBrainz server origins and request delay for every later request.
pub fn configure(config: ServiceConfig) {
    *CONFIG.write().unwrap_or_else(PoisonError::into_inner) = config;
}

fn transport() -> Result<Arc<dyn Transport>> {
    TRANSPORT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or_else(|| Error::Http("no HTTP transport configured".to_owned()))
}

fn with_config<T>(f: impl FnOnce(&ServiceConfig) -> T) -> T {
    f(&CONFIG.read().unwrap_or_else(PoisonError::into_inner))
}

/// Base of the MusicBrainz web service of the configured server, e.g. `https://musicbrainz.org/ws/2`.
pub fn ws_base() -> String {
    format!("{}/ws/2", server_origin())
}

/// `scheme://host[:port]` of the configured MusicBrainz server (for web pages, OAuth, lookups).
pub fn server_origin() -> String {
    with_config(|c| c.server_origin.clone())
}

/// `scheme://host[:port]` data (disc IDs, ISRCs, ratings...) is submitted to.
pub fn submission_origin() -> String {
    with_config(|c| c.submission_origin.clone())
}

/// Web service base of the submission server (`submission_origin()` + `/ws/2`).
pub fn submission_ws_base() -> String {
    format!("{}/ws/2", submission_origin())
}

/// Rate-limit key shared by every request to the configured MusicBrainz server.
const MB_HOST: &str = "musicbrainz.org";

/// Waits the configured MusicBrainz request delay (1 s for official servers).
pub fn throttle_mb() {
    let delay = with_config(|c| c.mb_request_delay);
    if !delay.is_zero() {
        throttle(MB_HOST, delay);
    }
}

/// Throttled GET of a MusicBrainz web service `url` (see [`ws_base`]) parsed as JSON.
pub fn mb_get_json(url: &str) -> Result<serde_json::Value> {
    throttle_mb();
    get_json(url, &[])
}

/// Blocks until at least `interval` has passed since the previous call with the same `host` key,
/// across all threads. MusicBrainz: 1 s. AcoustID: 334 ms. Discogs: 1 s (2.5 s without token).
/// A no-op on wasm32, which has neither threads to block nor a monotonic clock.
#[cfg(not(target_arch = "wasm32"))]
pub fn throttle(host: &'static str, interval: Duration) {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static NEXT_SLOT: LazyLock<Mutex<HashMap<&'static str, Instant>>> =
        LazyLock::new(Default::default);

    // The slot is reserved under the lock, before sleeping, so concurrent callers queue up one
    // `interval` apart instead of all waking at the same time.
    let wait = {
        let mut slots = NEXT_SLOT.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let slot = slots.get(host).map_or(now, |next| (*next).max(now));
        slots.insert(host, slot + interval);
        slot.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

/// See the native implementation; the browser build never blocks.
#[cfg(target_arch = "wasm32")]
pub fn throttle(host: &'static str, interval: Duration) {
    let _ = (host, interval);
}

/// GET `url` and parse JSON. Retries once on HTTP 429/500/502/503/504 after 1 s.
pub fn get_json(url: &str, headers: Headers<'_>) -> Result<serde_json::Value> {
    let response = get(url, headers)?;
    parse_json(&response.body, url)
}

/// GET `url` and return the body bytes and `Content-Type`.
///
/// The returned content type is the media type without parameters (`image/jpeg`), empty when the
/// server sent none. Retries once on HTTP 429/500/502/503/504 after 1 s.
pub fn get_bytes(url: &str, headers: Headers<'_>) -> Result<(Vec<u8>, String)> {
    let response = get(url, headers)?;
    Ok((response.body, response.content_type))
}

/// POST `application/x-www-form-urlencoded` `form` to `url` and parse JSON.
pub fn post_form_json(url: &str, form: &[(&str, &str)]) -> Result<serde_json::Value> {
    let response = transport()?.post_form(url, form)?;
    if !(200..300).contains(&response.status) {
        return Err(status_error(url, &response));
    }
    parse_json(&response.body, url)
}

/// Percent-encodes `s` for a URL query component (everything except RFC 3986 unreserved
/// characters `A-Z a-z 0-9 - . _ ~` is encoded, per UTF-8 byte).
pub fn encode_query(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0F)]));
        }
    }
    out
}

/// Sends a GET, retrying once after [`RETRY_DELAY`] on 429 and transient 5xx (the Cover Art
/// Archive's archive.org backend returns sporadic 500s). Returns only 2xx responses.
fn get(url: &str, headers: Headers<'_>) -> Result<Response> {
    let transport = transport()?;
    let mut retried = false;
    loop {
        let response = transport.get(url, headers)?;
        if (200..300).contains(&response.status) {
            return Ok(response);
        }
        if !retried && matches!(response.status, 429 | 500 | 502 | 503 | 504) {
            retried = true;
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::sleep(RETRY_DELAY);
            continue;
        }
        return Err(status_error(url, &response));
    }
}

fn parse_json(body: &[u8], url: &str) -> Result<serde_json::Value> {
    serde_json::from_slice(body)
        .map_err(|e| Error::Response(format!("invalid JSON from {url}: {e}")))
}

fn status_error(url: &str, response: &Response) -> Error {
    let status = response.status;
    let head = &response.body[..response.body.len().min(ERROR_EXCERPT)];
    let text = String::from_utf8_lossy(head);
    let excerpt = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if excerpt.is_empty() {
        Error::Http(format!("{STATUS_PREFIX}{status} for {url}"))
    } else {
        Error::Http(format!("{STATUS_PREFIX}{status} for {url}: {excerpt}"))
    }
}

/// The HTTP status behind an `Error::Http` produced for a non-2xx response; `None` for transport
/// failures and other error kinds.
pub fn status_code(err: &Error) -> Option<u16> {
    let Error::Http(message) = err else {
        return None;
    };
    status_of_message(message)
}

/// The status in the message of an `Error::Http` (see [`status_code`]), for hosts that carry the
/// message in their own error type.
pub fn status_of_message(message: &str) -> Option<u16> {
    message
        .strip_prefix(STATUS_PREFIX)?
        .split(' ')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{Error, encode_query, status_code};

    #[test]
    fn encode_query_keeps_only_unreserved() {
        assert_eq!(encode_query("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(
            encode_query("a b&c=d/e?f#g+h"),
            "a%20b%26c%3Dd%2Fe%3Ff%23g%2Bh"
        );
        assert_eq!(
            encode_query("release:\"OK Computer\" AND tracks:12"),
            "release%3A%22OK%20Computer%22%20AND%20tracks%3A12"
        );
    }

    #[test]
    fn encode_query_encodes_utf8_per_byte() {
        assert_eq!(
            encode_query("Björk – Ágætis"),
            "Bj%C3%B6rk%20%E2%80%93%20%C3%81g%C3%A6tis"
        );
        assert_eq!(encode_query(""), "");
    }

    #[test]
    fn status_code_reads_the_status_back() {
        assert_eq!(
            status_code(&Error::Http(
                "status 404 for https://x/y: {\"error\":\"Not Found\"}".into()
            )),
            Some(404)
        );
        assert_eq!(
            status_code(&Error::Http("status 503 for https://x/y".into())),
            Some(503)
        );
        assert_eq!(
            status_code(&Error::Http("GET https://x/y: connection failed".into())),
            None
        );
        assert_eq!(
            status_code(&Error::Response("status 404 for x".into())),
            None
        );
    }
}
