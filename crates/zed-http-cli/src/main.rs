use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::{header::CONTENT_TYPE, redirect::Policy, Method};
use serde_json::json;
use zed_http_core::{
    build_preview, format_pretty_response, format_request_file, introspection_payload,
    list_environments, mask_variables, parse_request_file, prepare_request, response_root,
    save_response, schema_root, RequestMethod, RequestSelector, ResolvedRequest, ResponseSummary,
};

#[derive(Debug, Parser)]
#[command(
    name = "zed-http",
    version,
    about = "Run HTTP and GraphQL request files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Run {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, conflicts_with = "name")]
        line: Option<usize>,
        #[arg(long)]
        column: Option<usize>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        worktree: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputMode::Pretty)]
        output: OutputMode,
        #[arg(long)]
        verbose: bool,
    },
    List {
        #[arg(long)]
        file: PathBuf,
    },
    Envs {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        worktree: Option<PathBuf>,
    },
    Format {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, conflicts_with = "check")]
        in_place: bool,
        #[arg(long)]
        check: bool,
    },
    Introspect {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, conflicts_with = "name")]
        line: Option<usize>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        worktree: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum OutputMode {
    Pretty,
    Json,
    Raw,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            file,
            line,
            column: _,
            name,
            env,
            worktree,
            output,
            verbose,
        } => {
            run_command(
                &file,
                line,
                name.as_deref(),
                env.as_deref(),
                worktree.as_deref(),
                output,
                verbose,
            )
            .await
        }
        Commands::List { file } => list_command(&file),
        Commands::Envs { file, worktree } => envs_command(&file, worktree.as_deref()),
        Commands::Format {
            file,
            in_place,
            check,
        } => format_command(&file, in_place, check),
        Commands::Introspect {
            file,
            line,
            name,
            env,
            worktree,
            output,
        } => {
            introspect_command(
                &file,
                line,
                name.as_deref(),
                env.as_deref(),
                worktree.as_deref(),
                output.as_deref(),
            )
            .await
        }
    }
}

fn list_command(file: &Path) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let request_file = parse_request_file(&contents)
        .with_context(|| format!("failed to parse {}", file.display()))?;

    for (index, request) in request_file.requests.iter().enumerate() {
        let name = request.name.as_deref().unwrap_or("Unnamed request");
        println!(
            "{}. {:<18} {:<8} line {}",
            index + 1,
            name,
            request.method,
            request.range.start_line
        );
    }

    Ok(())
}

fn format_command(file: &Path, in_place: bool, check: bool) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let request_file = parse_request_file(&contents)
        .with_context(|| format!("failed to parse {}", file.display()))?;
    let formatted = format_request_file(&request_file);

    if check {
        if contents != formatted {
            anyhow::bail!(
                "{} is not in canonical form; run `zed-http format --in-place` to fix",
                file.display()
            );
        }
        return Ok(());
    }

    if in_place {
        fs::write(file, &formatted)
            .with_context(|| format!("failed to write formatted file {}", file.display()))?;
    } else {
        print!("{formatted}");
    }
    Ok(())
}

async fn introspect_command(
    file: &Path,
    line: Option<usize>,
    name: Option<&str>,
    env: Option<&str>,
    worktree: Option<&Path>,
    output: Option<&Path>,
) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let selector = match (name, line) {
        (Some(name), _) => RequestSelector::Name(name),
        (None, Some(line)) => RequestSelector::Line(line),
        (None, None) => RequestSelector::First,
    };
    let resolved = prepare_request(file, &contents, selector, env, worktree)
        .with_context(|| format!("failed to prepare request from {}", file.display()))?;

    if resolved.method != RequestMethod::GraphQl {
        anyhow::bail!(
            "selected request is {}, not GRAPHQL; introspect needs a GRAPHQL request",
            resolved.method
        );
    }

    let client = reqwest::Client::new();
    let mut request = client.post(&resolved.url);
    for (header_name, header_value) in &resolved.headers {
        if header_name.eq_ignore_ascii_case("content-type")
            || header_name.eq_ignore_ascii_case("accept")
        {
            continue;
        }
        request = request.header(header_name, header_value);
    }
    request = request
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(introspection_payload());

    let response = request.send().await?;
    let status = response.status();
    let body_bytes = response.bytes().await?;

    if !status.is_success() {
        let body_text = String::from_utf8_lossy(&body_bytes);
        anyhow::bail!(
            "introspection request returned {}: {}",
            status,
            body_text.trim()
        );
    }

    let body_value: serde_json::Value =
        serde_json::from_slice(&body_bytes).context("introspection response was not valid JSON")?;
    if let Some(errors) = body_value.get("errors") {
        anyhow::bail!("introspection request returned GraphQL errors: {errors}");
    }
    let schema_value = body_value
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("introspection response missing 'data' field"))?;
    let pretty = serde_json::to_string_pretty(&schema_value)?;

    let target = match output {
        Some(path) => path.to_path_buf(),
        None => {
            let dir = schema_root(file, worktree);
            fs::create_dir_all(&dir).with_context(|| {
                format!("failed to create schema cache directory {}", dir.display())
            })?;
            dir.join(format!("{}.json", schema_slug(&resolved)))
        }
    };

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).ok();
        }
    }
    fs::write(&target, &pretty)
        .with_context(|| format!("failed to write schema cache {}", target.display()))?;

    println!("Schema cached:\n{}", target.display());
    Ok(())
}

