//! `http-client.env.json` discovery, layering, and secret masking.
//!
//! Mirrors JetBrains' lookup behaviour: walk up from the `.http` file's
//! directory toward the worktree root (or filesystem root) looking for
//! `http-client.env.json` (public, checked-in) and
//! `http-client.private.env.json` (gitignored, holds secrets). Then layer
//! private over public so a private file can override a public placeholder
//! without leaking real values into git.
//!
//! `mask_value` / `mask_variables` are only used by the CLI's `--verbose`
//! dump; they don't touch the wire request. The list of "sensitive"
//! substrings (`token`, `secret`, `password`, `apikey`, `api_key`,
//! `authorization`) is intentionally small — over-masking would hide useful
//! debug detail.

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

/// Does this variable or header name look like it holds a secret?
///
/// Substring match on a lower-cased name, so `X-API-Key` and `refresh_token`
/// are both caught. The dashed spellings are listed explicitly: a dash defeats
/// both the `apikey` and `api_key` needles.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "pwd",
        "apikey",
        "api_key",
        "api-key",
        "authorization",
        "credential",
        "bearer",
        "session",
        "jwt",
        "private_key",
        "private-key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn mask_value(key: &str, value: &str) -> String {
    if is_sensitive_key(key) && !value.is_empty() {
        "***".to_string()
    } else {
        value.to_string()
    }
}

/// Replace the *values* of secret-looking variables wherever they appear in
/// `text`.
///
/// Masking the variables table alone is theatre: a secret exists to be
/// interpolated, so it reappears in clear in the URL, a header, or the body.
/// This redacts the value itself, so `--verbose` output is safe to paste into
/// a bug report.
pub fn redact_secrets(text: &str, values: &VariableMap) -> String {
    // Longest first: if one secret is a substring of another, redacting the
    // short one first would leave a fragment of the long one behind.
    let mut secrets: Vec<&str> = values
        .iter()
        .filter(|(key, value)| is_sensitive_key(key) && !value.is_empty())
        .map(|(_, value)| value.as_str())
        .collect();
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));

    let mut redacted = text.to_string();
    for secret in secrets {
        redacted = redacted.replace(secret, "***");
    }
    redacted
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
    // Both sides are canonicalised before comparison: `--worktree .` or a path
    // reached through a symlink is not a textual ancestor of the request file,
    // and a lexical comparison would walk straight past the boundary to `/`.
    let start_dir = canonical_dir(http_file.parent().unwrap_or_else(|| Path::new(".")));
    let stop_at = worktree_root.map(canonical_dir);

    for dir in start_dir.ancestors() {
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        if stop_at.as_deref() == Some(dir) {
            break;
        }
        // A worktree that isn't an ancestor of the request file confines the
        // search to the directories at or below it, rather than leaking upward.
        if stop_at.as_ref().is_some_and(|stop| !dir.starts_with(stop)) {
            break;
        }
    }

    Ok(None)
}

