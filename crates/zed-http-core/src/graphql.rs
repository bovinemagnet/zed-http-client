use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::HttpClientError;

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
    let payload = build_graphql_payload(body)?;
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
