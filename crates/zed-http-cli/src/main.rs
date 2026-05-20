use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::{header::CONTENT_TYPE, Method};
use serde_json::json;
use zed_http_core::{
    build_preview, format_pretty_response, list_environments, mask_variables, parse_request_file,
    prepare_request, response_root, save_response, ResponseSummary,
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
        #[arg(long)]
        line: Option<usize>,
        #[arg(long)]
        column: Option<usize>,
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
            env,
            worktree,
            output,
            verbose,
        } => {
            run_command(
                &file,
                line,
                env.as_deref(),
                worktree.as_deref(),
                output,
                verbose,
            )
            .await
        }
        Commands::List { file } => list_command(&file),
        Commands::Envs { file, worktree } => envs_command(&file, worktree.as_deref()),
    }
}

fn list_command(file: &Path) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let request_file = parse_request_file(&contents)?;

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

fn envs_command(file: &Path, worktree: Option<&Path>) -> Result<()> {
    for env_name in list_environments(file, worktree)? {
        println!("{env_name}");
    }
    Ok(())
}

async fn run_command(
    file: &Path,
    line: Option<usize>,
    env: Option<&str>,
    worktree: Option<&Path>,
    output_mode: OutputMode,
    verbose: bool,
) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let resolved = prepare_request(file, &contents, line, env, worktree)?;

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

    let client = reqwest::Client::new();
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
        OutputMode::Pretty => print!("{}", format_pretty_response(&resolved, &summary)),
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
                    "body": body_value,
                }
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}
