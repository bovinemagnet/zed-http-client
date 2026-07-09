//! Cached schema lookup and schema-aware GraphQL validation.
//!
//! `zed-http introspect` writes a schema to
//! `<base>/.zed-http/schema/<slug>.json`; this module is the read side of
//! that cache. The slug derives from the request URL's scheme, host, port,
//! and path, so requests against the same endpoint share one cached schema
//! while two services on different ports of one host do not. Caches written
//! under the older host-only name are still read (see `legacy_schema_slug`).
//!
//! [`validate_against_schema`] walks the *outermost* selection set of a
//! GraphQL operation and reports field selections that aren't declared on
//! the schema's root type for the operation kind. Inline-fragment and
//! fragment-spread selections are intentionally skipped: validating them
//! correctly requires resolving the type condition, which we don't do yet.
//!
//! The selection-set walker (`extract_top_level_fields`) is character-based
//! with a small state machine — enough to handle aliases, arguments,
//! `@directive(...)` clauses, and `#` comments, but not a full GraphQL
//! grammar. Good enough for "schema knows this field name" checks.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use url::Url;

use crate::output::schema_root;

/// Cache key for a GraphQL endpoint. Scheme, port, and path all participate:
/// `localhost:4000/graphql` and `localhost:5000/graphql` are different
/// services and must not share a cached schema.
pub fn schema_slug(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;

    let mut slug = format!("{}-{host}", parsed.scheme());
    if let Some(port) = parsed.port() {
        slug.push('-');
        slug.push_str(&port.to_string());
    }
    let path = parsed.path().trim_matches('/');
    if !path.is_empty() {
        slug.push('-');
        slug.push_str(path);
    }

    Some(
        slug.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect(),
    )
}

/// The host-only key written by releases up to 0.4.9. Read-only: kept so an
/// existing cache keeps working for one release.
fn legacy_schema_slug(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.replace([':', '/'], "-"))
}

pub fn cached_schema_path(
    http_file: &Path,
    worktree_root: Option<&Path>,
    request_url: &str,
) -> Option<PathBuf> {
    let slug = schema_slug(request_url)?;
    Some(schema_root(http_file, worktree_root).join(format!("{slug}.json")))
}

pub fn load_cached_schema(
    http_file: &Path,
    worktree_root: Option<&Path>,
    request_url: &str,
) -> Option<Value> {
    let root = schema_root(http_file, worktree_root);
    let candidates = [schema_slug(request_url), legacy_schema_slug(request_url)];

    candidates
        .into_iter()
        .flatten()
        .map(|slug| root.join(format!("{slug}.json")))
        .find_map(|path| {
            let raw = fs::read_to_string(&path).ok()?;
            serde_json::from_str(&raw).ok()
        })
}

pub fn detect_operation_kind(query: &str) -> &'static str {
    for line in query.lines() {
        let trimmed = line.trim_start();
        for keyword in ["query", "mutation", "subscription"] {
            if let Some(rest) = trimmed.strip_prefix(keyword) {
                if rest
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace() || c == '(' || c == '{')
                    .unwrap_or(true)
                {
                    return match keyword {
                        "mutation" => "mutation",
                        "subscription" => "subscription",
                        _ => "query",
                    };
                }
            }
        }
        // Bare `{` shorthand defaults to query.
        if trimmed.starts_with('{') {
            return "query";
        }
    }
    "query"
}

pub fn validate_against_schema(query: &str, schema: &Value) -> Vec<String> {
    let kind = detect_operation_kind(query);
    let pointer = format!("/__schema/{kind}Type/name");
    let root_name = match schema.pointer(&pointer).and_then(Value::as_str) {
        Some(name) => name,
        None => {
            return vec![format!(
                "schema does not declare a {kind} root type — cannot validate"
            )];
        }
    };

    let Some(types) = schema.pointer("/__schema/types").and_then(Value::as_array) else {
        return vec!["schema is missing /__schema/types — is the cache complete?".to_string()];
    };

    let Some(root_type) = types
        .iter()
        .find(|ty| ty.get("name").and_then(Value::as_str) == Some(root_name))
    else {
        return vec![format!(
            "schema cache is missing root type definition for '{root_name}'"
        )];
    };

    let known: Vec<&str> = root_type
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|f| f.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();

    let mut issues = Vec::new();
    for field in extract_top_level_fields(query) {
        if is_meta_field(&field) {
            continue;
        }
        if !known.iter().any(|known_field| *known_field == field) {
            issues.push(format!(
                "field '{field}' is not declared on '{root_name}' (schema {kind} root type)"
            ));
        }
    }
    issues
}

