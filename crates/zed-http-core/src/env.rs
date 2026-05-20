use std::{
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde_json::Value;

use crate::error::HttpClientError;

pub type VariableMap = IndexMap<String, String>;

pub fn load_environment(
    http_file: &Path,
    worktree_root: Option<&Path>,
    env_name: Option<&str>,
) -> Result<VariableMap, HttpClientError> {
    let Some(env_name) = env_name else {
        return Ok(IndexMap::new());
    };

    let public = discover_env_file(http_file, worktree_root, "http-client.env.json")?;
    let private = discover_env_file(http_file, worktree_root, "http-client.private.env.json")?;

    let mut values = IndexMap::new();
    if let Some(path) = public {
        overlay_env_values(&mut values, &path, env_name)?;
    }
    if let Some(path) = private {
        overlay_env_values(&mut values, &path, env_name)?;
    }

    Ok(values)
}

pub fn list_environments(
    http_file: &Path,
    worktree_root: Option<&Path>,
) -> Result<Vec<String>, HttpClientError> {
    let public = discover_env_file(http_file, worktree_root, "http-client.env.json")?;
    let private = discover_env_file(http_file, worktree_root, "http-client.private.env.json")?;

    let mut envs = Vec::new();
    for path in [public, private].into_iter().flatten() {
        let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        if let Some(object) = value.as_object() {
            for key in object.keys() {
                if !envs.iter().any(|env| env == key) {
                    envs.push(key.clone());
                }
            }
        }
    }

    Ok(envs)
}

pub fn mask_value(key: &str, value: &str) -> String {
    let lower = key.to_ascii_lowercase();
    let sensitive = [
        "token",
        "secret",
        "password",
        "apikey",
        "api_key",
        "authorization",
    ];
    if sensitive.iter().any(|needle| lower.contains(needle)) && !value.is_empty() {
        "***".to_string()
    } else {
        value.to_string()
    }
}

pub fn mask_variables(values: &VariableMap) -> VariableMap {
    values
        .iter()
        .map(|(key, value)| (key.clone(), mask_value(key, value)))
        .collect()
}

fn overlay_env_values(
    values: &mut VariableMap,
    path: &Path,
    env_name: &str,
) -> Result<(), HttpClientError> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    if let Some(env_object) = value.get(env_name).and_then(Value::as_object) {
        for (key, value) in env_object {
            let string_value = match value {
                Value::String(value) => value.clone(),
                other => other.to_string(),
            };
            values.insert(key.clone(), string_value);
        }
    }
    Ok(())
}

fn discover_env_file(
    http_file: &Path,
    worktree_root: Option<&Path>,
    file_name: &str,
) -> Result<Option<PathBuf>, HttpClientError> {
    let start_dir = http_file.parent().unwrap_or_else(|| Path::new("."));
    let stop_at = worktree_root.map(Path::to_path_buf);

    for dir in start_dir.ancestors() {
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        if stop_at.as_deref() == Some(dir) {
            break;
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zed-http-client-env-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn overlays_private_values_over_public_values() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            "GET https://example.com
",
        )
        .unwrap();
        fs::write(
            dir.join("http-client.env.json"),
            r#"{
  "dev": {
    "host": "https://public.example.com",
    "token": "public-token"
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

        let values = load_environment(&request_file, Some(&dir), Some("dev")).unwrap();

        assert_eq!(
            values.get("host").map(String::as_str),
            Some("https://public.example.com")
        );
        assert_eq!(
            values.get("token").map(String::as_str),
            Some("private-token")
        );
    }

    #[test]
    fn lists_available_environment_names() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(
            &request_file,
            "GET https://example.com
",
        )
        .unwrap();
        fs::write(
            dir.join("http-client.env.json"),
            r#"{ "dev": {}, "prod": {} }"#,
        )
        .unwrap();
        fs::write(
            dir.join("http-client.private.env.json"),
            r#"{ "test": {} }"#,
        )
        .unwrap();

        let envs = list_environments(&request_file, Some(&dir)).unwrap();

        assert_eq!(envs, vec!["dev", "prod", "test"]);
    }

    #[test]
    fn masks_secret_values() {
        assert_eq!(mask_value("token", "super-secret"), "***");
        assert_eq!(
            mask_value("host", "https://example.com"),
            "https://example.com"
        );
    }
}
