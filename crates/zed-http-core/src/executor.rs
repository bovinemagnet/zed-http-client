use std::path::Path;

use indexmap::IndexMap;

use crate::{
    env::{load_environment, VariableMap},
    error::HttpClientError,
    graphql::render_graphql_json,
    interpolate::{interpolate_text, resolve_variables},
    model::{RequestBlock, RequestFile, RequestMethod},
    parser::{parse_request_file, select_request_by_line},
};

#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    pub name: Option<String>,
    pub method: RequestMethod,
    pub http_method: String,
    pub url: String,
    pub headers: IndexMap<String, String>,
    pub body: Option<String>,
    pub variables: VariableMap,
    pub range_start_line: usize,
}

pub fn parse_and_select_request(
    contents: &str,
    line: Option<usize>,
) -> Result<(RequestFile, RequestBlock), HttpClientError> {
    let request_file = parse_request_file(contents)?;
    let request = match line {
        Some(line) => select_request_by_line(&request_file, line)
            .cloned()
            .ok_or(HttpClientError::NoRequestForLine(line))?,
        None => request_file
            .requests
            .first()
            .cloned()
            .ok_or(HttpClientError::NoRequestFound)?,
    };

    Ok((request_file, request))
}

pub fn prepare_request(
    http_file: &Path,
    contents: &str,
    line: Option<usize>,
    env_name: Option<&str>,
    worktree_root: Option<&Path>,
) -> Result<ResolvedRequest, HttpClientError> {
    let (request_file, request) = parse_and_select_request(contents, line)?;
    let mut variables = load_environment(http_file, worktree_root, env_name)?;
    for variable in request_file.variables {
        variables.insert(variable.name, variable.value);
    }
    let variables = resolve_variables(&variables);

    let url = interpolate_text(&request.url, &variables)?;
    let mut headers = IndexMap::new();
    for header in &request.headers {
        headers.insert(
            header.name.clone(),
            interpolate_text(&header.value, &variables)?,
        );
    }

    let mut body = match request.body.as_deref() {
        Some(body) => Some(interpolate_text(body, &variables)?),
        None => None,
    };

    if request.method == RequestMethod::GraphQl {
        let graphql_body = body.as_deref().ok_or_else(|| {
            HttpClientError::Message("GRAPHQL requests require a body".to_string())
        })?;
        body = Some(render_graphql_json(graphql_body)?);
        let mut merged_headers = IndexMap::from([
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ]);
        for (key, value) in headers {
            merged_headers.insert(key, value);
        }
        headers = merged_headers;
    }

    Ok(ResolvedRequest {
        name: request.name,
        method: request.method.clone(),
        http_method: request.method.http_method().to_string(),
        url,
        headers,
        body,
        variables,
        range_start_line: request.range.start_line,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;

    use super::*;

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zed-http-client-executor-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prepares_graphql_request_with_env_overlay_and_interpolation() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            r#"@path = users

### GraphQL request
GRAPHQL {{host}}/graphql
Authorization: Bearer {{token}}

query GetUser($id: ID!) {
  user(id: $id) {
    id
    name
  }
}

{
  "id": "{{userId}}"
}
"#,
        )
        .unwrap();
        fs::write(
            dir.join("http-client.env.json"),
            r#"{
  "dev": {
    "host": "https://public.example.com",
    "token": "public-token",
    "userId": "123"
  }
}"#,
        )
        .unwrap();
        fs::write(
            dir.join("http-client.private.env.json"),
            r#"{
  "dev": {
    "token": "private-token"
  }
}"#,
        )
        .unwrap();

        let resolved = prepare_request(
            &request_file,
            &fs::read_to_string(&request_file).unwrap(),
            Some(4),
            Some("dev"),
            Some(&dir),
        )
        .unwrap();

        assert_eq!(resolved.url, "https://public.example.com/graphql");
        assert_eq!(
            resolved.headers.get("Authorization").map(String::as_str),
            Some("Bearer private-token")
        );
        assert_eq!(
            resolved.headers.get("Content-Type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(resolved.body.as_deref().unwrap()).unwrap(),
            json!({
                "query": "query GetUser($id: ID!) {\n  user(id: $id) {\n    id\n    name\n  }\n}",
                "variables": {
                    "id": "123"
                },
                "operationName": "GetUser"
            })
        );
    }
}
