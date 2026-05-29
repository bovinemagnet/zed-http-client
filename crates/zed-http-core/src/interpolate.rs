//! `{{name}}` substitution into request URLs, headers, bodies, and option
//! values.
//!
//! Two entry points:
//!
//! - [`resolve_variables`] runs first against the variable map so values can
//!   reference other variables (e.g. `baseUrl = {{host}}/api`). Missing
//!   references inside a value are left as the literal `{{...}}` text — we
//!   don't want a typo in one private value to cascade into a hard failure
//!   for unrelated requests. The pass is repeated up to `values.len()` times
//!   so chains of indirection eventually settle.
//! - [`interpolate_text`] then runs against the resolved map for the URL,
//!   each header, the body, the response-redirect path, etc. Missing
//!   references at this stage *are* hard errors because they would otherwise
//!   send the literal text "{{token}}" to the server.

use indexmap::IndexMap;
use regex::Regex;

use crate::{env::VariableMap, error::HttpClientError};

pub fn resolve_variables(values: &VariableMap) -> VariableMap {
    let mut resolved = values.clone();
    for _ in 0..values.len().max(1) {
        let snapshot = resolved.clone();
        for (key, value) in &snapshot {
            let next = interpolate_impl(value, &snapshot, false).unwrap_or_else(|_| value.clone());
            resolved.insert(key.clone(), next);
        }
    }
    resolved
}

pub fn interpolate_text(text: &str, values: &VariableMap) -> Result<String, HttpClientError> {
    interpolate_impl(text, values, true)
}

/// The `{{name}}` matcher, compiled once and reused across calls.
///
/// `$`-prefixed names are reserved for dynamic variables ($uuid,
/// $timestamp, ...) and must be allowed both as the leading character and
/// inside the name (a user could shadow `$timestamp` with
/// `@$timestamp = 1700000000` for deterministic tests).
fn interpolation_regex() -> &'static Regex {
    static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\{\{\s*([$A-Za-z_][$A-Za-z0-9_.-]*)\s*\}\}")
            .expect("valid interpolation regex")
    })
}

fn interpolate_impl(
    text: &str,
    values: &IndexMap<String, String>,
    fail_on_missing: bool,
) -> Result<String, HttpClientError> {
    let regex = interpolation_regex();
    let mut output = String::with_capacity(text.len());
    let mut last_end = 0usize;

    for captures in regex.captures_iter(text) {
        let matched = captures.get(0).expect("full match");
        let name = captures.get(1).expect("capture").as_str();
        output.push_str(&text[last_end..matched.start()]);
        if let Some(value) = values.get(name) {
            output.push_str(value);
        } else if fail_on_missing {
            return Err(HttpClientError::MissingVariable(name.to_string()));
        } else {
            output.push_str(matched.as_str());
        }
        last_end = matched.end();
    }

    output.push_str(&text[last_end..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_variables_in_text() {
        let values = VariableMap::from_iter([
            ("host".to_string(), "https://example.com".to_string()),
            ("path".to_string(), "/users".to_string()),
        ]);

        let rendered = interpolate_text("{{host}}{{path}}", &values).unwrap();

        assert_eq!(rendered, "https://example.com/users");
    }

    #[test]
    fn resolves_nested_variable_references() {
        let values = VariableMap::from_iter([
            ("host".to_string(), "https://example.com".to_string()),
            ("baseUrl".to_string(), "{{host}}/api".to_string()),
        ]);

        let resolved = resolve_variables(&values);

        assert_eq!(
            resolved.get("baseUrl").map(String::as_str),
            Some("https://example.com/api")
        );
    }

    #[test]
    fn interpolate_text_fails_on_unknown_variable() {
        let values = VariableMap::new();

        let result = interpolate_text("GET {{missing}}", &values);

        assert!(matches!(
            result,
            Err(HttpClientError::MissingVariable(name)) if name == "missing"
        ));
    }
}
