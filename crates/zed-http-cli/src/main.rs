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
    list_environments, load_cached_schema, mask_variables, parse_request_file, prepare_request,
    response_root, save_response, schema_root, schema_slug, validate_request_file_with_schemas,
    validate_request_with_schema, RequestMethod, RequestSelector, ResolvedRequest, ResponseSummary,
    ValidationIssue,
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
        #[arg(long)]
        no_validate: bool,
    },
    Check {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        worktree: Option<PathBuf>,
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
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SchemaCommand {
    List {
        #[arg(long)]
        worktree: Option<PathBuf>,
    },
    Show {
        #[arg(long)]
        host: String,
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
            name,
            env,
            worktree,
            output,
            verbose,
            no_validate,
        } => {
            run_command(RunOptions {
                file: &file,
                line,
                name: name.as_deref(),
                env: env.as_deref(),
                worktree: worktree.as_deref(),
                output_mode: output,
                verbose,
                no_validate,
            })
            .await
        }
        Commands::Check {
            file,
            env,
            worktree,
        } => check_command(&file, env.as_deref(), worktree.as_deref()),
        Commands::Schema { command } => schema_command(command),
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

fn check_command(file: &Path, env: Option<&str>, worktree: Option<&Path>) -> Result<()> {
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let parsed = parse_request_file(&contents)
        .with_context(|| format!("failed to parse {}", file.display()))?;

    let mut schemas: Vec<Option<serde_json::Value>> = Vec::with_capacity(parsed.requests.len());
    for request in &parsed.requests {
        let resolved_url = if env.is_some() {
            prepare_request(
                file,
                &contents,
                RequestSelector::Line(request.range.start_line),
                env,
                worktree,
            )
            .ok()
            .map(|resolved| resolved.url)
        } else if !request.url.contains("{{") {
            Some(request.url.clone())
        } else {
            None
        };
        schemas.push(resolved_url.and_then(|url| load_cached_schema(file, worktree, &url)));
    }

    let issues = validate_request_file_with_schemas(&parsed, |idx, _| {
        schemas.get(idx).and_then(|slot| slot.clone())
    });

    if issues.is_empty() {
        println!(
            "{}: {} request(s) validated, no issues",
            file.display(),
            parsed.requests.len()
        );
        return Ok(());
    }
    print_issues(file, &issues);
    anyhow::bail!("{} validation issue(s) found", issues.len());
}

fn schema_command(command: SchemaCommand) -> Result<()> {
    match command {
        SchemaCommand::List { worktree } => {
            let dir = schema_dir(worktree.as_deref());
            if !dir.exists() {
                println!("No cached schemas at {}", dir.display());
                return Ok(());
            }
            let mut entries: Vec<_> = fs::read_dir(&dir)
                .with_context(|| format!("failed to read {}", dir.display()))?
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .map(|ext| ext == "json")
                        .unwrap_or(false)
                })
                .collect();
            entries.sort_by_key(|entry| entry.file_name());
            if entries.is_empty() {
                println!("No cached schemas at {}", dir.display());
                return Ok(());
            }
            for entry in entries {
                let metadata = entry.metadata().ok();
                let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                println!(
                    "{}\t{} bytes",
                    entry
                        .path()
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    size
                );
            }
            Ok(())
        }
        SchemaCommand::Show { host, worktree } => {
            let dir = schema_dir(worktree.as_deref());
            let path = dir.join(format!("{host}.json"));
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read cached schema {}", path.display()))?;
            let value: serde_json::Value =
                serde_json::from_str(&raw).context("cached schema is not valid JSON")?;
            print_schema_summary(&host, &value);
            Ok(())
        }
    }
}

fn schema_dir(worktree: Option<&Path>) -> PathBuf {
    let base = worktree
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".zed-http").join("schema")
}