fn schema_slug(resolved: &ResolvedRequest) -> String {
    if let Ok(url) = reqwest::Url::parse(&resolved.url) {
        if let Some(host) = url.host_str() {
            return host.replace([':', '/'], "-");
        }
    }
    resolved
        .name
        .as_deref()
        .unwrap_or("schema")
        .to_ascii_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-")
}

fn envs_command(file: &Path, worktree: Option<&Path>) -> Result<()> {
    for env_name in list_environments(file, worktree)? {
        println!("{env_name}");
    }
    Ok(())
}

async fn run_command(
    file: &Path,
    line: Option<usize>,
    name: Option<&str>,
    env: Option<&str>,
    worktree: Option<&Path>,
    output_mode: OutputMode,
    verbose: bool,
) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let selector = match (name, line) {
        (Some(name), _) => RequestSelector::Name(name),
        (None, Some(line)) => RequestSelector::Line(line),
        (None, None) => RequestSelector::First,
    };
    let resolved = prepare_request(file, &contents, selector, env, worktree)
        .with_context(|| format!("failed to prepare request from {}", file.display()))?;

    if verbose {
        let masked_variables = mask_variables(&resolved.variables);
        println!("Resolved request:");
        println!("  Method: {}", resolved.http_method);
        println!("  URL: {}", resolved.url);
        if !masked_variables.is_empty() {
            println!("  Variables:");
            for (key, value) in masked_variables {
                println!("    {key} = {value}");
            }
        }
        if !resolved.headers.is_empty() {
            println!("  Headers:");
            for (key, value) in &resolved.headers {
                println!("    {key}: {value}");
            }
        }
        if let Some(body) = &resolved.body {
            println!(
                "  Body:
{body}"
            );
        }
        println!();
    }

    let client = build_client(&resolved)?;
    let method = Method::from_bytes(resolved.http_method.as_bytes())?;
    let mut request = client.request(method, &resolved.url);
    for (name, value) in &resolved.headers {
        request = request.header(name, value);
    }
    if let Some(body) = &resolved.body {
        request = request.body(body.clone());
    }

    let started = Instant::now();
    let response = request.send().await?;
    let duration = started.elapsed();
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body_bytes = response.bytes().await?;
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();
    let save_dir = response_root(file, worktree);
    let saved_path = save_response(
        &save_dir,
        resolved.name.as_deref(),
        &resolved.method,
        content_type.as_deref(),
        &body_text,
    )?;
    let redirect_path = match resolved.response_redirect.as_ref() {
        Some(redirect) => Some(write_response_redirect(file, redirect, &body_text)?),
        None => None,
    };
    let preview = build_preview(content_type.as_deref(), &body_text);
    let summary = ResponseSummary {
        status: format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )
        .trim()
        .to_string(),
        duration_ms: duration.as_millis(),
        content_type,
        saved_path,
        preview,
    };

    match output_mode {
        OutputMode::Pretty => {
            print!("{}", format_pretty_response(&resolved, &summary));
            if let Some(path) = &redirect_path {
                println!("Response redirect:\n{}", path.display());
            }
        }
        OutputMode::Raw => print!("{}", body_text),
        OutputMode::Json => {
            let body_value = serde_json::from_str::<serde_json::Value>(&body_text)
                .unwrap_or_else(|_| json!(body_text));
            let payload = json!({
                "request": {
                    "name": resolved.name,
                    "method": resolved.method.as_str(),
                    "url": resolved.url,
                    "line": resolved.range_start_line,
                },
                "response": {
                    "status": summary.status,
                    "duration_ms": summary.duration_ms,
                    "content_type": summary.content_type,
                    "saved_path": summary.saved_path,
                    "redirect_path": redirect_path,
                    "body": body_value,
                }
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn build_client(resolved: &ResolvedRequest) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if let Some(ms) = resolved.options.timeout_ms {
        builder = builder.timeout(Duration::from_millis(ms));
    }
    if let Some(ms) = resolved.options.connection_timeout_ms {
        builder = builder.connect_timeout(Duration::from_millis(ms));
    }
    if resolved.options.no_redirect {
        builder = builder.redirect(Policy::none());
    }
    builder
        .build()
        .context("failed to build HTTP client with request options")
}

fn write_response_redirect(
    http_file: &Path,
    redirect: &zed_http_core::ResponseRedirect,
    body: &str,
) -> Result<PathBuf> {
    let base_dir = http_file.parent().unwrap_or_else(|| Path::new("."));
    let target = base_dir.join(&redirect.path);
    if target.exists() && !redirect.force_overwrite {
        anyhow::bail!(
            "response redirect target {} already exists; use >>! to force overwrite",
            target.display()
        );
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create response redirect directory {}",
                    parent.display()
                )
            })?;
        }
    }
    fs::write(&target, body)
        .with_context(|| format!("failed to write response redirect to {}", target.display()))?;
    Ok(target)
}
