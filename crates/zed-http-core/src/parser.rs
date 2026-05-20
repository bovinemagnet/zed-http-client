use crate::{
    error::HttpClientError,
    model::{Header, InPlaceVariable, RequestBlock, RequestFile, RequestMethod, SourceRange},
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
    while cursor < lines.len() {
        let (_, line) = lines[cursor];
        if line.trim().is_empty() || is_comment(line) {
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
        if let Some((header_name, header_value)) = line.split_once(':') {
            headers.push(Header {
                name: header_name.trim().to_string(),
                value: header_value.trim().to_string(),
                line: line_no,
            });
        }
        cursor += 1;
    }

    let body_lines = trim_blank_body_lines(&lines[cursor..]);
    let end_line = body_lines
        .last()
        .map(|(line_no, _)| *line_no)
        .or_else(|| headers.last().map(|header| header.line))
        .unwrap_or(request_line_no);
    let body = if body_lines.is_empty() {
        None
    } else {
        Some(
            body_lines
                .iter()
                .map(|(_, line)| *line)
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
        )
    };

    Ok(Some(RequestBlock {
        name,
        method,
        url,
        headers,
        body,
        range: SourceRange {
            start_line: request_line_no,
            end_line,
        },
    }))
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
            .unwrap()
            .contains("\"Alice\""));
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
