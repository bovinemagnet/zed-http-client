use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::HttpClientError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableDefinition {
    pub name: String,
    pub type_ref: String,
    pub required: bool,
}

pub fn parse_variable_definitions(query: &str) -> Result<Vec<VariableDefinition>, HttpClientError> {
    let Some(group) = find_variable_group(query) else {
        return Ok(Vec::new());
    };
    let parts = split_top_level_commas(group);
    let mut defs = Vec::with_capacity(parts.len());
    for part in parts {
        let entry = part.trim();
        if entry.is_empty() {
            continue;
        }
        let after_dollar = entry.strip_prefix('$').ok_or_else(|| {
            HttpClientError::Message(format!("variable definition missing '$': {entry}"))
        })?;
        let (name, after_name) = after_dollar.split_once(':').ok_or_else(|| {
            HttpClientError::Message(format!("variable definition missing ':': {entry}"))
        })?;
        let (type_ref_raw, default) = match after_name.split_once('=') {
            Some((type_part, default)) => (type_part.trim(), Some(default.trim())),
            None => (after_name.trim(), None),
        };
        let required = type_ref_raw.ends_with('!') && default.is_none();
        defs.push(VariableDefinition {
            name: name.trim().to_string(),
            type_ref: type_ref_raw.to_string(),
            required,
        });
    }
    Ok(defs)
}

fn find_variable_group(query: &str) -> Option<&str> {
    let first_brace = query.find('{').unwrap_or(query.len());
    let open = query[..first_brace].find('(')?;
    let mut depth = 0usize;
    for (idx, ch) in query[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&query[open + 1..open + idx]);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in input.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&input[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

pub fn validate_variables(defs: &[VariableDefinition], variables: Option<&Value>) -> Vec<String> {
    let mut issues = Vec::new();
    let provided = variables.and_then(Value::as_object);

    for def in defs {
        let value = provided.and_then(|object| object.get(&def.name));
        match value {
            None if def.required => {
                issues.push(format!(
                    "required variable ${} ({}) is missing from the variables block",
                    def.name, def.type_ref
                ));
            }
            Some(Value::Null) if def.required => {
                issues.push(format!(
                    "required variable ${} ({}) was provided as null",
                    def.name, def.type_ref
                ));
            }
            _ => {}
        }
    }

    if let Some(object) = provided {
        for key in object.keys() {
            if !defs.iter().any(|def| def.name == *key) {
                issues.push(format!(
                    "variable '{key}' is provided but not declared on the operation"
                ));
            }
        }
    } else if variables.is_some() && !defs.is_empty() {
        issues.push("variables block must be a JSON object".to_string());
    }

    issues
}

pub const INTROSPECTION_QUERY: &str = r#"query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
    directives {
      name
      description
      locations
      args { ...InputValue }
    }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType { kind name }
            }
          }
        }
      }
    }
  }
}
"#;

pub fn introspection_payload() -> String {
    json!({
        "query": INTROSPECTION_QUERY,
        "operationName": "IntrospectionQuery"
    })
    .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphQlPayload {
    pub query: String,
    pub variables: Option<Value>,
    pub operation_name: Option<String>,
}

pub fn build_graphql_payload(body: &str) -> Result<GraphQlPayload, HttpClientError> {
    let (query, variables) = split_query_and_variables(body)?;
    let operation_name = detect_operation_name(&query);

    Ok(GraphQlPayload {
        query: query.trim().to_string(),
        variables,
        operation_name,
    })
}

pub fn render_graphql_json(body: &str) -> Result<String, HttpClientError> {
    render_graphql_json_with_extras(body, "")
}

pub fn render_graphql_json_with_extras(
    body: &str,
    extra_query: &str,
) -> Result<String, HttpClientError> {
    let mut payload = build_graphql_payload(body)?;
    let trimmed = extra_query.trim();
    if !trimmed.is_empty() {
        payload.query.push_str("\n\n");
        payload.query.push_str(trimmed);
    }
    let mut object = Map::new();
    object.insert("query".to_string(), Value::String(payload.query));
    if let Some(variables) = payload.variables {
        object.insert("variables".to_string(), variables);
    }
    if let Some(operation_name) = payload.operation_name {
        object.insert("operationName".to_string(), json!(operation_name));
    }
    Ok(serde_json::to_string(&Value::Object(object))?)
}

fn split_query_and_variables(body: &str) -> Result<(String, Option<Value>), HttpClientError> {
    let lines: Vec<&str> = body.lines().collect();
    for index in (0..lines.len()).rev() {
        if !lines[index].trim_start().starts_with('{') {
            continue;
        }
        if index > 0 && !lines[index - 1].trim().is_empty() {
            continue;
        }
        let candidate = lines[index..].join(
            "
",
        );
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            let query = lines[..index].join(
                "
",
            );
            return Ok((query, Some(value)));
        }
    }

    Ok((body.to_string(), None))
}

fn detect_operation_name(query: &str) -> Option<String> {
    let regex = Regex::new(r"(?m)^\s*(query|mutation|subscription)\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid graphql regex");
    regex
        .captures(query)
        .and_then(|captures| captures.get(2))
        .map(|value| value.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_variable_definitions_with_required_and_default() {
        let query = "query GetUser($id: ID!, $limit: Int = 10, $tag: String) { user { id } }";
        let defs = parse_variable_definitions(query).unwrap();

        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, "id");
        assert_eq!(defs[0].type_ref, "ID!");
        assert!(defs[0].required);
        assert_eq!(defs[1].name, "limit");
        assert!(!defs[1].required, "defaulted variables are not required");
        assert!(!defs[2].required, "nullable variables are not required");
    }

    #[test]
    fn parses_anonymous_operation_with_variables() {
        let query = "query ($id: ID!) { node(id: $id) { id } }";
        let defs = parse_variable_definitions(query).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "id");
    }

    #[test]
    fn returns_empty_for_no_variable_group() {
        let query = "query { viewer { name } }";
        let defs = parse_variable_definitions(query).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn validates_required_and_extra_variables() {
        let defs = vec![
            VariableDefinition {
                name: "id".to_string(),
                type_ref: "ID!".to_string(),
                required: true,
            },
            VariableDefinition {
                name: "limit".to_string(),
                type_ref: "Int".to_string(),
                required: false,
            },
        ];

        let issues = validate_variables(&defs, Some(&json!({ "limit": 5 })));
        assert!(issues.iter().any(|i| i.contains("$id")));
        assert_eq!(issues.len(), 1);

        let issues = validate_variables(&defs, Some(&json!({ "id": null })));
        assert!(issues.iter().any(|i| i.contains("provided as null")));

        let issues = validate_variables(&defs, Some(&json!({ "id": "x", "stray": 1 })));
        assert!(issues.iter().any(|i| i.contains("'stray'")));
    }

    #[test]
    fn parses_graphql_body_with_variables() {
        let input = r#"query GetUser($id: ID!) {
  user(id: $id) {
    id
    name
  }
}

{
  "id": "123"
}"#;

        let payload = build_graphql_payload(input).unwrap();

        assert_eq!(payload.operation_name.as_deref(), Some("GetUser"));
        assert_eq!(payload.variables, Some(json!({ "id": "123" })));
    }
}