fn is_meta_field(name: &str) -> bool {
    matches!(name, "__typename" | "__schema" | "__type")
}

fn extract_top_level_fields(query: &str) -> Vec<String> {
    let Some(selection) = outer_selection_set(query) else {
        return Vec::new();
    };

    let mut fields = Vec::new();
    let chars: Vec<char> = selection.chars().collect();
    let mut i = 0usize;
    let mut brace = 0i32;
    let mut paren = 0i32;

    while i < chars.len() {
        let c = chars[i];
        // Strings and comments first: a `}` or `)` inside either is data, not
        // structure, and counting it would truncate the selection set.
        if c == '"' {
            i = skip_string_literal(&chars, i);
            continue;
        }
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        match c {
            '{' => {
                brace += 1;
                i += 1;
                continue;
            }
            '}' => {
                brace -= 1;
                i += 1;
                continue;
            }
            '(' => {
                paren += 1;
                i += 1;
                continue;
            }
            ')' => {
                paren -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        if brace != 0 || paren != 0 {
            i += 1;
            continue;
        }
        if c == '.' {
            // Fragment spread or inline fragment — skip the identifier (or `on Type`) following.
            while i < chars.len() && chars[i] == '.' {
                i += 1;
            }
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            // Skip identifier (might be a fragment name or `on`)
            let mut consumed_first = String::new();
            while i < chars.len() && is_name_char(chars[i]) {
                consumed_first.push(chars[i]);
                i += 1;
            }
            if consumed_first == "on" {
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                while i < chars.len() && is_name_char(chars[i]) {
                    i += 1;
                }
            }
            continue;
        }
        if c == '@' {
            i += 1;
            while i < chars.len() && is_name_char(chars[i]) {
                i += 1;
            }
            continue;
        }
        if is_name_start(c) {
            let start = i;
            while i < chars.len() && is_name_char(chars[i]) {
                i += 1;
            }
            let first =
                selection[char_byte_offset(&chars, start)..char_byte_offset(&chars, i)].to_string();
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i < chars.len() && chars[i] == ':' {
                i += 1;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                let start2 = i;
                while i < chars.len() && is_name_char(chars[i]) {
                    i += 1;
                }
                let actual = selection
                    [char_byte_offset(&chars, start2)..char_byte_offset(&chars, i)]
                    .to_string();
                if !actual.is_empty() {
                    fields.push(actual);
                }
            } else {
                fields.push(first);
            }
            continue;
        }
        i += 1;
    }

    fields
}

/// The operation's own selection set. `fragment Name on Type { ... }` blocks
/// declared ahead of the operation are stepped over — their fields belong to
/// the fragment's type condition, not to the root type.
fn outer_selection_set(query: &str) -> Option<String> {
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0usize;
    let mut skip_next_block = false;

    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            i = skip_string_literal(&chars, i);
            continue;
        }
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if is_name_start(c) {
            let start = i;
            while i < chars.len() && is_name_char(chars[i]) {
                i += 1;
            }
            if chars[start..i].iter().collect::<String>() == "fragment" {
                skip_next_block = true;
            }
            continue;
        }
        if c == '{' {
            let close = matching_brace(&chars, i)?;
            if skip_next_block {
                skip_next_block = false;
                i = close + 1;
                continue;
            }
            return Some(chars[i + 1..close].iter().collect());
        }
        i += 1;
    }

    None
}

