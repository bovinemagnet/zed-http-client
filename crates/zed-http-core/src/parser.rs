//! Hand-rolled parser from a `.http` file body to a [`RequestFile`].
//!
//! IntelliJ's HTTP Client format is line-oriented with a small handful of
//! shapes: a top-of-file preamble of `@name = value` variables and `# @env`
//! directives, then any number of `###`-separated request blocks. Within
//! each block: optional `# @<option>` directives, a `METHOD url` line,
//! `Header: value` lines, a blank-line-separated body, and an optional
//! `>>` / `>>!` response-redirect line.
//!
//! The parser is deliberately tolerant — blank lines, comments, and
//! whitespace are forgiven — but it surfaces line-numbered
//! [`HttpClientError`] variants for the things that genuinely don't make
//! sense (e.g. `@timeout fast`).
//!
//! A separate Tree-sitter grammar exists for Zed's syntax highlighting and
//! runnable detection; the two parsers run independently and the CLI uses
//! this one for execution semantics.

use crate::{
    error::HttpClientError,
    model::{
        CaptureDirective, CaptureSource, Header, InPlaceVariable, RequestBlock, RequestBody,
        RequestFile, RequestMethod, RequestOptions, ResponseAssertion, ResponseRedirect,
        SourceRange,
    },
};

type NumberedLine<'a> = (usize, &'a str);

