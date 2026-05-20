use crate::{
    error::HttpClientError,
    model::{
        Header, InPlaceVariable, RequestBlock, RequestBody, RequestFile, RequestMethod,
        RequestOptions, ResponseAssertion, ResponseRedirect, SourceRange,
    },
};

type NumberedLine<'a> = (usize, &'a str);

pub fn parse_request_file(input: &str) -> Result<RequestFile, HttpClientError> {
    let lines: Vec<NumberedLine<'_>> = input
        .lines()
        .enumerate()
        .map(|(idx, line)| (idx + 1, line))
        .collect();

    let mut requests = Vec::new();
    let mut variables = Vec::new();
    let mut section_start = 0usize;
    let mut pending_name: Option<String> = None;
    let mut seen_separator = false;

    for (idx, (_, line)) in lines.iter().enumerate() {
        if line.trim_start().starts_with("###") {
            if !seen_separator {
                variables.extend(parse_variables(&lines[0..idx]));
            } else if let Some(request) =
                parse_section(&lines[section_start..idx], pending_name.take())?
            {
                requests.push(request);
            }

            seen_separator = true;
            section_start = idx + 1;
            pending_name = parse_request_name(line);
        }
    }

    if seen_separator {
        if let Some(request) = parse_section(&lines[section_start..], pending_name)? {
            requests.push(request);
        }
    } else {
        let prelude_vars = parse_variables(&lines);
        if prelude_vars.len() == lines.len()
            || lines
                .iter()
                .all(|(_, line)| line.trim().is_empty() || is_comment(line))
        {
            variables = prelude_vars;
        } else {
            variables = prelude_vars;
            if let Some(request) = parse_section(&lines, None)? {
                requests.push(request);
            }
        }
    }

    Ok(RequestFile {
        variables,
        requests,
    })
}

pub fn select_request_by_line(file: &RequestFile, line: usize) -> Option<&RequestBlock> {
    file.requests
        .iter()
        .find(|request| request.range.start_line <= line && line <= request.range.end_line)
}

pub fn select_request_by_name<'a>(file: &'a RequestFile, name: &str) -> Option<&'a RequestBlock> {
    let needle = name.trim();
    file.requests.iter().find(|request| {
        request
            .name
            .as_deref()
            .map(|candidate| candidate.eq_ignore_ascii_case(needle))
            .unwrap_or(false)
    })
}

fn parse_variables(lines: &[NumberedLine<'_>]) -> Vec<InPlaceVariable> {
    lines
        .iter()
        .filter_map(|(line_no, line)| parse_variable(*line_no, line))
        .collect()
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

fn parse_section(
    lines: &[NumberedLine<'_>],
    name: Option<String>,
) -> Result<Option<RequestBlock>, HttpClientError> {
    let mut cursor = 0usize;
    let mut options = RequestOptions::default();
    let mut assertions: Vec<ResponseAssertion> = Vec::new();
    while cursor < lines.len() {
        let (line_no, line) = lines[cursor];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        if let Some(directive) = parse_option_directive(line) {
            apply_directive(&mut options, &mut assertions, &directive, line_no)?;
            cursor += 1;
            continue;
        }
        if is_comment(line) {
            cursor += 1;
            continue;
        }
        break;
    }

    if cursor >= lines.len() {
        return Ok(None);
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
    let url = url_text.trim().to_string();

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

    Ok(Some(RequestBlock {
        name,
        method,
        url,
        headers,
        body,
        options,
        assertions,
        response_redirect,
        range: SourceRange {
            start_line: request_line_no,
            end_line,
        },
    }))
}

#[derive(Debug, Clone)]
struct OptionDirective {
    name: String,
    value: Option<String>,
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
    Some(OptionDirective {
        name: name.to_ascii_lowercase(),
        value: if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        },
    })
}

fn apply_directive(
    options: &mut RequestOptions,
    assertions: &mut Vec<ResponseAssertion>,
    directive: &OptionDirective,
    line: usize,
) -> Result<(), HttpClientError> {
    match directive.name.as_str() {
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
        _ => {
            // Unknown directives are ignored, mirroring JetBrains' forward-compatible behaviour.
        }
    }
    Ok(())
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
    let (rest, force) = if let Some(rest) = trimmed.strip_prefix(">>!") {
        (rest, true)
    } else if let Some(rest) = trimmed.strip_prefix(">>") {
        (rest, false)
    } else {
        return None;
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
        if let Some(path) = first.trim_start().strip_prefix('<') {
            let path = path.trim();
            if !path.is_empty()
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
    fn parses_request_options_and_skips_unknown_ones() {
        let input = "### Configured\n# @timeout 1500\n# @connection-timeout 250\n# @no-redirect\n# @future-option ignored\nGET https://example.com\n";
        let file = parse_request_file(input).unwrap();

        let options = &file.requests[0].options;
        assert_eq!(options.timeout_ms, Some(1500));
        assert_eq!(options.connection_timeout_ms, Some(250));
        assert!(options.no_redirect);
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
}
