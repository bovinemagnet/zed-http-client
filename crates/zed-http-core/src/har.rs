//! HTTP Archive (HAR 1.2) → [`RequestFile`] importer.
//!
//! HAR is the JSON shape browser devtools use for "Save all as HAR with
//! content" exports — the bulk version of curl import. Each entry in
//! `log.entries[]` is translated into a single `RequestBlock`; URLs keep
//! their query strings as-is, headers come over verbatim (excluding the
//! pseudo-headers like `:authority` that HTTP/2 captures emit), and
//! `postData.text` becomes an inline body when present. Multipart
//! `postData.params` is recognised but the body is skipped with a note
//! appended to the request name, mirroring how the curl importer handles
//! `-F` flags.
//!
//! Request names default to `<index>: <METHOD> <path>` so the output of
//! `zed-http list` against an imported file is still scannable. A
//! caller-provided `--name-prefix` is prepended in CLI usage.
//!
//! Inputs are decoded by [`decode_har_input`], which sniffs the RFC 1952
//! gzip magic bytes (`0x1f 0x8b`) and transparently decompresses a
//! `.har.gz` archive before parsing — so browser exports can be imported
//! either compressed or plain.

use std::io::Read;

use flate2::read::GzDecoder;
use serde_json::Value;

use crate::{
    error::HttpClientError,
    model::{
        Header, RequestBlock, RequestBody, RequestFile, RequestMethod, RequestOptions, SourceRange,
    },
};

/// Pseudo-headers that browsers emit for HTTP/2 captures but that have
/// no meaning in a replayed `.http` request. Filtering them out avoids
/// confusing diagnostics from `reqwest` when the file is replayed.
const HTTP2_PSEUDO_HEADERS: &[&str] = &[":authority", ":method", ":path", ":scheme", ":status"];

/// RFC 1952 gzip magic bytes.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Decode raw HAR bytes into the UTF-8 JSON string that [`import_har`]
/// expects, transparently decompressing gzip-magic input. Used by the
/// CLI before handing the text to the parser so `.har.gz` archives
/// from browser devtools work without a separate `gunzip` step.
pub fn decode_har_input(bytes: &[u8]) -> Result<String, HttpClientError> {
    if bytes.len() >= 2 && bytes[..2] == GZIP_MAGIC {
        let mut out = String::new();
        GzDecoder::new(bytes)
            .read_to_string(&mut out)
            .map_err(|e| HttpClientError::Message(format!("HAR gzip decode error: {e}")))?;
        return Ok(out);
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|e| HttpClientError::Message(format!("HAR input is not valid UTF-8: {e}")))
}

pub fn import_har(input: &str, name_prefix: Option<&str>) -> Result<RequestFile, HttpClientError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|e| HttpClientError::Message(format!("HAR JSON parse error: {e}")))?;
    let entries = value
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            HttpClientError::Message(
                "HAR file missing /log/entries — is this a valid HAR 1.2 archive?".to_string(),
            )
        })?;

    let mut requests = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if let Some(block) = build_request_block(entry, index, name_prefix) {
            requests.push(block);
        }
    }

    Ok(RequestFile {
        default_env: None,
        variables: Vec::new(),
        requests,
    })
}

fn build_request_block(
    entry: &Value,
    index: usize,
    name_prefix: Option<&str>,
) -> Option<RequestBlock> {
    let request = entry.get("request")?;
    let method_text = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let method = RequestMethod::parse(method_text).unwrap_or(RequestMethod::Get);
    let url = request.get("url").and_then(Value::as_str)?.to_string();

    let mut headers: Vec<Header> = Vec::new();
    if let Some(array) = request.get("headers").and_then(Value::as_array) {
        for h in array {
            let Some(name) = h.get("name").and_then(Value::as_str) else {
                continue;
            };
            if HTTP2_PSEUDO_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            let value = h.get("value").and_then(Value::as_str).unwrap_or("");
            headers.push(Header {
                name: name.to_string(),
                value: value.to_string(),
                line: 0,
            });
        }
    }

    let (body, multipart_seen) = extract_body(request);

    // Build a sensible name: `<index+1>: <METHOD> <path-and-query>`. Strip
    // the scheme/host so the listing stays compact for long URLs.
    let label_path = path_for_label(&url);
    let mut name = format!("{}: {} {}", index + 1, method_text, label_path);
    if multipart_seen {
        name.push_str(" — multipart body skipped");
    }
    if let Some(prefix) = name_prefix {
        let prefix = prefix.trim();
        if !prefix.is_empty() {
            name = format!("{prefix} / {name}");
        }
    }

    Some(RequestBlock {
        name: Some(name),
        name_directive: None,
        method,
        url,
        headers,
        body,
        options: RequestOptions::default(),
        assertions: Vec::new(),
        captures: Vec::new(),
        unknown_directives: Vec::new(),
        response_redirect: None,
        range: SourceRange {
            start_line: 0,
            end_line: 0,
        },
    })
}

