use crate::{
    graphql::{build_graphql_payload, parse_variable_definitions, validate_variables},
    model::{RequestBlock, RequestBody, RequestFile, RequestMethod},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub request_index: usize,
    pub request_name: Option<String>,
    pub line: usize,
    pub message: String,
}

pub fn validate_request_file(file: &RequestFile) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    for (idx, request) in file.requests.iter().enumerate() {
        for message in validate_request_messages(request) {
            issues.push(ValidationIssue {
                request_index: idx,
                request_name: request.name.clone(),
                line: request.range.start_line,
                message,
            });
        }
    }
    issues
}

pub fn validate_request(request: &RequestBlock) -> Vec<ValidationIssue> {
    validate_request_messages(request)
        .into_iter()
        .map(|message| ValidationIssue {
            request_index: 0,
            request_name: request.name.clone(),
            line: request.range.start_line,
            message,
        })
        .collect()
}

fn validate_request_messages(request: &RequestBlock) -> Vec<String> {
    let mut messages = Vec::new();

    if request.url.trim().is_empty() {
        messages.push("request URL is empty".to_string());
    }

    if request.method == RequestMethod::GraphQl {
        match request.body.as_ref() {
            None => messages.push("GRAPHQL request has no body".to_string()),
            Some(RequestBody::FromFile { .. }) => {
                // Cannot validate without reading the file. Skip — executor will surface read errors.
            }
            Some(RequestBody::Inline(body)) => match build_graphql_payload(body) {
                Err(err) => messages.push(format!("GRAPHQL body did not parse: {err}")),
                Ok(payload) => match parse_variable_definitions(&payload.query) {
                    Err(err) => {
                        messages.push(format!("could not parse variable definitions: {err}"))
                    }
                    Ok(defs) => {
                        let issues = validate_variables(&defs, payload.variables.as_ref());
                        messages.extend(issues);
                    }
                },
            },
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_request_file;

    #[test]
    fn flags_missing_required_graphql_variable() {
        let input = "### GetUser\nGRAPHQL https://example.com/graphql\n\nquery GetUser($id: ID!) { user(id: $id) { id } }\n\n{}\n";
        let file = parse_request_file(input).unwrap();
        let issues = validate_request_file(&file);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("$id"));
        assert_eq!(issues[0].line, 2);
    }

    #[test]
    fn happy_path_passes_validation() {
        let input = "### GetUser\nGRAPHQL https://example.com/graphql\n\nquery GetUser($id: ID!) { user(id: $id) { id } }\n\n{ \"id\": \"42\" }\n";
        let file = parse_request_file(input).unwrap();
        let issues = validate_request_file(&file);
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn flags_extra_variable() {
        let input = "### GetUser\nGRAPHQL https://example.com/graphql\n\nquery GetUser($id: ID!) { user(id: $id) { id } }\n\n{ \"id\": \"42\", \"unused\": true }\n";
        let file = parse_request_file(input).unwrap();
        let issues = validate_request_file(&file);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("'unused'"));
    }
}
