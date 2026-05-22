//! Response persistence, pretty terminal output, and the conventional
//! `<base>/.zed-http/` artefact directory.
//!
//! Every `zed-http run` saves the full response body to
//! `<base>/.zed-http/responses/<timestamp>-<slug>.<ext>` so the user can
//! re-inspect later from the terminal-clickable path. The extension is
//! inferred from the response `Content-Type`; the slug comes from the
//! request name, falling back to the HTTP method.
//!
//! The same base directory hosts the schema cache
//! (`<base>/.zed-http/schema/<host>.json`) and the cookie jar
//! (`<base>/.zed-http/cookies.json`), kept together so a user can wipe
//! all extension state with one `rm -rf`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;

use crate::{error::HttpClientError, executor::ResolvedRequest, model::RequestMethod};

#[derive(Debug, Clone)]
pub struct ResponseSummary {
    pub status: String,
    pub duration_ms: u128,
    pub content_type: Option<String>,
    pub saved_path: PathBuf,
    pub preview: String,
}

pub fn response_root(http_file: &Path, worktree_root: Option<&Path>) -> PathBuf {
    base_artifact_dir(http_file, worktree_root).join("responses")
}

pub fn schema_root(http_file: &Path, worktree_root: Option<&Path>) -> PathBuf {
    base_artifact_dir(http_file, worktree_root).join("schema")
}

pub fn cookie_jar_path(http_file: &Path, worktree_root: Option<&Path>) -> PathBuf {
    base_artifact_dir(http_file, worktree_root).join("cookies.json")
}

fn base_artifact_dir(http_file: &Path, worktree_root: Option<&Path>) -> PathBuf {
    let base = worktree_root
        .map(Path::to_path_buf)
        .or_else(|| http_file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".zed-http")
}

pub fn save_response(
    base_dir: &Path,
    request_name: Option<&str>,
    method: &RequestMethod,
    content_type: Option<&str>,
    body: &str,
) -> Result<PathBuf, HttpClientError> {
    fs::create_dir_all(base_dir)?;
    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S");
    let slug = slugify(request_name.unwrap_or(method.as_str()));
    let extension = content_type_to_extension(content_type);
    let file_path = base_dir.join(format!("{timestamp}-{slug}{extension}"));
    fs::write(&file_path, body)?;
    Ok(file_path)
}

pub fn build_preview(content_type: Option<&str>, body: &str) -> String {
    if is_json(content_type, body) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            return serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string());
        }
    }

    let max_chars = 2000usize;
    let preview: String = body.chars().take(max_chars).collect();
    if body.chars().count() > max_chars {
        format!("{preview}\n…")
    } else {
        preview
    }
}

pub fn format_pretty_response(request: &ResolvedRequest, summary: &ResponseSummary) -> String {
    let title = request.name.as_deref().unwrap_or(request.method.as_str());
    let mut output = String::new();
    output.push_str(&format!("▶ {title}\n"));
    output.push_str(&format!(
        "{} {}\n\n",
        request.method.http_method(),
        request.url
    ));
    output.push_str(&format!("Status: {}\n", summary.status));
    output.push_str(&format!("Duration: {} ms\n", summary.duration_ms));
    output.push_str(&format!(
        "Content-Type: {}\n\n",
        summary.content_type.as_deref().unwrap_or("unknown")
    ));
    output.push_str("Response saved:\n");
    output.push_str(&format!("{}\n", summary.saved_path.display()));
    if !summary.preview.trim().is_empty() {
        output.push('\n');
        output.push_str(&summary.preview);
        output.push('\n');
    }
    output
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "response".to_string()
    } else {
        slug
    }
}

fn content_type_to_extension(content_type: Option<&str>) -> &'static str {
    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    match content_type {
        "application/json" => ".json",
        "application/graphql" => ".graphql",
        "text/html" => ".html",
        "text/plain" => ".txt",
        _ => ".body",
    }
}

fn is_json(content_type: Option<&str>, body: &str) -> bool {
    content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim() == "application/json")
        .unwrap_or(false)
        || serde_json::from_str::<serde_json::Value>(body).is_ok()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zed-http-client-output-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slugify_lowercases_and_collapses_non_alphanumerics() {
        assert_eq!(slugify("List Users"), "list-users");
        assert_eq!(slugify("GraphQL  user   query"), "graphql-user-query");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_separators() {
        assert_eq!(slugify("  ### Health check!! "), "health-check");
    }

    #[test]
    fn slugify_falls_back_to_response_when_empty() {
        assert_eq!(slugify(""), "response");
        assert_eq!(slugify("!!! ???"), "response");
    }

    #[test]
    fn content_type_extension_maps_known_types_and_ignores_charset() {
        assert_eq!(
            content_type_to_extension(Some("application/json; charset=utf-8")),
            ".json"
        );
        assert_eq!(content_type_to_extension(Some("text/html")), ".html");
        assert_eq!(content_type_to_extension(Some("text/plain")), ".txt");
        assert_eq!(
            content_type_to_extension(Some("application/graphql")),
            ".graphql"
        );
    }

    #[test]
    fn content_type_extension_defaults_to_body_for_unknown_or_missing() {
        assert_eq!(content_type_to_extension(Some("image/png")), ".body");
        assert_eq!(content_type_to_extension(None), ".body");
    }

    #[test]
    fn is_json_detects_by_content_type_and_by_sniffing_the_body() {
        assert!(is_json(Some("application/json"), "not json"));
        assert!(is_json(None, r#"{"ok":true}"#));
        assert!(!is_json(Some("text/plain"), "plain body"));
    }

    #[test]
    fn build_preview_pretty_prints_json_bodies() {
        let preview = build_preview(Some("application/json"), r#"{"b":2,"a":1}"#);
        assert!(preview.contains("\n  \"b\": 2"));
    }

    #[test]
    fn build_preview_truncates_long_non_json_bodies() {
        let body = "x".repeat(5000);
        let preview = build_preview(Some("text/plain"), &body);
        assert!(preview.ends_with('…'));
        assert_eq!(preview.chars().count(), 2002); // 2000 chars + newline + ellipsis
    }

    #[test]
    fn build_preview_leaves_short_bodies_untouched() {
        assert_eq!(build_preview(Some("text/plain"), "short"), "short");
    }

    #[test]
    fn save_response_writes_a_timestamped_slugged_file() {
        let dir = temp_dir().join("responses");
        let path = save_response(
            &dir,
            Some("List Users"),
            &RequestMethod::Get,
            Some("application/json; charset=utf-8"),
            r#"{"ok":true}"#,
        )
        .unwrap();

        assert!(path.exists());
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.ends_with("-list-users.json"), "got {name}");
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn save_response_falls_back_to_method_when_unnamed() {
        let dir = temp_dir().join("responses");
        let path = save_response(&dir, None, &RequestMethod::Delete, None, "").unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.ends_with("-delete.body"), "got {name}");
    }

    #[test]
    fn artifact_dirs_prefer_worktree_root_over_file_parent() {
        let http_file = Path::new("/project/api/requests.http");
        let worktree = Path::new("/project");

        assert_eq!(
            response_root(http_file, Some(worktree)),
            Path::new("/project/.zed-http/responses")
        );
        assert_eq!(
            schema_root(http_file, None),
            Path::new("/project/api/.zed-http/schema")
        );
        assert_eq!(
            cookie_jar_path(http_file, Some(worktree)),
            Path::new("/project/.zed-http/cookies.json")
        );
    }
}