pub fn parse_request_file(input: &str) -> Result<RequestFile, HttpClientError> {
    let lines: Vec<NumberedLine<'_>> = input
        .lines()
        .enumerate()
        .map(|(idx, line)| (idx + 1, line))
        .collect();

    // Split on column-0 `###` markers. An indented `###` is body content, not
    // a separator. The region before the first marker is a section in its own
    // right: it may carry file variables, `# @env`, and — as IntelliJ allows —
    // an unnamed first request.
    let mut sections: Vec<(Option<String>, &[NumberedLine<'_>])> = Vec::new();
    let mut section_start = 0usize;
    let mut pending_name: Option<String> = None;

    for (idx, (_, line)) in lines.iter().enumerate() {
        if !line.starts_with("###") {
            continue;
        }
        sections.push((pending_name.take(), &lines[section_start..idx]));
        section_start = idx + 1;
        pending_name = parse_request_name(line);
    }
    sections.push((pending_name, &lines[section_start..]));

    // `# @env` is a file-level directive, so it is only honoured in the
    // preamble ahead of the first separator.
    let default_env = parse_default_env(sections[0].1)?;

    let mut requests = Vec::new();
    let mut variables = Vec::new();
    for (index, (name, section_lines)) in sections.into_iter().enumerate() {
        // `# @env` belongs to the file, and `parse_default_env` has already
        // taken it from the preamble. Elsewhere it means nothing to us, so it
        // is preserved verbatim rather than dropped.
        let section = parse_section(section_lines, name, index == 0)?;
        variables.extend(section.variables);
        if let Some(request) = section.block {
            requests.push(request);
        }
    }

    Ok(RequestFile {
        default_env,
        variables,
        requests,
    })
}

fn parse_default_env(lines: &[NumberedLine<'_>]) -> Result<Option<String>, HttpClientError> {
    let mut result: Option<String> = None;
    for (line_no, line) in lines {
        let Some(directive) = parse_option_directive(line) else {
            continue;
        };
        if directive.name != "env" {
            continue;
        }
        let value = directive
            .value
            .as_deref()
            .ok_or_else(|| HttpClientError::InvalidOption {
                line: *line_no,
                content: "@env requires an environment name".to_string(),
            })?;
        if result.is_some() {
            return Err(HttpClientError::InvalidOption {
                line: *line_no,
                content: format!("@env is declared more than once (second value: '{value}')"),
            });
        }
        result = Some(value.trim().to_string());
    }
    Ok(result)
}

pub fn select_request_by_line(file: &RequestFile, line: usize) -> Option<&RequestBlock> {
    file.requests
        .iter()
        .find(|request| request.range.start_line <= line && line <= request.range.end_line)
}

/// Match either name a block can carry: its `### separator` text or its
/// `# @name` directive. Both read as "the name of this request" to a user.
pub fn select_request_by_name<'a>(file: &'a RequestFile, name: &str) -> Option<&'a RequestBlock> {
    let needle = name.trim();
    file.requests.iter().find(|request| {
        [request.name_directive.as_deref(), request.name.as_deref()]
            .into_iter()
            .flatten()
            .any(|candidate| candidate.eq_ignore_ascii_case(needle))
    })
}

fn parse_variable(line_no: usize, line: &str) -> Option<InPlaceVariable> {
    let trimmed = line.trim();
    if !trimmed.starts_with('@') {
        return None;
    }

    let (name, value) = trimmed[1..].split_once('=')?;
    Some(InPlaceVariable {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        line: line_no,
    })
}

fn parse_request_name(line: &str) -> Option<String> {
    let name = line.trim_start().trim_start_matches('#').trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// One `###`-delimited region: the request it declares (if any) plus the
/// `@name = value` variables its preamble contributes to the file.
struct ParsedSection {
    block: Option<RequestBlock>,
    variables: Vec<InPlaceVariable>,
}

/// Everything the `# @...` lines ahead of a request line contribute to it.
#[derive(Default)]
struct SectionDirectives {
    options: RequestOptions,
    assertions: Vec<ResponseAssertion>,
    captures: Vec<CaptureDirective>,
    name: Option<String>,
    unknown: Vec<String>,
    /// The region before the first `###`, whose `# @env` the file already took.
    is_preamble: bool,
}

fn parse_section(
    lines: &[NumberedLine<'_>],
    name: Option<String>,
    is_preamble: bool,
) -> Result<ParsedSection, HttpClientError> {
    let mut cursor = 0usize;
    let mut directives = SectionDirectives {
        is_preamble,
        ..SectionDirectives::default()
    };
    let mut variables: Vec<InPlaceVariable> = Vec::new();
    while cursor < lines.len() {
        let (line_no, line) = lines[cursor];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        if let Some(directive) = parse_option_directive(line) {
            apply_directive(&mut directives, &directive, line_no)?;
            cursor += 1;
            continue;
        }
        if is_comment(line) {
            cursor += 1;
            continue;
        }
        if let Some(variable) = parse_variable(line_no, line) {
            variables.push(variable);
            cursor += 1;
            continue;
        }
        break;
    }

    if cursor >= lines.len() {
        return Ok(ParsedSection {
            block: None,
            variables,
        });
    }

    let (request_line_no, request_line) = lines[cursor];
    let (method_text, url_text) = request_line
        .trim()
        .split_once(char::is_whitespace)
        .ok_or_else(|| HttpClientError::InvalidRequestLine {
            line: request_line_no,
            content: request_line.to_string(),
        })?;
    let method =
        RequestMethod::parse(method_text).ok_or_else(|| HttpClientError::InvalidRequestLine {
            line: request_line_no,
            content: request_line.to_string(),
        })?;
    let url = strip_http_version(url_text.trim()).to_string();

    cursor += 1;
    let mut headers = Vec::new();
    while cursor < lines.len() {
        let (line_no, line) = lines[cursor];
        if line.trim().is_empty() {
            cursor += 1;
            break;
        }
        if is_comment(line) {
            cursor += 1;
            continue;
        }
        if let Some((header_name, header_value)) = line.split_once(':') {
            headers.push(Header {
                name: header_name.trim().to_string(),
                value: header_value.trim().to_string(),
                line: line_no,
            });
        }
        cursor += 1;
    }

    let tail = trim_blank_body_lines(&lines[cursor..]);
    let (body_lines, redirect) = split_off_response_redirect(tail)?;
    let body = build_body(body_lines)?;
    let end_line = redirect
        .as_ref()
        .map(|(line_no, _)| *line_no)
        .or_else(|| body_lines.last().map(|(line_no, _)| *line_no))
        .or_else(|| headers.last().map(|header| header.line))
        .unwrap_or(request_line_no);
    let response_redirect = redirect.map(|(_, value)| value);

    Ok(ParsedSection {
        block: Some(RequestBlock {
            name,
            name_directive: directives.name,
            method,
            url,
            headers,
            body,
            options: directives.options,
            assertions: directives.assertions,
            captures: directives.captures,
            unknown_directives: directives.unknown,
            response_redirect,
            range: SourceRange {
                start_line: request_line_no,
                end_line,
            },
        }),
        variables,
    })
}

/// `METHOD URI HTTP-Version` is standard request-line syntax; the version is
/// informational here, so drop it rather than gluing it onto the URL.
fn strip_http_version(url: &str) -> &str {
    match url.rsplit_once(char::is_whitespace) {
        Some((head, version)) if is_http_version(version) && !head.trim().is_empty() => {
            head.trim_end()
        }
        _ => url,
    }
}

fn is_http_version(token: &str) -> bool {
    match token.strip_prefix("HTTP/") {
        Some(digits) => {
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        }
        None => false,
    }
}

#[derive(Debug, Clone)]
struct OptionDirective {
    name: String,
    value: Option<String>,
    /// The directive as written, from the `@` onwards, so an unrecognised one
    /// can be re-emitted with its original spelling.
    raw: String,
}

fn parse_option_directive(line: &str) -> Option<OptionDirective> {
    let trimmed = line.trim_start();
    let stripped = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("//"))?;
    if stripped.starts_with('#') {
        // ### request separator, not a directive
        return None;
    }
    let body = stripped.trim_start();
    let body = body.strip_prefix('@')?;
    let (name, rest) = match body.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (body, ""),
    };
    if name.is_empty() {
        return None;
    }
    let raw = if rest.is_empty() {
        format!("@{name}")
    } else {
        format!("@{name} {rest}")
    };
    Some(OptionDirective {
        name: name.to_ascii_lowercase(),
        value: if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        },
        raw,
    })
}

