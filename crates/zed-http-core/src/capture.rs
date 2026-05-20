//! Evaluates `# @capture` directives against a captured response and
//! produces a [`VariableMap`] suitable for layering on top of an env-file
//! overlay.
//!
//! Three source forms (parsed by [`crate::parser`]):
//!
//! - `json:<pointer>` — JSON Pointer into the response body. String,
//!   number, bool, and null are stringified as you'd expect; arrays and
//!   objects are re-serialised with `serde_json::to_string` so they can
//!   still be threaded through `{{var}}` (mostly useful when the next
//!   request wants the raw JSON snippet).
//! - `header:<name>` — case-insensitive header lookup. Multiple headers
//!   with the same name are joined with `, ` (matches the standard
//!   "merge as comma-separated" behaviour HTTP uses for cacheable
//!   headers).
//! - `status` — three-digit status code as a string.
//!
//! Unresolvable captures (pointer doesn't resolve, header missing,
//! response body isn't JSON) are surfaced as [`CaptureWarning`] entries.
//! The CLI prints them but does not abort — by design, since a missing
//! capture is far more often a flaky upstream than a fatal config bug.

use serde_json::Value;

use crate::{
    assertion::AssertionResponse,
    env::VariableMap,
    model::{CaptureDirective, CaptureSource},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureWarning {
    pub variable: String,
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct CaptureOutcome {
    pub captured: VariableMap,
    pub warnings: Vec<CaptureWarning>,
}

pub fn evaluate_captures(
    captures: &[CaptureDirective],
    response: &AssertionResponse<'_>,
) -> CaptureOutcome {
    let mut outcome = CaptureOutcome::default();
    let mut cached_json: Option<Result<Value, serde_json::Error>> = None;

    for directive in captures {
        match &directive.source {
            CaptureSource::Status => {
                outcome
                    .captured
                    .insert(directive.variable.clone(), response.status.to_string());
            }
            CaptureSource::Header(name) => {
                let joined: Vec<&str> = response
                    .headers
                    .iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.as_str())
                    .collect();
                if joined.is_empty() {
                    outcome.warnings.push(CaptureWarning {
                        variable: directive.variable.clone(),
                        line: directive.line,
                        message: format!("header '{name}' was not present"),
                    });
                } else {
                    outcome
                        .captured
                        .insert(directive.variable.clone(), joined.join(", "));
                }
            }
            CaptureSource::JsonPointer(pointer) => {
                let parsed =
                    cached_json.get_or_insert_with(|| serde_json::from_str::<Value>(response.body));
                match parsed {
                    Err(err) => outcome.warnings.push(CaptureWarning {
                        variable: directive.variable.clone(),
                        line: directive.line,
                        message: format!("response body was not JSON: {err}"),
                    }),
                    Ok(value) => match value.pointer(pointer) {
                        None => outcome.warnings.push(CaptureWarning {
                            variable: directive.variable.clone(),
                            line: directive.line,
                            message: format!("JSON pointer '{pointer}' did not resolve"),
                        }),
                        Some(found) => {
                            outcome
                                .captured
                                .insert(directive.variable.clone(), stringify_value(found));
                        }
                    },
                }
            }
        }
    }

    outcome
}

fn stringify_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
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
    fn captures_status_code_as_string() {
        let directives = vec![CaptureDirective {
            variable: "code".to_string(),
            source: CaptureSource::Status,
            line: 1,
        }];
        let outcome = evaluate_captures(
            &directives,
            &AssertionResponse {
                status: 201,
                headers: &[],
                body: "",
            },
        );
        assert_eq!(outcome.captured.get("code").unwrap(), "201");
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn captures_header_case_insensitive_and_joins_duplicates() {
        let hdrs = headers(&[
            ("Set-Cookie", "session=abc"),
            ("set-cookie", "tracking=xyz"),
        ]);
        let directives = vec![CaptureDirective {
            variable: "cookies".to_string(),
            source: CaptureSource::Header("Set-Cookie".to_string()),
            line: 2,
        }];
        let outcome = evaluate_captures(
            &directives,
            &AssertionResponse {
                status: 200,
                headers: &hdrs,
                body: "",
            },
        );
        assert_eq!(
            outcome.captured.get("cookies").unwrap(),
            "session=abc, tracking=xyz"
        );
    }

    #[test]
    fn warns_on_missing_header() {
        let directives = vec![CaptureDirective {
            variable: "x".to_string(),
            source: CaptureSource::Header("X-Missing".to_string()),
            line: 3,
        }];
        let outcome = evaluate_captures(
            &directives,
            &AssertionResponse {
                status: 200,
                headers: &[],
                body: "",
            },
        );
        assert!(outcome.captured.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].message.contains("X-Missing"));
    }

    #[test]
    fn captures_json_pointer_strings_numbers_and_objects() {
        let body = r#"{"token":"abc","attempts":3,"profile":{"id":"u1"}}"#;
        let directives = vec![
            CaptureDirective {
                variable: "token".to_string(),
                source: CaptureSource::JsonPointer("/token".to_string()),
                line: 4,
            },
            CaptureDirective {
                variable: "attempts".to_string(),
                source: CaptureSource::JsonPointer("/attempts".to_string()),
                line: 5,
            },
            CaptureDirective {
                variable: "profile".to_string(),
                source: CaptureSource::JsonPointer("/profile".to_string()),
                line: 6,
            },
        ];
        let outcome = evaluate_captures(
            &directives,
            &AssertionResponse {
                status: 200,
                headers: &[],
                body,
            },
        );
        assert_eq!(outcome.captured.get("token").unwrap(), "abc");
        assert_eq!(outcome.captured.get("attempts").unwrap(), "3");
        assert_eq!(outcome.captured.get("profile").unwrap(), "{\"id\":\"u1\"}");
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn warns_when_pointer_unresolvable_or_body_not_json() {
        let directives = vec![
            CaptureDirective {
                variable: "x".to_string(),
                source: CaptureSource::JsonPointer("/missing".to_string()),
                line: 7,
            },
            CaptureDirective {
                variable: "y".to_string(),
                source: CaptureSource::JsonPointer("/foo".to_string()),
                line: 8,
            },
        ];
        let outcome = evaluate_captures(
            &directives,
            &AssertionResponse {
                status: 200,
                headers: &[],
                body: "<html>",
            },
        );
        assert_eq!(outcome.warnings.len(), 2);
        assert!(outcome
            .warnings
            .iter()
            .any(|w| w.message.contains("was not JSON")));
    }
}