/// Resolve `path` to an absolute, symlink-free directory. `std::path::absolute`
/// would do for the fallback but postdates the 1.74 MSRV.
fn canonical_dir(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn masks_the_dashed_api_key_spellings() {
        // The dash defeats both the `apikey` and `api_key` needles, so these
        // very common header spellings leaked in full.
        assert_eq!(mask_value("api-key", "hunter2"), "***");
        assert_eq!(mask_value("X-API-Key", "hunter2"), "***");
    }

    #[test]
    fn masks_the_other_common_secret_spellings() {
        for key in [
            "passwd",
            "credential",
            "session",
            "jwt",
            "private-key",
            "refresh_token",
        ] {
            assert_eq!(mask_value(key, "hunter2"), "***", "{key} should be masked");
        }
    }

    #[test]
    fn leaves_a_non_secret_key_alone() {
        assert_eq!(mask_value("host", "example.com"), "example.com");
        assert_eq!(mask_value("user_id", "42"), "42");
    }

    #[test]
    fn an_empty_secret_is_not_masked_into_stars() {
        // Masking "" to *** would imply a value exists where none does.
        assert_eq!(mask_value("token", ""), "");
    }

    #[test]
    fn redacts_a_secret_value_wherever_it_appears_in_text() {
        // The whole point of #13: the variables table said `token = ***`, then
        // the interpolated header printed the secret in clear.
        let mut vars = VariableMap::new();
        vars.insert("token".to_string(), "s3cr3t".to_string());
        vars.insert("host".to_string(), "example.com".to_string());

        let redacted = redact_secrets("Authorization: Bearer s3cr3t", &vars);

        assert_eq!(redacted, "Authorization: Bearer ***");
    }

    #[test]
    fn redacting_leaves_non_secret_values_intact() {
        let mut vars = VariableMap::new();
        vars.insert("host".to_string(), "example.com".to_string());

        assert_eq!(
            redact_secrets("https://example.com/api", &vars),
            "https://example.com/api"
        );
    }

    #[test]
    fn redacts_every_occurrence_and_every_secret() {
        let mut vars = VariableMap::new();
        vars.insert("token".to_string(), "aaa".to_string());
        vars.insert("password".to_string(), "bbb".to_string());

        let redacted = redact_secrets("aaa bbb aaa", &vars);

        assert_eq!(redacted, "*** *** ***");
    }

    #[test]
    fn redacting_ignores_an_empty_secret_value() {
        // An empty value would otherwise match at every character boundary and
        // shatter the text into `***` between every char.
        let mut vars = VariableMap::new();
        vars.insert("token".to_string(), String::new().to_string());

        assert_eq!(redact_secrets("nothing to hide", &vars), "nothing to hide");
    }

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

    #[test]
    fn returns_empty_map_when_no_env_name_requested() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();

        let values = load_environment(&request_file, Some(&dir), None).unwrap();

        assert!(values.is_empty());
    }

    #[test]
    fn returns_empty_map_when_no_env_files_exist() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();

        let values = load_environment(&request_file, Some(&dir), Some("dev")).unwrap();

        assert!(values.is_empty());
    }

    #[test]
    fn malformed_env_file_is_reported_as_an_error() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();
        fs::write(dir.join("http-client.env.json"), "{ not valid json").unwrap();

        let result = load_environment(&request_file, Some(&dir), Some("dev"));

        assert!(matches!(result, Err(HttpClientError::Json(_))));
    }

    #[test]
    fn unknown_env_name_yields_no_values() {
        let dir = temp_dir();
        let request_file = dir.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();
        fs::write(
            dir.join("http-client.env.json"),
            r#"{ "dev": { "host": "https://dev.example.com" } }"#,
        )
        .unwrap();

        let values = load_environment(&request_file, Some(&dir), Some("prod")).unwrap();

        assert!(values.is_empty());
    }

    #[test]
    fn finds_an_env_file_in_an_ancestor_directory_inside_the_worktree() {
        let root = temp_dir();
        let nested = root.join("api").join("v1");
        fs::create_dir_all(&nested).unwrap();
        let request_file = nested.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();
        fs::write(
            root.join("http-client.env.json"),
            r#"{ "dev": { "host": "https://root.example.com" } }"#,
        )
        .unwrap();

        let values = load_environment(&request_file, Some(&root), Some("dev")).unwrap();

        assert_eq!(
            values.get("host").map(String::as_str),
            Some("https://root.example.com")
        );
    }

    #[test]
    fn does_not_escape_a_worktree_expressed_non_canonically() {
        // `outside` sits above the worktree root and must never be consulted.
        let outside = temp_dir();
        let worktree = outside.join("project");
        let nested = worktree.join("api");
        fs::create_dir_all(&nested).unwrap();
        let request_file = nested.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();
        fs::write(
            outside.join("http-client.env.json"),
            r#"{ "dev": { "host": "https://leaked.example.com" } }"#,
        )
        .unwrap();

        // `<worktree>/api/..` is the same directory as `<worktree>`, but it is
        // not a textual ancestor of the request file's directory.
        let non_canonical = nested.join("..");
        let values = load_environment(&request_file, Some(&non_canonical), Some("dev")).unwrap();

        assert!(
            values.is_empty(),
            "env discovery escaped the worktree and found {values:?}"
        );
    }

    #[test]
    fn does_not_escape_when_the_worktree_is_not_an_ancestor_at_all() {
        let outside = temp_dir();
        let worktree = temp_dir();
        let nested = outside.join("api");
        fs::create_dir_all(&nested).unwrap();
        let request_file = nested.join("requests.http");
        fs::write(&request_file, "GET https://example.com\n").unwrap();
        fs::write(
            outside.join("http-client.env.json"),
            r#"{ "dev": { "host": "https://leaked.example.com" } }"#,
        )
        .unwrap();

        let values = load_environment(&request_file, Some(&worktree), Some("dev")).unwrap();

        assert!(
            values.is_empty(),
            "env discovery ascended out of an unrelated worktree and found {values:?}"
        );
    }
}