fn apply_directive(
    section: &mut SectionDirectives,
    directive: &OptionDirective,
    line: usize,
) -> Result<(), HttpClientError> {
    let options = &mut section.options;
    let assertions = &mut section.assertions;
    let captures = &mut section.captures;
    match directive.name.as_str() {
        "name" => {
            let value =
                directive
                    .value
                    .as_deref()
                    .ok_or_else(|| HttpClientError::InvalidOption {
                        line,
                        content: "@name requires a request name".to_string(),
                    })?;
            if section.name.is_some() {
                return Err(HttpClientError::InvalidOption {
                    line,
                    content: format!("@name is declared more than once (second value: '{value}')"),
                });
            }
            section.name = Some(value.trim().to_string());
        }
        // Already consumed by `parse_default_env`; anywhere else it is inert
        // text we must not lose, so fall through to the unknown branch.
        "env" if section.is_preamble => {}
        "no-redirect" => {
            options.no_redirect = true;
        }
        "timeout" => {
            options.timeout_ms = Some(parse_duration_ms(directive, line)?);
        }
        "connection-timeout" => {
            options.connection_timeout_ms = Some(parse_duration_ms(directive, line)?);
        }
        "fragments" => {
            let path =
                directive
                    .value
                    .as_deref()
                    .ok_or_else(|| HttpClientError::InvalidOption {
                        line,
                        content: "@fragments requires a path".to_string(),
                    })?;
            options.fragment_paths.push(path.to_string());
        }
        "expect-status" => {
            let raw = directive
                .value
                .as_deref()
                .ok_or_else(|| HttpClientError::InvalidOption {
                    line,
                    content: "@expect-status requires at least one status code".to_string(),
                })?;
            let mut codes = Vec::new();
            for chunk in raw.split(|c: char| c == ',' || c.is_whitespace()) {
                let chunk = chunk.trim();
                if chunk.is_empty() {
                    continue;
                }
                let code = chunk
                    .parse::<u16>()
                    .map_err(|_| HttpClientError::InvalidOption {
                        line,
                        content: format!("@expect-status got non-integer code '{chunk}'"),
                    })?;
                codes.push(code);
            }
            if codes.is_empty() {
                return Err(HttpClientError::InvalidOption {
                    line,
                    content: "@expect-status requires at least one status code".to_string(),
                });
            }
            assertions.push(ResponseAssertion::Status { codes, line });
        }
        "expect-header" => {
            let raw = directive
                .value
                .as_deref()
                .ok_or_else(|| HttpClientError::InvalidOption {
                    line,
                    content: "@expect-header requires '<name> <substring>'".to_string(),
                })?;
            let (name, substring) = raw.split_once(char::is_whitespace).ok_or_else(|| {
                HttpClientError::InvalidOption {
                    line,
                    content: "@expect-header requires '<name> <substring>'".to_string(),
                }
            })?;
            assertions.push(ResponseAssertion::Header {
                name: name.trim().to_string(),
                substring: substring.trim().to_string(),
                line,
            });
        }
        "expect-json" => {
            let raw = directive
                .value
                .as_deref()
                .ok_or_else(|| HttpClientError::InvalidOption {
                    line,
                    content: "@expect-json requires '<pointer> <expected>'".to_string(),
                })?;
            let (pointer, expected) = raw.split_once(char::is_whitespace).ok_or_else(|| {
                HttpClientError::InvalidOption {
                    line,
                    content: "@expect-json requires '<pointer> <expected>'".to_string(),
                }
            })?;
            assertions.push(ResponseAssertion::JsonValue {
                pointer: pointer.trim().to_string(),
                expected: expected.trim().to_string(),
                line,
            });
        }
        "capture" => {
            let raw = directive
                .value
                .as_deref()
                .ok_or_else(|| HttpClientError::InvalidOption {
                    line,
                    content: "@capture requires '<variable> <source>'".to_string(),
                })?;
            let (variable, spec) = raw.split_once(char::is_whitespace).ok_or_else(|| {
                HttpClientError::InvalidOption {
                    line,
                    content: "@capture requires '<variable> <source>'".to_string(),
                }
            })?;
            let variable = variable.trim();
            if variable.is_empty() {
                return Err(HttpClientError::InvalidOption {
                    line,
                    content: "@capture variable name was empty".to_string(),
                });
            }
            let source = parse_capture_source(spec.trim(), line)?;
            captures.push(CaptureDirective {
                variable: variable.to_string(),
                source,
                line,
            });
        }
        _ => {
            // Unknown directives have no effect here, mirroring JetBrains'
            // forward-compatible behaviour — but they are kept verbatim so the
            // formatter rewrites the file without deleting them.
            section.unknown.push(directive.raw.clone());
        }
    }
    Ok(())
}