fn extract_body(request: &Value) -> (Option<RequestBody>, bool) {
    let Some(post_data) = request.get("postData") else {
        return (None, false);
    };

    let mime_type = post_data
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_multipart = mime_type.starts_with("multipart/");

    if let Some(text) = post_data.get("text").and_then(Value::as_str) {
        if !text.is_empty() {
            return (Some(RequestBody::Inline(text.to_string())), is_multipart);
        }
    }
    if is_multipart {
        // `postData.params` is the encoded form; we don't render that yet.
        return (None, true);
    }
    (None, false)
}

fn path_for_label(url: &str) -> String {
    // Strip scheme + authority for a short label. Falls back to the full
    // URL if parsing fails.
    if let Ok(parsed) = url::Url::parse(url) {
        let mut label = parsed.path().to_string();
        if let Some(q) = parsed.query() {
            label.push('?');
            label.push_str(q);
        }
        if label.is_empty() {
            return parsed.host_str().unwrap_or(url).to_string();
        }
        return label;
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::GzEncoder, Compression};

    use super::*;

    fn import(input: &str) -> RequestFile {
        import_har(input, None).unwrap()
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    #[test]
    fn imports_simple_get_entry() {
        let input = r#"{
            "log": {
                "version": "1.2",
                "creator": { "name": "Test", "version": "1.0" },
                "entries": [{
                    "startedDateTime": "2026-05-20T00:00:00.000Z",
                    "time": 12,
                    "request": {
                        "method": "GET",
                        "url": "https://example.com/api/users?limit=10",
                        "httpVersion": "HTTP/2",
                        "headers": [
                            { "name": "Accept", "value": "application/json" }
                        ],
                        "queryString": [],
                        "cookies": [],
                        "headersSize": -1,
                        "bodySize": 0
                    },
                    "response": {}
                }]
            }
        }"#;

        let file = import(input);
        assert_eq!(file.requests.len(), 1);
        let req = &file.requests[0];
        assert_eq!(req.method, RequestMethod::Get);
        assert_eq!(req.url, "https://example.com/api/users?limit=10");
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.name.as_deref(), Some("1: GET /api/users?limit=10"));
    }

    #[test]
    fn imports_post_with_inline_body() {
        let input = r#"{
            "log": { "version": "1.2", "entries": [{
                "request": {
                    "method": "POST",
                    "url": "https://example.com/users",
                    "headers": [{ "name": "Content-Type", "value": "application/json" }],
                    "postData": {
                        "mimeType": "application/json",
                        "text": "{\"name\":\"Alice\"}"
                    }
                }
            }] }
        }"#;

        let file = import(input);
        let req = &file.requests[0];
        assert_eq!(req.method, RequestMethod::Post);
        match req.body.as_ref().unwrap() {
            RequestBody::Inline(text) => assert!(text.contains("Alice")),
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn strips_http2_pseudo_headers() {
        let input = r#"{
            "log": { "version": "1.2", "entries": [{
                "request": {
                    "method": "GET",
                    "url": "https://example.com/x",
                    "headers": [
                        { "name": ":authority", "value": "example.com" },
                        { "name": ":path",      "value": "/x" },
                        { "name": ":scheme",    "value": "https" },
                        { "name": ":method",    "value": "GET" },
                        { "name": "Accept",     "value": "*/*" }
                    ]
                }
            }] }
        }"#;

        let file = import(input);
        let names: Vec<&str> = file.requests[0]
            .headers
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        assert_eq!(names, vec!["Accept"]);
    }

    #[test]
    fn multipart_post_drops_body_with_note() {
        let input = r#"{
            "log": { "version": "1.2", "entries": [{
                "request": {
                    "method": "POST",
                    "url": "https://example.com/upload",
                    "headers": [],
                    "postData": {
                        "mimeType": "multipart/form-data; boundary=----X",
                        "params": [{ "name": "file", "fileName": "a.json" }]
                    }
                }
            }] }
        }"#;

        let file = import(input);
        let req = &file.requests[0];
        assert!(req.body.is_none());
        assert!(req
            .name
            .as_ref()
            .unwrap()
            .contains("multipart body skipped"));
    }

    #[test]
    fn flattens_multiple_entries_into_one_file() {
        let input = r#"{
            "log": { "version": "1.2", "entries": [
                { "request": { "method": "GET",  "url": "https://example.com/a", "headers": [] } },
                { "request": { "method": "POST", "url": "https://example.com/b", "headers": [] } }
            ] }
        }"#;

        let file = import(input);
        assert_eq!(file.requests.len(), 2);
        assert!(file.requests[0]
            .name
            .as_ref()
            .unwrap()
            .contains("1: GET /a"));
        assert!(file.requests[1]
            .name
            .as_ref()
            .unwrap()
            .contains("2: POST /b"));
    }

    #[test]
    fn name_prefix_is_applied_to_every_request() {
        let input = r#"{
            "log": { "version": "1.2", "entries": [
                { "request": { "method": "GET", "url": "https://example.com/a", "headers": [] } }
            ] }
        }"#;

        let file = import_har(input, Some("Smoke")).unwrap();
        assert_eq!(file.requests[0].name.as_deref(), Some("Smoke / 1: GET /a"));
    }

    #[test]
    fn errors_when_log_entries_missing() {
        let err = import_har("{\"log\":{}}", None).unwrap_err();
        match err {
            HttpClientError::Message(msg) => assert!(msg.contains("missing /log/entries")),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    const MINIMAL_HAR: &str = r#"{
        "log": { "version": "1.2", "entries": [
            { "request": { "method": "GET", "url": "https://example.com/a", "headers": [] } }
        ] }
    }"#;

    #[test]
    fn decode_har_input_passes_through_plain_json() {
        let decoded = decode_har_input(MINIMAL_HAR.as_bytes()).unwrap();
        assert_eq!(decoded, MINIMAL_HAR);
    }

    #[test]
    fn decode_har_input_decompresses_gzip() {
        let compressed = gzip(MINIMAL_HAR.as_bytes());
        assert_eq!(&compressed[..2], &GZIP_MAGIC);
        let decoded = decode_har_input(&compressed).unwrap();
        assert_eq!(decoded, MINIMAL_HAR);
    }

    #[test]
    fn import_har_via_decode_handles_gzip_end_to_end() {
        let compressed = gzip(MINIMAL_HAR.as_bytes());
        let decoded = decode_har_input(&compressed).unwrap();
        let from_gzip = import_har(&decoded, None).unwrap();
        let from_plain = import(MINIMAL_HAR);
        assert_eq!(from_gzip.requests.len(), 1);
        assert_eq!(
            from_gzip.requests[0].name, from_plain.requests[0].name,
            "gzip round-trip should match plain import"
        );
        assert_eq!(from_gzip.requests[0].url, from_plain.requests[0].url);
    }

    #[test]
    fn decode_har_input_rejects_invalid_gzip() {
        // Gzip magic followed by garbage — decompressor should fail.
        let mut bad = vec![0x1f, 0x8b];
        bad.extend_from_slice(b"not really a gzip stream");
        let err = decode_har_input(&bad).unwrap_err();
        match err {
            HttpClientError::Message(msg) => {
                assert!(msg.contains("HAR gzip decode"), "got: {msg}")
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn decode_har_input_rejects_non_utf8_plain() {
        // No gzip magic, but invalid UTF-8 — should surface a UTF-8 error.
        let bytes: Vec<u8> = vec![0xff, 0xfe, 0xfd];
        let err = decode_har_input(&bytes).unwrap_err();
        match err {
            HttpClientError::Message(msg) => assert!(msg.contains("UTF-8"), "got: {msg}"),
            other => panic!("expected Message, got {other:?}"),
        }
    }
}
