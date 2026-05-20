use std::{fs, path::Path};

use indexmap::IndexMap;

use crate::{
    env::{load_environment, VariableMap},
    error::HttpClientError,
    graphql::render_graphql_json_with_extras,
    interpolate::{interpolate_text, resolve_variables},
    model::{
        RequestBlock, RequestBody, RequestFile, RequestMethod, RequestOptions, ResponseRedirect,
    },
    parser::{parse_request_file, select_request_by_line, select_request_by_name},
};

#[derive(Debug, Clone, Default)]
pub enum RequestSelector<'a> {
    #[default]
    First,
    Line(usize),
    Name(&'a str),
}

#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    pub name: Option<String>,
    pub method: RequestMethod,
    pub http_method: String,
    pub url: String,
    pub headers: IndexMap<String, String>,
    pub body: Option<String>,
    pub options: RequestOptions,
    pub response_redirect: Option<ResponseRedirect>,
    pub variables: VariableMap,
    pub range_start_line: usize,
}

pub fn parse_and_select_request(
    contents: &str,
    selector: RequestSelector<'_>,
) -> Result<(RequestFile, RequestBlock), HttpClientError> {
    let request_file = parse_request_file(contents)?;
    let request = match selector {
        RequestSelector::Line(line) => select_request_by_line(&request_file, line)
            .cloned()
            .ok_or(HttpClientError::NoRequestForLine(line))?,
        RequestSelector::Name(name) => select_request_by_name(&request_file, name)
            .cloned()
            .ok_or_else(|| HttpClientError::NoRequestForName(name.to_string()))?,
        RequestSelector::First => request_file
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
    selector: RequestSelector<'_>,
    env_name: Option<&str>,
    worktree_root: Option<&Path>,
) -> Result<ResolvedRequest, HttpClientError> {
    let (request_file, request) = parse_and_select_request(contents, selector)?;
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

    let base_dir = http_file.parent().unwrap_or_else(|| Path::new("."));
    let mut body = match request.body.as_ref() {
        Some(RequestBody::Inline(text)) => Some(interpolate_text(text, &variables)?),
        Some(RequestBody::FromFile { path }) => {
            let resolved_path = interpolate_text(path, &variables)?;
            let target = base_dir.join(&resolved_path);
            let contents = fs::read_to_string(&target).map_err(|err| {
                HttpClientError::Message(format!(
                    "failed to read request body file {}: {err}",
                    target.display()
                ))
            })?;
            Some(interpolate_text(&contents, &variables)?)
        }
        None => None,
    };

    if request.method == RequestMethod::GraphQl {
        let graphql_body = body.as_deref().ok_or_else(|| {
            HttpClientError::Message("GRAPHQL requests require a body".to_string())
        })?;
        let mut fragments = String::new();
        for path in &request.options.fragment_paths {
            let resolved_path = interpolate_text(path, &variables)?;
            let fragment_path = base_dir.join(&resolved_path);
            let contents = fs::read_to_string(&fragment_path).map_err(|err| {
                HttpClientError::Message(format!(
                    "failed to read fragment file {}: {err}",
                    fragment_path.display()
                ))
            })?;
            let interpolated = interpolate_text(&contents, &variables)?;
            if !fragments.is_empty() {
                fragments.push_str("\n\n");
            }
            fragments.push_str(interpolated.trim_end());
        }
        body = Some(render_graphql_json_with_extras(graphql_body, &fragments)?);
        let mut merged_headers = IndexMap::from([
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ]);
        for (key, value) in headers {
            merged_headers.insert(key, value);
        }
        headers = merged_headers;
    }

    let response_redirect = match request.response_redirect {
        Some(ResponseRedirect {
            path,
            force_overwrite,
        }) => Some(ResponseRedirect {
            path: interpolate_text(&path, &variables)?,
            force_overwrite,
        }),
        None => None,
    };

    Ok(ResolvedRequest {
        name: request.name,
        method: request.method.clone(),
        http_method: request.method.http_method().to_string(),
        url,
        headers,
        body,
        options: request.options,
        response_redirect,
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
    fn reads_body_from_referenced_file_and_interpolates_it() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            "### Create user\nPOST https://example.com/users\nContent-Type: application/json\n\n< ./payload.json\n",
        )
        .unwrap();
        fs::write(dir.join("payload.json"), "{\"name\": \"{{name}}\"}").unwrap();
        fs::write(
            dir.join("http-client.env.json"),
            r#"{ "dev": { "name": "Alice" } }"#,
        )
        .unwrap();

        let resolved = prepare_request(
            &request_file,
            &fs::read_to_string(&request_file).unwrap(),
            RequestSelector::First,
            Some("dev"),
            Some(&dir),
        )
        .unwrap();

        assert_eq!(resolved.body.as_deref(), Some("{\"name\": \"Alice\"}"));
    }

    #[test]
    fn carries_request_options_and_response_redirect() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            "### Configured\n# @timeout 1000\n# @no-redirect\nGET https://example.com\n\n>>! ./out/last.json\n",
        )
        .unwrap();

        let resolved = prepare_request(
            &request_file,
            &fs::read_to_string(&request_file).unwrap(),
            RequestSelector::First,
            None,
            Some(&dir),
        )
        .unwrap();

        assert_eq!(resolved.options.timeout_ms, Some(1000));
        assert!(resolved.options.no_redirect);
        let redirect = resolved.response_redirect.unwrap();
        assert_eq!(redirect.path, "./out/last.json");
        assert!(redirect.force_overwrite);
    }

    #[test]
    fn includes_fragments_from_referenced_file() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            "### Spread\n# @fragments ./fragments.graphql\nGRAPHQL https://example.com/graphql\n\nquery { user { ...UserFragment } }\n\n{}\n",
        )
        .unwrap();
        fs::write(
            dir.join("fragments.graphql"),
            "fragment UserFragment on User {\n  id\n  name\n}\n",
        )
        .unwrap();

        let resolved = prepare_request(
            &request_file,
            &fs::read_to_string(&request_file).unwrap(),
            RequestSelector::First,
            None,
            Some(&dir),
        )
        .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(resolved.body.as_deref().unwrap()).unwrap();
        let query = payload.get("query").and_then(|v| v.as_str()).unwrap();
        assert!(query.contains("query { user { ...UserFragment } }"));
        assert!(query.contains("fragment UserFragment on User"));
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
            RequestSelector::Line(4),
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