fn parse_capture_source(spec: &str, line: usize) -> Result<CaptureSource, HttpClientError> {
    if spec.eq_ignore_ascii_case("status") {
        return Ok(CaptureSource::Status);
    }
    if let Some(pointer) = spec.strip_prefix("json:") {
        if pointer.is_empty() {
            return Err(HttpClientError::InvalidOption {
                line,
                content: "@capture json: source needs a JSON pointer".to_string(),
            });
        }
        return Ok(CaptureSource::JsonPointer(pointer.to_string()));
    }
    if let Some(name) = spec.strip_prefix("header:") {
        if name.is_empty() {
            return Err(HttpClientError::InvalidOption {
                line,
                content: "@capture header: source needs a header name".to_string(),
            });
        }
        return Ok(CaptureSource::Header(name.to_string()));
    }
    Err(HttpClientError::InvalidOption {
        line,
        content: format!(
            "@capture source must be one of json:<pointer>, header:<name>, status (got '{spec}')"
        ),
    })
}

fn parse_duration_ms(directive: &OptionDirective, line: usize) -> Result<u64, HttpClientError> {
    let raw = directive
        .value
        .as_deref()
        .ok_or_else(|| HttpClientError::InvalidOption {
            line,
            content: format!("@{} requires a millisecond value", directive.name),
        })?;
    raw.parse::<u64>()
        .map_err(|_| HttpClientError::InvalidOption {
            line,
            content: format!(
                "@{} expected a millisecond integer, got '{}'",
                directive.name, raw
            ),
        })
}