fn print_schema_summary(host: &str, schema: &serde_json::Value) {
    println!("Schema for {host}");
    let query = schema
        .pointer("/__schema/queryType/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<none>");
    let mutation = schema
        .pointer("/__schema/mutationType/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<none>");
    let subscription = schema
        .pointer("/__schema/subscriptionType/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<none>");
    println!("  Query root:        {query}");
    println!("  Mutation root:     {mutation}");
    println!("  Subscription root: {subscription}");

    let types = schema
        .pointer("/__schema/types")
        .and_then(serde_json::Value::as_array);
    if let Some(types) = types {
        println!("  Types:             {}", types.len());
        if query != "<none>" {
            print_root_field_count(types, query, "Query fields");
        }
        if mutation != "<none>" {
            print_root_field_count(types, mutation, "Mutation fields");
        }
    }
}

fn print_root_field_count(types: &[serde_json::Value], root_name: &str, label: &str) {
    let count = types
        .iter()
        .find(|ty| ty.get("name").and_then(serde_json::Value::as_str) == Some(root_name))
        .and_then(|ty| ty.get("fields").and_then(serde_json::Value::as_array))
        .map(|fields| fields.len())
        .unwrap_or(0);
    println!("  {label:<18} {count}");
}

fn print_issues(file: &Path, issues: &[ValidationIssue]) {
    for issue in issues {
        let label = issue.request_name.as_deref().unwrap_or("(unnamed)");
        eprintln!(
            "{}:{}: [{}] {}",
            file.display(),
            issue.line,
            label,
            issue.message
        );
    }
}

fn select_block_for_validation<'a>(
    file: &'a zed_http_core::RequestFile,
    name: Option<&str>,
    line: Option<usize>,
) -> Result<&'a zed_http_core::RequestBlock> {
    match (name, line) {
        (Some(name), _) => zed_http_core::select_request_by_name(file, name)
            .ok_or_else(|| anyhow::anyhow!("no request named '{name}' found for validation")),
        (None, Some(line)) => zed_http_core::select_request_by_line(file, line)
            .ok_or_else(|| anyhow::anyhow!("no request found at line {line} for validation")),
        (None, None) => file
            .requests
            .first()
            .ok_or_else(|| anyhow::anyhow!("no requests found in file for validation")),
    }
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
            let slug = schema_slug(&resolved.url).unwrap_or_else(|| {
                resolved
                    .name
                    .clone()
                    .unwrap_or_else(|| "schema".to_string())
                    .to_ascii_lowercase()
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-")
            });
            dir.join(format!("{slug}.json"))
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

fn envs_command(file: &Path, worktree: Option<&Path>) -> Result<()> {
    for env_name in list_environments(file, worktree)? {
        println!("{env_name}");
    }
    Ok(())
}

struct RunOptions<'a> {
    file: &'a Path,
    line: Option<usize>,
    name: Option<&'a str>,
    env: Option<&'a str>,
    worktree: Option<&'a Path>,
    output_mode: OutputMode,
    verbose: bool,
    no_validate: bool,
}

async fn run_command(opts: RunOptions<'_>) -> Result<()> {
    let RunOptions {
        file,
        line,
        name,
        env,
        worktree,
        output_mode,
        verbose,
        no_validate,
    } = opts;
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let selector = match (name, line) {
        (Some(name), _) => RequestSelector::Name(name),
        (None, Some(line)) => RequestSelector::Line(line),
        (None, None) => RequestSelector::First,
    };

    let resolved = prepare_request(file, &contents, selector, env, worktree)
        .with_context(|| format!("failed to prepare request from {}", file.display()))?;

    if !no_validate {
        let parsed = parse_request_file(&contents)
            .with_context(|| format!("failed to parse {}", file.display()))?;
        let selected = select_block_for_validation(&parsed, name, line)?;
        let schema = load_cached_schema(file, worktree, &resolved.url);
        let issues = validate_request_with_schema(selected, schema.as_ref());
        if !issues.is_empty() {
            print_issues(file, &issues);
            anyhow::bail!(
                "request failed validation; re-run with --no-validate to skip these checks"
            );
        }
    }

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
