use serde_json::Value;

use crate::model::ResponseAssertion;

#[derive(Debug, Clone)]
pub struct AssertionResponse<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionFailure {
    pub line: usize,
    pub message: String,
}

pub fn evaluate_assertions(
    assertions: &[ResponseAssertion],
    response: &AssertionResponse<'_>,
) -> Vec<AssertionFailure> {
    let mut failures = Vec::new();
    let mut cached_json: Option<Result<Value, serde_json::Error>> = None;

    for assertion in assertions {
        match assertion {
            ResponseAssertion::Status { codes, line } => {
                if !codes.contains(&response.status) {
                    let expected = codes
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(",");
                    failures.push(AssertionFailure {
                        line: *line,
                        message: format!(
                            "expected status in [{expected}], got {}",
                            response.status
                        ),
                    });
                }
            }
            ResponseAssertion::Header {
                name,
                substring,
                line,
            } => {
                let actual = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.as_str());
                match actual {
                    None => failures.push(AssertionFailure {
                        line: *line,
                        message: format!("expected header '{name}' to be present"),
                    }),
                    Some(value) if !value.contains(substring) => failures.push(AssertionFailure {
                        line: *line,
                        message: format!(
                            "header '{name}' did not contain '{substring}' (was '{value}')"
                        ),
                    }),
                    _ => {}
                }
            }
            ResponseAssertion::JsonValue {
                pointer,
                expected,
                line,
            } => {
                let parsed =
                    cached_json.get_or_insert_with(|| serde_json::from_str::<Value>(response.body));
                match parsed {
                    Err(err) => failures.push(AssertionFailure {
                        line: *line,
                        message: format!("response body was not JSON: {err}"),
                    }),
                    Ok(value) => {
                        let resolved = value.pointer(pointer);
                        match resolved {
                            None => failures.push(AssertionFailure {
                                line: *line,
                                message: format!("JSON pointer '{pointer}' did not resolve"),
                            }),
                            Some(found) if !value_matches(found, expected) => {
                                failures.push(AssertionFailure {
                                    line: *line,
                                    message: format!(
                                        "JSON pointer '{pointer}' was {}, expected '{expected}'",
                                        format_json_value(found)
                                    ),
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    failures
}

fn value_matches(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(s) => s == expected,
        Value::Number(n) => n.to_string() == expected,
        Value::Bool(b) => b.to_string() == expected,
        Value::Null => expected == "null",
        Value::Array(_) | Value::Object(_) => false,
    }
}

fn format_json_value(value: &Value) -> String {
    match value {
        Value::String(s) => format!("'{s}'"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn status_assertion_passes_and_fails() {
        let pass = evaluate_assertions(
            &[ResponseAssertion::Status {
                codes: vec![200, 201],
                line: 2,
            }],
            &AssertionResponse {
                status: 200,
                headers: &[],
                body: "",
            },
        );
        assert!(pass.is_empty());

        let fail = evaluate_assertions(
            &[ResponseAssertion::Status {
                codes: vec![200],
                line: 2,
            }],
            &AssertionResponse {
                status: 500,
                headers: &[],
                body: "",
            },
        );
        assert_eq!(fail.len(), 1);
        assert!(fail[0].message.contains("got 500"));
    }

    #[test]
    fn header_assertion_is_case_insensitive_and_substring() {
        let hdrs = headers(&[("Content-Type", "application/json; charset=utf-8")]);
        let pass = evaluate_assertions(
            &[ResponseAssertion::Header {
                name: "content-type".to_string(),
                substring: "application/json".to_string(),
                line: 3,
            }],
            &AssertionResponse {
                status: 200,
                headers: &hdrs,
                body: "",
            },
        );
        assert!(pass.is_empty());
    }

    #[test]
    fn json_pointer_assertion() {
        let body = r#"{"users":[{"name":"Alice"},{"name":"Bob"}]}"#;
        let pass = evaluate_assertions(
            &[ResponseAssertion::JsonValue {
                pointer: "/users/0/name".to_string(),
                expected: "Alice".to_string(),
                line: 4,
            }],
            &AssertionResponse {
                status: 200,
                headers: &[],
                body,
            },
        );
        assert!(pass.is_empty());

        let fail = evaluate_assertions(
            &[ResponseAssertion::JsonValue {
                pointer: "/users/0/name".to_string(),
                expected: "Bob".to_string(),
                line: 4,
            }],
            &AssertionResponse {
                status: 200,
                headers: &[],
                body,
            },
        );
        assert_eq!(fail.len(), 1);
        assert!(fail[0].message.contains("'Alice'"));
    }
}