type RedirectMatch = Option<(usize, ResponseRedirect)>;
type BodyAndRedirect<'a> = (&'a [NumberedLine<'a>], RedirectMatch);

fn split_off_response_redirect<'a>(
    tail: &'a [NumberedLine<'a>],
) -> Result<BodyAndRedirect<'a>, HttpClientError> {
    let last_redirect = tail
        .iter()
        .rposition(|(_, line)| line.trim_start().starts_with(">>"));
    let Some(idx) = last_redirect else {
        return Ok((tail, None));
    };
    let (line_no, raw) = tail[idx];
    let parsed = parse_response_redirect(raw).ok_or_else(|| HttpClientError::InvalidOption {
        line: line_no,
        content: format!("invalid response redirect: {raw}"),
    })?;
    let body_lines = trim_blank_body_lines(&tail[..idx]);
    Ok((body_lines, Some((line_no, parsed))))
}

fn parse_response_redirect(line: &str) -> Option<ResponseRedirect> {
    let trimmed = line.trim_start();
    // `>>` is a prefix of `>>!`, so the forcing form must be tested first.
    let (rest, force) = match trimmed.strip_prefix(">>!") {
        Some(rest) => (rest, true),
        None => (trimmed.strip_prefix(">>")?, false),
    };
    let path = rest.trim();
    if path.is_empty() {
        return None;
    }
    Some(ResponseRedirect {
        path: path.to_string(),
        force_overwrite: force,
    })
}

fn build_body(body_lines: &[NumberedLine<'_>]) -> Result<Option<RequestBody>, HttpClientError> {
    if body_lines.is_empty() {
        return Ok(None);
    }
    let first_non_blank = body_lines.iter().find(|(_, line)| !line.trim().is_empty());
    if let Some((_, first)) = first_non_blank {
        // `< ./body.json` is a file reference; `<hello/>` is an XML body. The
        // separating whitespace is what tells them apart.
        if let Some(rest) = first.trim_start().strip_prefix('<') {
            let path = rest.trim();
            if rest.starts_with(char::is_whitespace)
                && !path.is_empty()
                && body_lines
                    .iter()
                    .filter(|(_, line)| !line.trim().is_empty())
                    .count()
                    == 1
            {
                return Ok(Some(RequestBody::FromFile {
                    path: path.to_string(),
                }));
            }
        }
    }
    let joined = body_lines
        .iter()
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Some(RequestBody::Inline(joined)))
}

