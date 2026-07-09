//! Re-emits a parsed [`RequestFile`] in a canonical layout, used by
//! `zed-http format` and by the Postman importer.
//!
//! Round-trip target: anything the parser can produce should be
//! re-emittable here, and re-parsing the output should give an equal
//! `RequestFile`. The one explicit loss is non-directive comments inside
//! a request — the parser discards them, so the formatter can't put them
//! back. Document-level comments outside requests are likewise dropped.
//!
//! `RequestFile`s built by the importers are *not* parser output, so they can
//! hold text the format cannot express: a name spanning two lines, or a body
//! line that reads back as a `###` separator, a `>>` redirect, or a `< file`
//! reference. Names are sanitised on the way out (a newline there would let an
//! imported name smuggle in a whole extra request), and
//! [`format_request_file_checked`] verifies the rest by re-parsing its own
//! output — better a loud error than a corrupted `.http` file.

use crate::{
    error::HttpClientError,
    model::{
        CaptureDirective, RequestBlock, RequestBody, RequestFile, RequestOptions,
        ResponseAssertion, ResponseRedirect,
    },
    parser::parse_request_file,
};

pub fn format_request_file(file: &RequestFile) -> String {
    let mut output = String::new();

    if let Some(env) = &file.default_env {
        output.push_str(&format!("# @env {env}\n"));
    }
    if file.default_env.is_some() && !file.variables.is_empty() {
        output.push('\n');
    }

    for variable in &file.variables {
        output.push_str(&format!("@{} = {}\n", variable.name, variable.value));
    }
    if (!file.variables.is_empty() || file.default_env.is_some())
        && !file.requests.is_empty()
        && !output.ends_with("\n\n")
    {
        output.push('\n');
    }

    for (index, request) in file.requests.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        format_request(&mut output, request);
    }

    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Format `file`, then prove the result re-parses to the same requests.
///
/// Importers build `RequestFile`s from untrusted external data; this is the
/// gate that stops a body or name the format cannot represent from being
/// written to disk as a file that means something else.
pub fn format_request_file_checked(file: &RequestFile) -> Result<String, HttpClientError> {
    let rendered = format_request_file(file);
    let reparsed = parse_request_file(&rendered).map_err(|error| {
        HttpClientError::Message(format!(
            "the generated request file does not parse back: {error}"
        ))
    })?;

    if reparsed.requests.len() != file.requests.len() {
        return Err(HttpClientError::Message(format!(
            "the generated request file describes {} request(s), expected {} — \
             a request name or body contains a line that reads as a '###' separator",
            reparsed.requests.len(),
            file.requests.len()
        )));
    }

    for (original, actual) in file.requests.iter().zip(&reparsed.requests) {
        if !same_request(original, actual) {
            let label = sanitise_name(original.name.as_deref().unwrap_or_default())
                .unwrap_or_else(|| original.url.clone());
            return Err(HttpClientError::Message(format!(
                "request '{label}' cannot be written to a .http file without changing meaning — \
                 its body contains a line starting with '###', '>>', or '< '"
            )));
        }
    }

    Ok(rendered)
}

/// Compare the parts a `.http` file actually encodes, ignoring line numbers.
fn same_request(original: &RequestBlock, actual: &RequestBlock) -> bool {
    let names_match = sanitise_name(original.name.as_deref().unwrap_or_default())
        == actual.name.as_deref().and_then(sanitise_name);
    let headers_match = original.headers.len() == actual.headers.len()
        && original
            .headers
            .iter()
            .zip(&actual.headers)
            .all(|(a, b)| a.name == b.name && a.value == b.value);

    names_match
        && headers_match
        && original.method == actual.method
        && original.url == actual.url
        && normalise_body(original.body.as_ref()) == normalise_body(actual.body.as_ref())
        && original.response_redirect == actual.response_redirect
}

/// The parser strips blank leading/trailing body lines, so compare bodies the
/// same way. An inline body that is entirely blank is no body at all.
fn normalise_body(body: Option<&RequestBody>) -> Option<(bool, String)> {
    match body? {
        RequestBody::FromFile { path } => Some((true, path.clone())),
        RequestBody::Inline(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.iter().position(|line| !line.trim().is_empty())?;
            let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
            Some((false, lines[start..=end].join("\n")))
        }
    }
}

