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
    let base = worktree_root
        .map(Path::to_path_buf)
        .or_else(|| http_file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".zed-http").join("responses")
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