fn trim_blank_body_lines<'a>(lines: &'a [NumberedLine<'a>]) -> &'a [NumberedLine<'a>] {
    let start = lines.iter().position(|(_, line)| !line.trim().is_empty());
    let end = lines.iter().rposition(|(_, line)| !line.trim().is_empty());
    match (start, end) {
        (Some(start), Some(end)) if start <= end => &lines[start..=end],
        _ => &[],
    }
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    (trimmed.starts_with('#') && !trimmed.starts_with("###")) || trimmed.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_get_request() {
        let input = "GET https://example.com\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].method, RequestMethod::Get);
        assert_eq!(file.requests[0].url, "https://example.com");
    }

    #[test]
    fn parses_multiple_requests_separated_by_markers() {
        let input =
            "### One\nGET https://example.com/one\n\n### Two\nPOST https://example.com/two\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 2);
        assert_eq!(file.requests[1].method, RequestMethod::Post);
    }

    #[test]
    fn parses_request_names() {
        let input = "### List users\nGET https://example.com/users\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests[0].name.as_deref(), Some("List users"));
    }

    #[test]
    fn parses_in_place_variables() {
        let input = "@host = https://example.com\n@token = secret\n\n### Ping\nGET {{host}}/ping\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.variables.len(), 2);
        assert_eq!(file.variables[0].name, "host");
        assert_eq!(file.variables[1].value, "secret");
    }

    #[test]
    fn parses_headers() {
        let input = "### Request\nGET https://example.com\nAccept: application/json\nAuthorization: Bearer token\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests[0].headers.len(), 2);
        assert_eq!(file.requests[0].headers[1].name, "Authorization");
    }

    #[test]
    fn parses_json_body() {
        let input = r#"### Create
POST https://example.com
Content-Type: application/json

{
  "name": "Alice"
}
"#;
        let file = parse_request_file(input).unwrap();

        assert!(file.requests[0]
            .body
            .as_ref()
            .and_then(RequestBody::as_inline)
            .unwrap()
            .contains("\"Alice\""));
    }

    #[test]
    fn parses_file_level_env_directive() {
        let input = "# @env dev\n@host = https://example.com\n\n### Ping\nGET {{host}}/ping\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.default_env.as_deref(), Some("dev"));
        assert_eq!(file.variables.len(), 1);
        assert_eq!(file.requests.len(), 1);
    }

    #[test]
    fn env_directive_without_separators_still_parses() {
        let input = "# @env prod\n";
        let file = parse_request_file(input).unwrap();
        assert_eq!(file.default_env.as_deref(), Some("prod"));
        assert!(file.requests.is_empty());
    }

    #[test]
    fn duplicate_env_directive_errors() {
        let input = "# @env dev\n# @env prod\n\n### Ping\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, content } => {
                assert_eq!(line, 2);
                assert!(content.contains("more than once"));
            }
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn env_directive_with_no_value_errors() {
        let input = "# @env\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, .. } => assert_eq!(line, 1),
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn parses_request_options_and_skips_unknown_ones() {
        let input = "### Configured\n# @timeout 1500\n# @connection-timeout 250\n# @no-redirect\n# @future-option ignored\nGET https://example.com\n";
        let file = parse_request_file(input).unwrap();

        let options = &file.requests[0].options;
        assert_eq!(options.timeout_ms, Some(1500));
        assert_eq!(options.connection_timeout_ms, Some(250));
        assert!(options.no_redirect);
    }

    #[test]
    fn parses_capture_directives_for_all_sources() {
        let input = concat!(
            "### Login\n",
            "# @capture token json:/access_token\n",
            "# @capture sid header:Set-Cookie\n",
            "# @capture code status\n",
            "POST https://example.com/login\n",
        );
        let file = parse_request_file(input).unwrap();
        let captures = &file.requests[0].captures;
        assert_eq!(captures.len(), 3);
        assert_eq!(captures[0].variable, "token");
        matches!(captures[0].source, CaptureSource::JsonPointer(ref p) if p == "/access_token");
        assert_eq!(captures[1].variable, "sid");
        matches!(captures[1].source, CaptureSource::Header(ref h) if h == "Set-Cookie");
        assert_eq!(captures[2].variable, "code");
        matches!(captures[2].source, CaptureSource::Status);
    }

    #[test]
    fn rejects_capture_with_missing_value() {
        let input = "### Bad\n# @capture\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, content } => {
                assert_eq!(line, 2);
                assert!(content.contains("@capture requires"));
            }
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_capture_with_unknown_source() {
        let input = "### Bad\n# @capture token body:/foo\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { content, .. } => {
                assert!(content.contains("must be one of"));
            }
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn parses_response_assertions() {
        let input = "### Endpoint\n# @expect-status 200,204\n# @expect-header content-type application/json\n# @expect-json /users/0/name Alice\nGET https://example.com/users\n";
        let file = parse_request_file(input).unwrap();
        let assertions = &file.requests[0].assertions;
        assert_eq!(assertions.len(), 3);
        match &assertions[0] {
            ResponseAssertion::Status { codes, .. } => assert_eq!(codes, &[200, 204]),
            other => panic!("unexpected: {other:?}"),
        }
        match &assertions[1] {
            ResponseAssertion::Header {
                name, substring, ..
            } => {
                assert_eq!(name, "content-type");
                assert_eq!(substring, "application/json");
            }
            other => panic!("unexpected: {other:?}"),
        }
        match &assertions[2] {
            ResponseAssertion::JsonValue {
                pointer, expected, ..
            } => {
                assert_eq!(pointer, "/users/0/name");
                assert_eq!(expected, "Alice");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn name_directive_names_the_request() {
        let input = "###\n# @name login\nPOST https://example.com/login\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests[0].resolved_name(), Some("login"));
        assert_eq!(
            select_request_by_name(&file, "LOGIN").unwrap().url,
            "https://example.com/login"
        );
    }

    #[test]
    fn name_directive_wins_over_the_separator_text_but_both_select() {
        let input = "### Create a user\n# @name createUser\nPOST https://example.com/users\n";
        let file = parse_request_file(input).unwrap();

        let request = &file.requests[0];
        assert_eq!(request.resolved_name(), Some("createUser"));
        assert_eq!(request.name.as_deref(), Some("Create a user"));
        assert!(select_request_by_name(&file, "createUser").is_some());
        assert!(select_request_by_name(&file, "Create a user").is_some());
    }

    #[test]
    fn name_directive_without_a_value_errors() {
        let input = "### Bad\n# @name\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, content } => {
                assert_eq!(line, 2);
                assert!(content.contains("@name requires"));
            }
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_name_directive_errors() {
        let input = "### Bad\n# @name one\n# @name two\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, content } => {
                assert_eq!(line, 3);
                assert!(content.contains("more than once"));
            }
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn unknown_directives_are_kept_verbatim_on_the_block() {
        let input = "### Req\n# @no-cookie-jar\n// @No-Log yes please\nGET https://example.com\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(
            file.requests[0].unknown_directives,
            vec![
                "@no-cookie-jar".to_string(),
                "@No-Log yes please".to_string()
            ]
        );
    }

    #[test]
    fn the_preamble_env_directive_is_not_kept_as_an_unknown_directive() {
        // `# @env dev` is consumed at file level, so a bare first request must
        // not also carry it as an unrecognised directive (it would be emitted
        // twice by the formatter).
        let input = "# @env dev\nGET https://example.com\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.default_env.as_deref(), Some("dev"));
        assert!(file.requests[0].unknown_directives.is_empty());
    }

    #[test]
    fn an_env_directive_outside_the_preamble_is_kept_as_an_unknown_directive() {
        let input = "### One\nGET https://example.com/one\n\n### Two\n# @env prod\nGET https://example.com/two\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.default_env, None);
        assert!(file.requests[0].unknown_directives.is_empty());
        assert_eq!(file.requests[1].unknown_directives, vec!["@env prod"]);
    }

    #[test]
    fn rejects_non_integer_timeout() {
        let input = "### Bad\n# @timeout fast\nGET https://example.com\n";
        let err = parse_request_file(input).unwrap_err();
        match err {
            HttpClientError::InvalidOption { line, .. } => assert_eq!(line, 2),
            other => panic!("expected InvalidOption, got {other:?}"),
        }
    }

    #[test]
    fn parses_body_from_file_directive() {
        let input = "### From file\nPOST https://example.com\nContent-Type: application/json\n\n< ./body.json\n";
        let file = parse_request_file(input).unwrap();

        match file.requests[0].body.as_ref().unwrap() {
            RequestBody::FromFile { path } => assert_eq!(path, "./body.json"),
            other => panic!("expected FromFile body, got {other:?}"),
        }
    }

    #[test]
    fn parses_response_redirect_with_force() {
        let input = "### With redirect\nGET https://example.com\n\n>>! ./out/last.json\n";
        let file = parse_request_file(input).unwrap();

        let redirect = file.requests[0].response_redirect.as_ref().unwrap();
        assert_eq!(redirect.path, "./out/last.json");
        assert!(redirect.force_overwrite);
        assert!(file.requests[0].body.is_none());
    }

    #[test]
    fn keeps_body_separate_from_response_redirect() {
        let input = "### Mixed\nPOST https://example.com\nContent-Type: application/json\n\n{\"name\": \"Alice\"}\n\n>> ./out.json\n";
        let file = parse_request_file(input).unwrap();

        let body = file.requests[0]
            .body
            .as_ref()
            .and_then(RequestBody::as_inline)
            .unwrap();
        assert!(body.contains("Alice"));
        let redirect = file.requests[0].response_redirect.as_ref().unwrap();
        assert_eq!(redirect.path, "./out.json");
        assert!(!redirect.force_overwrite);
    }

    #[test]
    fn selects_request_by_name_ignoring_case_and_padding() {
        let input =
            "### One\nGET https://example.com/one\n\n### Two\nGET https://example.com/two\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(
            select_request_by_name(&file, "  one  ").unwrap().url,
            "https://example.com/one"
        );
        assert_eq!(
            select_request_by_name(&file, "TWO").unwrap().url,
            "https://example.com/two"
        );
        assert!(select_request_by_name(&file, "missing").is_none());
    }

    #[test]
    fn selects_request_by_line_number() {
        let input =
            "### One\nGET https://example.com/one\n\n### Two\nGET https://example.com/two\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(
            select_request_by_line(&file, 5).unwrap().name.as_deref(),
            Some("Two")
        );
    }

    #[test]
    fn keeps_a_bare_first_request_ahead_of_a_separator() {
        let input = "GET https://first.example.com\n\n### Second\nGET https://second.example.com\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 2);
        assert_eq!(file.requests[0].url, "https://first.example.com");
        assert_eq!(file.requests[0].name, None);
        assert_eq!(file.requests[1].name.as_deref(), Some("Second"));
    }

    #[test]
    fn keeps_prelude_variables_alongside_a_bare_first_request() {
        let input =
            "@host = https://example.com\n\nGET {{host}}/ping\n\n### Second\nGET {{host}}/pong\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.variables.len(), 1);
        assert_eq!(file.variables[0].name, "host");
        assert_eq!(file.requests.len(), 2);
    }

    #[test]
    fn strips_http_version_token_from_the_request_line() {
        let input = "### Versioned\nGET https://example.com/a HTTP/1.1\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests[0].url, "https://example.com/a");
    }

    #[test]
    fn keeps_a_url_that_merely_contains_http_in_a_segment() {
        let input = "### Plain\nGET https://example.com/HTTP/1.1\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests[0].url, "https://example.com/HTTP/1.1");
    }

    #[test]
    fn treats_single_line_xml_body_as_inline_not_a_file_reference() {
        let input =
            "### Xml\nPOST https://example.com\nContent-Type: application/xml\n\n<hello/>\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(
            file.requests[0].body,
            Some(RequestBody::Inline("<hello/>".to_string()))
        );
    }

    #[test]
    fn allows_variables_inside_a_section() {
        let input = "### Req\n@host = https://example.com\nGET {{host}}/ping\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].url, "{{host}}/ping");
        assert_eq!(file.variables.len(), 1);
        assert_eq!(file.variables[0].name, "host");
    }

    #[test]
    fn allows_variables_ahead_of_a_bare_request_without_separators() {
        let input = "@host = https://example.com\nGET {{host}}/ping\n";
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].url, "{{host}}/ping");
        assert_eq!(file.variables.len(), 1);
    }

    #[test]
    fn indented_hash_marker_inside_a_body_does_not_split_the_file() {
        let input = concat!(
            "### Notes\n",
            "POST https://example.com/notes\n",
            "Content-Type: text/markdown\n",
            "\n",
            "intro\n",
            "   ### heading\n",
            "rest of the note\n",
        );
        let file = parse_request_file(input).unwrap();

        assert_eq!(file.requests.len(), 1);
        assert_eq!(
            file.requests[0].body,
            Some(RequestBody::Inline(
                "intro\n   ### heading\nrest of the note".to_string()
            ))
        );
    }
}
