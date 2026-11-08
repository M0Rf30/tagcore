# tagcore

Shared music-tagging core, extracted from [Picardy](https://github.com/M0Rf30/picardy) so that
other players and taggers can reuse it. Tag names follow MusicBrainz Picard's internal variable
names (`title`, `albumartist`, `musicbrainz_recordingid`, ...).

## Modules

| Module | What it does |
|---|---|
| `metadata`, `keys` | `Metadata` (case-insensitive multi-value tag map), `CoverImage`, `ImageType`, Picard key constants |
| `file` | `load` / `save` for every container Symphonia and `symphonia-tagging` support; `AudioProperties`, `SUPPORTED_EXTENSIONS`, `MAX_RATING`; re-exports the `WriteOptions` types |
| `image` | `identify`, `dimensions`, `mime_of`, `ImageFormat` |
| `fingerprint` | Chromaprint fingerprints (`chromaprint-next`) |
| `http` | `Transport`, retry/throttle/JSON helpers, `ServiceConfig` |
| `mb`, `genre`, `discogs`, `acoustid` | MusicBrainz, folksonomy genres, Discogs and AcoustID clients and mappers |
| `matching`, `cluster` | Release/recording matching and file clustering |
| `release`, `settings` | `Release` model; `MetadataSettings` / `GenresSettings` (serde, stable field names) |
| `py`, `pyre` | Python string / `re` semantics the MusicBrainz and genre code relies on |

## HTTP transport

tagcore links no HTTP client or TLS stack (it builds for `wasm32-unknown-unknown`). The host
implements `http::Transport` over its own client and installs it once:

```rust
struct MyTransport; // wraps reqwest, ureq, ...
impl tagcore::http::Transport for MyTransport {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> tagcore::Result<tagcore::http::Response> { todo!() }
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> tagcore::Result<tagcore::http::Response> { todo!() }
}
tagcore::http::set_transport(std::sync::Arc::new(MyTransport));
tagcore::http::configure(tagcore::http::ServiceConfig::default());
```

A transport sets its own `User-Agent`, follows redirects, applies its timeout/proxy and caps
bodies at `http::MAX_BODY`. A non-2xx status is returned as a `Response`, not an error; tagcore
retries once on 429/5xx and formats status errors. Until a transport is installed every request
fails with `Error::Http("no HTTP transport configured")`.

## License

GPL-3.0-or-later.
