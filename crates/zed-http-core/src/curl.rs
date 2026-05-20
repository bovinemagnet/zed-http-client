//! `curl` command-line → [`RequestFile`] importer.
//!
//! Aimed at the "paste from browser devtools" workflow: a user copies the
//! "Copy as cURL" shape from Chrome / Firefox / Safari, and we translate it
//! into a single canonical request block they can drop into a `.http`
//! file.
//!
//! Supports the curl flags devtools actually emit: `-X`/`--request`,
//! `-H`/`--header`, `-d` / `--data` / `--data-raw` / `--data-binary` /
//! `--data-urlencode`, `-u` / `--user`, `-A` / `--user-agent`,
//! `-e` / `--referer`, `-b` / `--cookie`, `-G` / `--get`, the bare URL, and
//! line-continuation backslashes. Multipart (`-F` / `--form`) is recognised
//! but produces a `# TODO multipart not supported yet` comment alongside
//! the request body — the runtime doesn't render multipart yet so we'd
//! lie if we tried to encode it for real.
//!
//! Tokenisation is a small shell-like lexer: single-quoted runs are
//! preserved verbatim (curl's standard escape), double-quoted runs handle
//! `\"` and `\\`, and an unquoted backslash followed by a newline is
//! treated as line-continuation whitespace.

use base64::{engine::general_purpose::STANDARD as Base64Std, Engine as _};

use crate::{
    error::HttpClientError,
    model::{Header, RequestBlock, RequestBody, RequestFile, RequestMethod, SourceRange},
};

pub fn import_curl(input: &str, name: Option<&str>) -> Result<RequestFile, HttpClientError> {
    let tokens = tokenize(input)?;
    let request = build_request_block(&tokens, name)?;
    Ok(RequestFile {
        default_env: None,
        variables: Vec::new(),
        requests: vec![request],
    })
}

fn build_request_block(
    tokens: &[String],
    name_override: Option<&str>,
) -> Result<RequestBlock, HttpClientError> {
    let mut iter = tokens.iter().peekable();

    // Skip a leading `curl` token if present.
    if iter.peek().map(|s| s.as_str()) == Some("curl") {
        iter.next();
    }

    let mut method: Option<RequestMethod> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<Header> = Vec::new();
    let mut body_parts: Vec<String> = Vec::new();
    let mut urlencoded_parts: Vec<String> = Vec::new();
    let mut multipart_seen = false;
    let mut get_after_data = false;
    let mut basic_auth: Option<String> = None;
    let mut user_agent: Option<String> = None;
    let mut referer: Option<String> = None;
    let mut cookies: Vec<String> = Vec::new();
    let mut body_from_file: Option<String> = None;

    while let Some(token) = iter.next() {
        match token.as_str() {
            "-X" | "--request" => {
                let value = iter.next().ok_or_else(|| missing_value("-X/--request"))?;
                method = Some(parse_method(value)?);
            }
            "-H" | "--header" => {
                let value = iter.next().ok_or_else(|| missing_value("-H/--header"))?;
                if let Some((name, val)) = value.split_once(':') {
                    headers.push(Header {
                        name: name.trim().to_string(),
                        value: val.trim().to_string(),
                        line: 0,
                    });
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                let value = iter.next().ok_or_else(|| missing_value(token))?;
                if let Some(path) = value.strip_prefix('@') {
                    body_from_file = Some(path.to_string());
                } else {
                    body_parts.push(value.clone());
                }
            }
            "--data-urlencode" => {
                let value = iter.next().ok_or_else(|| missing_value(token))?;
                urlencoded_parts.push(value.clone());
            }
            "-F" | "--form" => {
                // Consume the value so it doesn't get treated as the URL.
                let _ = iter.next();
                multipart_seen = true;
            }
            "-u" | "--user" => {
                let value = iter.next().ok_or_else(|| missing_value("-u/--user"))?;
                basic_auth = Some(value.clone());
            }
            "-A" | "--user-agent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| missing_value("-A/--user-agent"))?;
                user_agent = Some(value.clone());
            }
            "-e" | "--referer" => {
                let value = iter.next().ok_or_else(|| missing_value("-e/--referer"))?;
                referer = Some(value.clone());
            }
            "-b" | "--cookie" => {
                let value = iter.next().ok_or_else(|| missing_value("-b/--cookie"))?;
                cookies.push(value.clone());
            }
            "-G" | "--get" => {
                get_after_data = true;
            }
            "--url" => {
                let value = iter.next().ok_or_else(|| missing_value("--url"))?;
                url = Some(value.clone());
            }
            "--compressed" | "-L" | "--location" | "-s" | "--silent" | "-S" | "--show-error"
            | "-k" | "--insecure" | "-i" | "--include" | "-v" | "--verbose" | "-f" | "--fail"
            | "--http2" | "--http1.1" => {
                // Recognised but not actionable for our import. Continue.
            }
            other if other.starts_with("--") && consumes_value(other) => {
                // Unrecognised flag that almost certainly takes a value
                // (e.g. --cookie-jar foo.txt). Skip the next token too so we
                // don't mistake it for the URL.
                let _ = iter.next();
            }
            other if other.starts_with('-') && other.len() > 1 => {
                // Unrecognised short flag — skip but don't consume the
                // next token in case it's the URL.
            }
            _ => {
                if url.is_none() {
                    url = Some(token.clone());
                }
            }
        }
    }

    let mut url =
        url.ok_or_else(|| HttpClientError::Message("curl command had no URL".to_string()))?;

    // Push -u into a header before any caller-provided Authorization wins.
    if let Some(creds) = basic_auth {
        let encoded = Base64Std.encode(creds.as_bytes());
        headers.insert(
            0,
            Header {
                name: "Authorization".to_string(),
                value: format!("Basic {encoded}"),
                line: 0,
            },
        );
    }
    if let Some(ua) = user_agent {
        upsert_header(&mut headers, "User-Agent", &ua);
    }
    if let Some(r) = referer {
        upsert_header(&mut headers, "Referer", &r);
    }
    if !cookies.is_empty() {
        upsert_header(&mut headers, "Cookie", &cookies.join("; "));
    }

    if !urlencoded_parts.is_empty() {
        let pairs: Vec<String> = urlencoded_parts.iter().map(|p| encode_pair(p)).collect();
        body_parts.push(pairs.join("&"));
    }

    let body_text = if body_parts.is_empty() {
        None
    } else {
        Some(body_parts.join("&"))
    };

    // -G turns the body into URL query parameters.
    if get_after_data {
        if let Some(text) = body_text.as_ref() {
            let sep = if url.contains('?') { '&' } else { '?' };
            url.push(sep);
            url.push_str(text);
        }
    }

    let inferred_method = if let Some(m) = method {
        m
    } else if get_after_data {
        // -G forces GET even when -d was passed; -d data becomes the
        // query string, never the body.
        RequestMethod::Get
    } else if body_text.is_some() || body_from_file.is_some() || multipart_seen {
        RequestMethod::Post
    } else {
        RequestMethod::Get
    };

    let body = if get_after_data {
        None
    } else if let Some(path) = body_from_file {
        Some(RequestBody::FromFile { path })
    } else {
        body_text.map(RequestBody::Inline)
    };

    let mut name = name_override.map(str::to_string);
    if name.is_none() {
        name = Some("Imported from curl".to_string());
    }

    if multipart_seen {
        // We can't faithfully render multipart yet — leave a marker.
        let note = "TODO: multipart body skipped (curl -F not yet supported)";
        if let Some(existing) = name.as_mut() {
            existing.push_str(" — ");
            existing.push_str(note);
        }
    }

    Ok(RequestBlock {
        name,
        method: inferred_method,
        url,
        headers,
        body,
        options: Default::default(),
        assertions: Vec::new(),
        response_redirect: None,
        range: SourceRange {
            start_line: 0,
            end_line: 0,
        },
    })
}

