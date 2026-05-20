//! Postman v2.1 collection → [`RequestFile`] importer.
//!
//! Walks the recursive `item` tree, flattening nested folders into request
//! names like `Folder / Sub-Folder / Request`. Handles `raw` and `graphql`
//! body modes; `formdata`, `urlencoded`, `file`, and `binary` modes are
//! skipped (we don't yet support those on the execution side). Collection
//! variables are imported as in-file `@name = value` declarations so they
//! travel with the resulting `.http` file.
//!
//! Postman's variable syntax (`{{var}}`) happens to match the format we
//! already use, so URLs and headers pass through unchanged.

use serde_json::Value;

use crate::{
    error::HttpClientError,
    model::{
        Header, RequestBlock, RequestBody, RequestFile, RequestMethod, RequestOptions, SourceRange,
    },
};

pub fn import_collection(input: &str) -> Result<RequestFile, HttpClientError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|e| HttpClientError::Message(format!("Postman JSON parse error: {e}")))?;
    let items = value.get("item").and_then(Value::as_array).ok_or_else(|| {
        HttpClientError::Message("Postman collection missing 'item' array".to_string())
    })?;

    let mut requests = Vec::new();
    walk_items(items, None, &mut requests);

    let variables = match value.get("variable").and_then(Value::as_array) {
        Some(vars) => vars
            .iter()
            .filter_map(|var| {
                let key = var.get("key").and_then(Value::as_str)?;
                let raw_value = var.get("value");
                let value = raw_value
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| raw_value.map(|v| v.to_string()))
                    .unwrap_or_default();
                Some(crate::model::InPlaceVariable {
                    name: key.to_string(),
                    value,
                    line: 0,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    Ok(RequestFile {
        default_env: None,
        variables,
        requests,
    })
}

fn walk_items(items: &[Value], prefix: Option<&str>, out: &mut Vec<RequestBlock>) {
    for item in items {
        let raw_name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Unnamed");
        let full_name = match prefix {
            Some(p) => format!("{p} / {raw_name}"),
            None => raw_name.to_string(),
        };
        if let Some(nested) = item.get("item").and_then(Value::as_array) {
            walk_items(nested, Some(&full_name), out);
            continue;
        }
        if let Some(request) = item.get("request") {
            if let Some(block) = build_request_block(request, &full_name) {
                out.push(block);
            }
        }
    }
}

fn build_request_block(request: &Value, name: &str) -> Option<RequestBlock> {
    let (method, is_graphql) = extract_method(request);
    let url = extract_url(request)?;
    let headers = extract_headers(request);
    let body = extract_body(request, is_graphql);
    Some(RequestBlock {
        name: Some(name.to_string()),
        method,
        url,
        headers,
        body,
        options: RequestOptions::default(),
        assertions: Vec::new(),
        captures: Vec::new(),
        response_redirect: None,
        range: SourceRange {
            start_line: 0,
            end_line: 0,
        },
    })
}

fn extract_method(request: &Value) -> (RequestMethod, bool) {
    let body_mode = request
        .pointer("/body/mode")
        .and_then(Value::as_str)
        .unwrap_or("");
    if body_mode == "graphql" {
        return (RequestMethod::GraphQl, true);
    }
    let method_text = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let method = RequestMethod::parse(method_text).unwrap_or(RequestMethod::Get);
    (method, false)
}

fn extract_url(request: &Value) -> Option<String> {
    if let Some(url) = request.get("url") {
        if let Some(s) = url.as_str() {
            return Some(s.to_string());
        }
        if let Some(raw) = url.get("raw").and_then(Value::as_str) {
            return Some(raw.to_string());
        }
        if let Some(host) = url.get("host").and_then(Value::as_array) {
            let host_text = host
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(".");
            let path = url
                .get("path")
                .and_then(Value::as_array)
                .map(|segments| {
                    segments
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            let mut composed = host_text;
            if !path.is_empty() {
                composed.push('/');
                composed.push_str(&path);
            }
            if !composed.is_empty() {
                return Some(composed);
            }
        }
    }
    None
}

fn extract_headers(request: &Value) -> Vec<Header> {
    let Some(array) = request.get("header").and_then(Value::as_array) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|header| {
            let disabled = header
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if disabled {
                return None;
            }
            let key = header.get("key").and_then(Value::as_str)?;
            let value = header.get("value").and_then(Value::as_str).unwrap_or("");
            Some(Header {
                name: key.to_string(),
                value: value.to_string(),
                line: 0,
            })
        })
        .collect()
}

fn extract_body(request: &Value, is_graphql: bool) -> Option<RequestBody> {
    let body = request.get("body")?;
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or("");

    if is_graphql || mode == "graphql" {
        let graphql = body.get("graphql")?;
        let query = graphql
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let variables = graphql
            .get("variables")
            .and_then(Value::as_str)
            .unwrap_or("");
        let variables_trimmed = variables.trim();
        let combined = if variables_trimmed.is_empty() {
            query
        } else {
            format!("{query}\n\n{variables_trimmed}")
        };
        if combined.is_empty() {
            return None;
        }
        return Some(RequestBody::Inline(combined));
    }

    match mode {
        "raw" => body
            .get("raw")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| RequestBody::Inline(s.to_string())),
        "" => body
            .get("raw")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| RequestBody::Inline(s.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_a_simple_collection() {
        let input = r#"{
            "info": { "name": "Demo", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
            "variable": [{ "key": "host", "value": "https://api.example.com" }],
            "item": [
                {
                    "name": "Health",
                    "request": {
                        "method": "GET",
                        "header": [{ "key": "Accept", "value": "application/json" }],
                        "url": { "raw": "{{host}}/health" }
                    }
                },
                {
                    "name": "Users",
                    "item": [
                        {
                            "name": "Create",
                            "request": {
                                "method": "POST",
                                "header": [{ "key": "Content-Type", "value": "application/json" }],
                                "url": { "raw": "{{host}}/api/users" },
                                "body": {
                                    "mode": "raw",
                                    "raw": "{\"name\": \"Alice\"}"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;

        let file = import_collection(input).unwrap();
        assert_eq!(file.requests.len(), 2);
        assert_eq!(file.requests[0].name.as_deref(), Some("Health"));
        assert_eq!(file.requests[1].name.as_deref(), Some("Users / Create"));
        assert_eq!(file.requests[1].method, RequestMethod::Post);
        assert_eq!(file.variables.len(), 1);
        assert_eq!(file.variables[0].name, "host");
    }

    #[test]
    fn imports_a_graphql_request() {
        let input = r#"{
            "info": { "name": "GQL", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
            "item": [{
                "name": "GetUser",
                "request": {
                    "method": "POST",
                    "header": [],
                    "url": { "raw": "{{host}}/graphql" },
                    "body": {
                        "mode": "graphql",
                        "graphql": {
                            "query": "query GetUser($id: ID!) { user(id: $id) { id name } }",
                            "variables": "{\"id\": \"42\"}"
                        }
                    }
                }
            }]
        }"#;

        let file = import_collection(input).unwrap();
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].method, RequestMethod::GraphQl);
        let body = file.requests[0]
            .body
            .as_ref()
            .and_then(|b| match b {
                RequestBody::Inline(text) => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert!(body.contains("query GetUser"));
        assert!(body.contains("\"id\": \"42\""));
    }
}
