use crate::model::{RequestBlock, RequestBody, RequestFile, RequestOptions, ResponseRedirect};

pub fn format_request_file(file: &RequestFile) -> String {
    let mut output = String::new();

    for variable in &file.variables {
        output.push_str(&format!("@{} = {}\n", variable.name, variable.value));
    }
    if !file.variables.is_empty() && !file.requests.is_empty() {
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
    fn places_blank_line_between_requests() {
        let input =
            "### One\nGET https://example.com/one\n\n### Two\nGET https://example.com/two\n";
        let parsed = parse_request_file(input).unwrap();

        let formatted = format_request_file(&parsed);

        assert!(formatted.contains("\n\n### Two"));
    }
}