fn upsert_header(headers: &mut Vec<Header>, name: &str, value: &str) {
    if headers.iter().any(|h| h.name.eq_ignore_ascii_case(name)) {
        return;
    }
    headers.push(Header {
        name: name.to_string(),
        value: value.to_string(),
        line: 0,
    });
}

fn encode_pair(raw: &str) -> String {
    // curl's --data-urlencode supports several forms:
    //   `value`               → encoded value with no name
    //   `name=value`          → name unchanged, value encoded
    //   `name@file`           → file contents encoded (out of scope here;
    //                            we leave the literal text)
    if let Some((name, value)) = raw.split_once('=') {
        format!("{name}={}", url_encode(value))
    } else {
        url_encode(raw)
    }
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let c = *byte;
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}

fn parse_method(text: &str) -> Result<RequestMethod, HttpClientError> {
    RequestMethod::parse(text).ok_or_else(|| {
        HttpClientError::Message(format!("curl -X used an unknown HTTP method '{text}'"))
    })
}

fn missing_value(flag: &str) -> HttpClientError {
    HttpClientError::Message(format!("curl flag {flag} was missing its value"))
}

fn consumes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--cookie-jar"
            | "--connect-timeout"
            | "--max-time"
            | "--retry"
            | "--proxy"
            | "--cert"
            | "--key"
            | "--cacert"
            | "--output"
            | "--write-out"
            | "--resolve"
            | "--interface"
            | "--limit-rate"
            | "--max-redirs"
            | "--upload-file"
            | "--unix-socket"
            | "--http-version"
    )
}