/// Index of the `}` closing the `{` at `open`, ignoring braces that occur
/// inside string literals or `#` comments.
fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                i = skip_string_literal(chars, i);
                continue;
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Index just past the string literal beginning at `chars[start]` (a `"`),
/// handling both `"..."` (with `\` escapes) and `"""..."""` block strings.
fn skip_string_literal(chars: &[char], start: usize) -> usize {
    debug_assert_eq!(chars[start], '"');
    const FENCE: [char; 3] = ['"', '"', '"'];

    if chars[start..].starts_with(&FENCE) {
        let mut i = start + 3;
        while i < chars.len() {
            if chars[i..].starts_with(&FENCE) {
                return i + 3;
            }
            i += 1;
        }
        return chars.len();
    }

    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn char_byte_offset(chars: &[char], char_index: usize) -> usize {
    chars[..char_index].iter().map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fake_schema() -> Value {
        json!({
            "__schema": {
                "queryType": { "name": "Query" },
                "mutationType": null,
                "subscriptionType": null,
                "types": [
                    {
                        "kind": "OBJECT",
                        "name": "Query",
                        "fields": [
                            { "name": "user", "type": null },
                            { "name": "viewer", "type": null }
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn detects_operation_kind() {
        assert_eq!(detect_operation_kind("query Foo { x }"), "query");
        assert_eq!(detect_operation_kind("mutation Foo { x }"), "mutation");
        assert_eq!(
            detect_operation_kind("subscription Foo { x }"),
            "subscription"
        );
        assert_eq!(detect_operation_kind("{ x }"), "query");
    }

    #[test]
    fn extracts_top_level_fields_with_alias_and_args() {
        let query =
            "query GetThing($id: ID!) { aliased: user(id: $id) { name } viewer @include(if: true) { id } }";
        let mut fields = extract_top_level_fields(query);
        fields.sort();
        assert_eq!(fields, vec!["user", "viewer"]);
    }

    #[test]
    fn skips_fragment_spreads_and_inline_fragments() {
        // Inline-fragment selections are intentionally skipped at v0.3 fidelity:
        // validating them needs to resolve the fragment's type condition, which we
        // don't do yet. Plain fragment spreads (`...Name`) are likewise skipped.
        let query = "query { user { id } ...UserFragment ... on Query { something } }";
        let fields = extract_top_level_fields(query);
        assert_eq!(fields, vec!["user"]);
    }

    #[test]
    fn flags_unknown_top_level_field() {
        let issues =
            validate_against_schema("query { user { id } bogus { name } }", &fake_schema());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("bogus"));
    }

    #[test]
    fn passes_known_fields() {
        let issues = validate_against_schema("query { user { id } viewer { id } }", &fake_schema());
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn missing_mutation_root_is_reported_when_needed() {
        let issues = validate_against_schema("mutation { update { id } }", &fake_schema());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("mutation root type"));
    }

    #[test]
    fn slug_distinguishes_scheme_port_and_path() {
        assert_eq!(
            schema_slug("https://countries.trevorblades.com/graphql"),
            Some("https-countries.trevorblades.com-graphql".to_string())
        );
        assert_ne!(
            schema_slug("http://localhost:4000/graphql"),
            schema_slug("http://localhost:5000/graphql")
        );
        assert_ne!(
            schema_slug("http://localhost:4000/admin"),
            schema_slug("http://localhost:4000/public")
        );
    }

    #[test]
    fn legacy_slug_preserves_the_host_only_name() {
        assert_eq!(
            legacy_schema_slug("https://countries.trevorblades.com/graphql"),
            Some("countries.trevorblades.com".to_string())
        );
    }

    #[test]
    fn braces_inside_string_arguments_do_not_truncate_the_selection_set() {
        let fields = extract_top_level_fields(r#"query { user(q: "}{") { id } viewer { id } }"#);
        assert_eq!(fields, vec!["user", "viewer"]);
    }

    #[test]
    fn parens_and_hashes_inside_string_arguments_are_literal() {
        let fields = extract_top_level_fields(r#"query { user(q: "a)b # not a comment") { id } }"#);
        assert_eq!(fields, vec!["user"]);
    }

    #[test]
    fn escaped_quotes_and_block_strings_are_skipped() {
        let fields = extract_top_level_fields(r#"query { user(q: "say \"}\" now") { id } }"#);
        assert_eq!(fields, vec!["user"]);

        let block = extract_top_level_fields("query { user(q: \"\"\"a } b\"\"\") { id } }");
        assert_eq!(block, vec!["user"]);
    }

    #[test]
    fn valid_query_with_braces_in_a_string_is_not_flagged() {
        let issues = validate_against_schema(r#"query { user(q: "}{") { id } }"#, &fake_schema());
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn a_fragment_defined_before_the_operation_is_not_the_outer_selection_set() {
        let query = "fragment Details on User { id name } query { user { ...Details } }";
        let fields = extract_top_level_fields(query);
        assert_eq!(fields, vec!["user"]);

        let issues = validate_against_schema(query, &fake_schema());
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }
}
