//! Re-emits a parsed [`RequestFile`] in a canonical layout, used by
//! `zed-http format` and by the Postman importer.
//!
//! Round-trip target: anything the parser can produce should be
//! re-emittable here, and re-parsing the output should give an equal
//! `RequestFile`. The one explicit loss is non-directive comments inside
//! a request — the parser discards them, so the formatter can't put them
//! back. Document-level comments outside requests are likewise dropped.

use crate::model::{
    RequestBlock, RequestBody, RequestFile, RequestOptions, ResponseAssertion, ResponseRedirect,
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

fn format_request(output: &mut String, request: &RequestBlock) {
    output.push_str("###");
    if let Some(name) = &request.name {
        output.push(' ');
        output.push_str(name);
    }
    output.push('\n');

    format_options(output, &request.options);
    format_assertions(output, &request.assertions);

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
}