fn tokenize(input: &str) -> Result<Vec<String>, HttpClientError> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                current.push(c);
            }
            i += 1;
            continue;
        }
        if in_double {
            if c == '"' {
                in_double = false;
            } else if c == '\\' && i + 1 < chars.len() {
                let next = chars[i + 1];
                match next {
                    '"' | '\\' | '$' | '`' => {
                        current.push(next);
                        i += 2;
                        continue;
                    }
                    '\n' => {
                        // Line continuation inside double quotes.
                        i += 2;
                        continue;
                    }
                    _ => {
                        // Preserve the backslash for unknown escapes.
                        current.push('\\');
                    }
                }
            } else {
                current.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                i += 1;
            }
            '"' => {
                in_double = true;
                i += 1;
            }
            '\\' if i + 1 < chars.len() && chars[i + 1] == '\n' => {
                // Backslash-newline outside quotes → line continuation;
                // treat as whitespace.
                i += 2;
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '\\' if i + 1 < chars.len() => {
                current.push(chars[i + 1]);
                i += 2;
            }
            ws if ws.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                i += 1;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }

    if in_single || in_double {
        return Err(HttpClientError::Message(
            "curl command had an unterminated quoted string".to_string(),
        ));
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(text: &str) -> RequestBlock {
        import_curl(text, None).unwrap().requests.pop().unwrap()
    }

    #[test]
    fn tokenises_quoted_strings_and_continuations() {
        let tokens = tokenize(
            "curl 'https://example.com/api' \\\n  -H 'accept: application/json' \\\n  --data-raw '{\"a\":1}'",
        )
        .unwrap();
        assert_eq!(
            tokens,
            vec![
                "curl",
                "https://example.com/api",
                "-H",
                "accept: application/json",
                "--data-raw",
                "{\"a\":1}",
            ]
        );
    }

    #[test]
    fn imports_simple_get() {
        let block = import("curl https://example.com/health");
        assert_eq!(block.method, RequestMethod::Get);
        assert_eq!(block.url, "https://example.com/health");
        assert!(block.body.is_none());
        assert!(block.headers.is_empty());
    }

    #[test]
    fn imports_post_with_json_body() {
        let block = import(
            "curl -X POST https://example.com/api -H 'Content-Type: application/json' --data-raw '{\"a\":1}'",
        );
        assert_eq!(block.method, RequestMethod::Post);
        assert_eq!(block.headers[0].name.to_ascii_lowercase(), "content-type");
        match block.body.as_ref().unwrap() {
            RequestBody::Inline(text) => assert_eq!(text, "{\"a\":1}"),
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn data_without_explicit_method_implies_post() {
        let block = import("curl https://example.com/api -d hello=world");
        assert_eq!(block.method, RequestMethod::Post);
    }

    #[test]
    fn dash_g_moves_body_into_query_string() {
        let block = import("curl -G https://example.com/search -d 'q=zed' -d 'lang=en'");
        assert_eq!(block.method, RequestMethod::Get);
        assert_eq!(block.url, "https://example.com/search?q=zed&lang=en");
        assert!(block.body.is_none());
    }

    #[test]
    fn data_at_file_becomes_from_file_body() {
        let block = import("curl https://example.com/upload -d @./payload.json");
        match block.body.as_ref().unwrap() {
            RequestBody::FromFile { path } => assert_eq!(path, "./payload.json"),
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn basic_auth_is_base64_encoded_into_authorization_header() {
        let block = import("curl -u admin:secret https://example.com/protected");
        let auth = block
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("authorization"))
            .unwrap();
        // base64("admin:secret") == "YWRtaW46c2VjcmV0"
        assert_eq!(auth.value, "Basic YWRtaW46c2VjcmV0");
    }

    #[test]
    fn user_agent_and_referer_short_flags_become_headers() {
        let block =
            import("curl https://example.com -A 'zed-http-test/1.0' -e 'https://from.example.com'");
        assert!(block
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("user-agent") && h.value == "zed-http-test/1.0"));
        assert!(block.headers.iter().any(
            |h| h.name.eq_ignore_ascii_case("referer") && h.value == "https://from.example.com"
        ));
    }

    #[test]
    fn cookies_are_concatenated_into_one_header() {
        let block = import("curl https://example.com -b 'session=abc' -b 'tracking=xyz'");
        let cookie = block
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("cookie"))
            .unwrap();
        assert_eq!(cookie.value, "session=abc; tracking=xyz");
    }

    #[test]
    fn data_urlencode_pairs_are_percent_encoded() {
        let block = import("curl -X POST https://example.com --data-urlencode 'name=Alice & Bob'");
        match block.body.as_ref().unwrap() {
            RequestBody::Inline(text) => assert_eq!(text, "name=Alice%20%26%20Bob"),
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn multipart_is_flagged_in_the_name() {
        let block =
            import("curl -X POST https://example.com -F 'file=@./payload.json' -F 'kind=upload'");
        assert!(block
            .name
            .as_ref()
            .unwrap()
            .contains("multipart body skipped"));
    }

    #[test]
    fn unrecognised_value_taking_flags_skip_their_value() {
        // --max-time 30 should not be mistaken for a URL or method.
        let block = import("curl --max-time 30 https://example.com/x");
        assert_eq!(block.url, "https://example.com/x");
    }

    #[test]
    fn unterminated_quote_errors() {
        let err = import_curl("curl 'https://example.com", None).unwrap_err();
        assert!(matches!(err, HttpClientError::Message(_)));
    }

    #[test]
    fn name_override_is_used_when_provided() {
        let file = import_curl("curl https://example.com", Some("Health check")).unwrap();
        assert_eq!(file.requests[0].name.as_deref(), Some("Health check"));
    }
}