/// A request name lives on the `###` line, so it must be a single line and
/// must not start with the `#` the parser would strip back off.
fn sanitise_name(name: &str) -> Option<String> {
    let single_line: String = name
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let trimmed = single_line.trim().trim_start_matches('#').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_request(output: &mut String, request: &RequestBlock) {
    output.push_str("###");
    if let Some(name) = request.name.as_deref().and_then(sanitise_name) {
        output.push(' ');
        output.push_str(&name);
    }
    output.push('\n');

    format_options(output, &request.options);
    format_assertions(output, &request.assertions);
    format_captures(output, &request.captures);

    output.push_str(&format!("{} {}\n", request.method.as_str(), request.url));

    for header in &request.headers {
        output.push_str(&format!("{}: {}\n", header.name, header.value));
    }

    if let Some(body) = &request.body {
        output.push('\n');
        match body {
            RequestBody::Inline(text) => {
                output.push_str(text.trim_end_matches('\n'));
                output.push('\n');
            }
            RequestBody::FromFile { path } => {
                output.push_str(&format!("< {path}\n"));
            }
        }
    }

    if let Some(redirect) = &request.response_redirect {
        output.push('\n');
        format_response_redirect(output, redirect);
    }
}

fn format_options(output: &mut String, options: &RequestOptions) {
    if let Some(ms) = options.timeout_ms {
        output.push_str(&format!("# @timeout {ms}\n"));
    }
    if let Some(ms) = options.connection_timeout_ms {
        output.push_str(&format!("# @connection-timeout {ms}\n"));
    }
    if options.no_redirect {
        output.push_str("# @no-redirect\n");
    }
    for path in &options.fragment_paths {
        output.push_str(&format!("# @fragments {path}\n"));
    }
}

fn format_assertions(output: &mut String, assertions: &[ResponseAssertion]) {
    for assertion in assertions {
        match assertion {
            ResponseAssertion::Status { codes, .. } => {
                let joined = codes
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                output.push_str(&format!("# @expect-status {joined}\n"));
            }
            ResponseAssertion::Header {
                name, substring, ..
            } => {
                output.push_str(&format!("# @expect-header {name} {substring}\n"));
            }
            ResponseAssertion::JsonValue {
                pointer, expected, ..
            } => {
                output.push_str(&format!("# @expect-json {pointer} {expected}\n"));
            }
        }
    }
}

fn format_captures(output: &mut String, captures: &[CaptureDirective]) {
    for capture in captures {
        output.push_str(&format!(
            "# @capture {} {}\n",
            capture.variable,
            capture.source.as_string()
        ));
    }
}

fn format_response_redirect(output: &mut String, redirect: &ResponseRedirect) {
    let marker = if redirect.force_overwrite {
        ">>!"
    } else {
        ">>"
    };
    output.push_str(&format!("{marker} {}\n", redirect.path));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_request_file;

    #[test]
    fn canonicalises_a_simple_request_file() {
        let input = "@host  =   https://example.com\n\n### List   users\nGET   {{host}}/api/users\nAccept:   application/json\n";
        let parsed = parse_request_file(input).unwrap();

        let formatted = format_request_file(&parsed);

        assert_eq!(
            formatted,
            "@host = https://example.com\n\n### List   users\nGET {{host}}/api/users\nAccept: application/json\n"
        );
    }

    #[test]
    fn round_trips_options_body_and_redirect() {
        let input = "### Upload\n# @timeout 5000\n# @no-redirect\nPOST https://example.com/upload\nContent-Type: application/json\n\n< ./body.json\n\n>>! ./out.json\n";
        let parsed = parse_request_file(input).unwrap();

        let formatted = format_request_file(&parsed);
        let reparsed = parse_request_file(&formatted).unwrap();

        assert_eq!(parsed, reparsed);
        assert!(formatted.contains("# @timeout 5000"));
        assert!(formatted.contains("# @no-redirect"));
        assert!(formatted.contains("< ./body.json"));
        assert!(formatted.contains(">>! ./out.json"));
    }

    #[test]
    fn round_trips_capture_directives() {
        let input = "### Login\n# @capture token json:/access_token\n# @capture sid header:Set-Cookie\n# @capture code status\nPOST https://example.com/login\n";
        let parsed = parse_request_file(input).unwrap();
        let formatted = format_request_file(&parsed);
        let reparsed = parse_request_file(&formatted).unwrap();
        assert_eq!(parsed, reparsed);
        assert!(formatted.contains("# @capture token json:/access_token"));
        assert!(formatted.contains("# @capture sid header:Set-Cookie"));
        assert!(formatted.contains("# @capture code status"));
    }

    #[test]
    fn emits_file_level_env_directive() {
        let input = "# @env dev\n@host = https://example.com\n\n### Ping\nGET {{host}}/ping\n";
        let parsed = parse_request_file(input).unwrap();
        let formatted = format_request_file(&parsed);

        assert!(formatted.starts_with("# @env dev\n"));
        // re-parse round-trip
        let reparsed = parse_request_file(&formatted).unwrap();
        assert_eq!(reparsed.default_env.as_deref(), Some("dev"));
        assert_eq!(reparsed.variables.len(), 1);
        assert_eq!(reparsed.requests.len(), 1);
    }

    #[test]
    fn places_blank_line_between_requests() {
        let input =
            "### One\nGET https://example.com/one\n\n### Two\nGET https://example.com/two\n";
        let parsed = parse_request_file(input).unwrap();

        let formatted = format_request_file(&parsed);

        assert!(formatted.contains("\n\n### Two"));
    }

    fn block(name: &str, body: Option<RequestBody>) -> RequestFile {
        RequestFile {
            default_env: None,
            variables: Vec::new(),
            requests: vec![RequestBlock {
                name: Some(name.to_string()),
                method: crate::model::RequestMethod::Post,
                url: "https://example.com/real".to_string(),
                headers: Vec::new(),
                body,
                options: RequestOptions::default(),
                assertions: Vec::new(),
                captures: Vec::new(),
                response_redirect: None,
                range: crate::model::SourceRange {
                    start_line: 0,
                    end_line: 0,
                },
            }],
        }
    }

    #[test]
    fn a_newline_in_a_request_name_cannot_smuggle_a_second_request() {
        let file = block("Line one\nGET https://evil.example.com/injected", None);

        let formatted = format_request_file(&file);
        let reparsed = parse_request_file(&formatted).unwrap();

        assert_eq!(reparsed.requests.len(), 1);
        assert_eq!(reparsed.requests[0].url, "https://example.com/real");
        // The name text may still mention the URL; what must not happen is it
        // landing on a line of its own, where it parses as a request line.
        assert!(!formatted.contains("\nGET https://evil.example.com/injected"));
        assert!(format_request_file_checked(&file).is_ok());
    }

    #[test]
    fn a_leading_hash_in_a_name_is_normalised_so_it_round_trips() {
        // `### #1 Create user` re-parses with the leading `#` stripped, so the
        // formatter strips it up front rather than emitting a name it cannot
        // read back.
        let file = block("#1 Create user", None);

        let formatted = format_request_file(&file);
        let reparsed = parse_request_file(&formatted).unwrap();

        assert_eq!(reparsed.requests[0].name.as_deref(), Some("1 Create user"));
        assert!(format_request_file_checked(&file).is_ok());
    }

    #[test]
    fn checked_accepts_a_representable_file() {
        let input = "### Create\nPOST https://example.com/users\nContent-Type: application/json\n\n{\"name\": \"Alice\"}\n";
        let parsed = parse_request_file(input).unwrap();

        assert!(format_request_file_checked(&parsed).is_ok());
    }

    #[test]
    fn checked_rejects_a_body_containing_a_separator_line() {
        let file = block(
            "Notes",
            Some(RequestBody::Inline("intro\n### heading\nrest".to_string())),
        );

        // A `###` line in the body may split the file into a shape that no
        // longer parses at all, so either failure mode is acceptable — what
        // matters is that nothing is written.
        assert!(format_request_file_checked(&file).is_err());
    }

    #[test]
    fn checked_names_the_offending_request_when_the_output_still_parses() {
        let file = block(
            "Notes",
            Some(RequestBody::Inline("intro\n### heading".to_string())),
        );

        let error = format_request_file_checked(&file).unwrap_err();
        assert!(
            error.to_string().contains("Notes") || error.to_string().contains("request(s)"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn checked_rejects_a_body_line_that_looks_like_a_response_redirect() {
        let file = block(
            "Quoted",
            Some(RequestBody::Inline(
                "line one\n>> quoted\nline three".to_string(),
            )),
        );

        assert!(format_request_file_checked(&file).is_err());
    }

    #[test]
    fn checked_rejects_an_inline_body_that_looks_like_a_file_reference() {
        let file = block("Xml", Some(RequestBody::Inline("< foo".to_string())));

        assert!(format_request_file_checked(&file).is_err());
    }
}
