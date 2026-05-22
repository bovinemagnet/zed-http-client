//! `zed-http` — the CLI half of the zed-http-client project.
//!
//! Thin wrapper around [`zed_http_core`]: clap dispatches a subcommand, the
//! handler asks core to parse / resolve / validate, and then this binary
//! does the I/O — `reqwest` for HTTP, `cookie_store` for the persistent
//! jar, plus filesystem writes for response persistence, schema caching,
//! and `>>` / `>>!` redirect output.
//!
//! Subcommands:
//!
//! - `run`        — execute a single request, with optional pre-flight
//!   validation and per-request `# @timeout` / `# @no-redirect` /
//!   `# @fragments` / `# @expect-*` directives honoured.
//! - `run-all`    — execute every request in a file in order, with a
//!   per-request status line and a pass/fail summary. Shares one cookie
//!   jar across iterations so login → action flows work in CI.
//! - `check`      — validate every request in a file without sending.
//! - `list`       — enumerate requests with line numbers (drives Zed's
//!   "select a request" pickers).
//! - `envs`       — list environment names defined across the public and
//!   private env files.
//! - `format`     — re-emit a file in canonical layout.
//! - `introspect` — send the standard GraphQL introspection query against
//!   a selected GRAPHQL request and cache the schema.
//! - `schema`     — inspect / list cached schemas.
//! - `import`     — translate a Postman v2.1 collection (`import postman`)
//!   or a `curl` command (`import curl`) into a canonical `.http` block.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use cookie_store::CookieStore;
use reqwest::{header::CONTENT_TYPE, redirect::Policy, Method};
use reqwest_cookie_store::CookieStoreMutex;
use serde_json::json;
use zed_http_core::{
    build_preview, cookie_jar_path, decode_har_input, evaluate_assertions, evaluate_captures,
    format_pretty_response, format_request_file, import_curl, import_har,
    import_postman_collection, introspection_payload, list_environments, load_cached_schema,
    mask_variables, parse_request_file, prepare_request, prepare_request_with_extras,
    response_root, save_response, schema_root, schema_slug, validate_request_file_with_schemas,
    validate_request_with_schema, AssertionResponse, CaptureWarning, RequestMethod,
    RequestSelector, ResolvedRequest, ResponseSummary, ValidationIssue,
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
        #[arg(long)]
        no_cookies: bool,
        #[arg(long)]
        cookie_jar: Option<PathBuf>,
        /// Override variables: `--var name=value`, repeatable. Wins over
        /// every other layer (env files, in-file @vars, dynamic vars).
        #[arg(long = "var", value_name = "NAME=VALUE")]
        var: Vec<String>,
    },
    Check {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        worktree: Option<PathBuf>,
    },
    RunAll {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        worktree: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputMode::Pretty)]
        output: OutputMode,
        #[arg(long)]
        bail: bool,
        #[arg(long)]
        no_validate: bool,
        #[arg(long)]
        no_cookies: bool,
        #[arg(long)]
        cookie_jar: Option<PathBuf>,
        /// Seed variables: `--var name=value`, repeatable. Captures from
        /// earlier requests in this run-all invocation accumulate on top.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        var: Vec<String>,
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
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    /// Generate shell completion scripts for bash/zsh/fish/powershell/elvish.
    Completions {
        /// Target shell.
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum ImportCommand {
    Postman {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Har {
        /// Path to the HAR 1.2 archive (Chrome/Firefox/Safari "Save all
        /// as HAR with content" output).
        #[arg(long)]
        file: PathBuf,
        /// Write the resulting .http content here. Defaults to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Prepend each imported request's name with this prefix, e.g.
        /// `--name-prefix "Smoke"` produces `Smoke / 1: GET /api/users`.
        #[arg(long)]
        name_prefix: Option<String>,
    },
    Curl {
        /// Inline curl command, e.g. `'curl https://example.com'`.
        #[arg(value_name = "CURL_COMMAND")]
        command: Option<String>,
        /// Read the curl command from a file.
        #[arg(long, conflicts_with_all = ["command", "stdin"])]
        file: Option<PathBuf>,
        /// Read the curl command from stdin.
        #[arg(long, conflicts_with_all = ["command", "file"])]
        stdin: bool,
        /// Write the resulting .http content here. Defaults to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Override the imported request's name.
        #[arg(long)]
        name: Option<String>,
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

/// Response assertions (`# @expect-*`) failed, or a `run-all` request
/// returned a non-2xx status. Surfaced as process exit code 2 so CI can
/// tell "the requests ran but failed their checks" from a generic error.
#[derive(Debug)]
struct TestFailure(String);

impl std::fmt::Display for TestFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TestFailure {}

/// Pre-flight validation rejected the request file before anything was
/// sent. Surfaced as process exit code 3.
#[derive(Debug)]
struct ValidationFailure(String);

impl std::fmt::Display for ValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValidationFailure {}

/// Map a top-level failure to a process exit code: 2 for test failures,
/// 3 for validation failures, 1 for everything else.
fn exit_code_for(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<TestFailure>().is_some() {
        2
    } else if err.downcast_ref::<ValidationFailure>().is_some() {
        3
    } else {
        1
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = dispatch(cli.command).await {
        eprintln!("Error: {err:#}");
        std::process::exit(exit_code_for(&err));
    }
}

async fn dispatch(command: Commands) -> Result<()> {
    match command {
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
            no_cookies,
            cookie_jar,
            var,
        } => {
            let var_overrides = parse_var_overrides(&var)?;
            run_command(RunOptions {
                file: &file,
                line,
                name: name.as_deref(),
                env: env.as_deref(),
                worktree: worktree.as_deref(),
                output_mode: output,
                verbose,
                no_validate,
                no_cookies,
                cookie_jar: cookie_jar.as_deref(),
                var_overrides: &var_overrides,
            })
            .await
        }
        Commands::Check {
            file,
            env,
            worktree,
        } => check_command(&file, env.as_deref(), worktree.as_deref()),
        Commands::RunAll {
            file,
            env,
            worktree,
            output,
            bail,
            no_validate,
            no_cookies,
            cookie_jar,
            var,
        } => {
            let var_overrides = parse_var_overrides(&var)?;
            run_all_command(RunAllOptions {
                file: &file,
                env: env.as_deref(),
                worktree: worktree.as_deref(),
                output_mode: output,
                bail,
                no_validate,
                no_cookies,
                cookie_jar: cookie_jar.as_deref(),
                var_overrides: &var_overrides,
            })
            .await
        }
        Commands::Schema { command } => schema_command(command),
        Commands::Import { command } => import_command(command),
        Commands::Completions { shell } => completions_command(shell),
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

    let issues = validate_whole_file(file, &contents, &parsed, env, worktree);

    if issues.is_empty() {
        println!(
            "{}: {} request(s) validated, no issues",
            file.display(),
            parsed.requests.len()
        );
        return Ok(());
    }
    print_issues(file, &issues);
    Err(ValidationFailure(format!("{} validation issue(s) found", issues.len())).into())
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

fn import_command(command: ImportCommand) -> Result<()> {
    match command {
        ImportCommand::Postman { file, out } => {
            let raw = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let request_file = import_postman_collection(&raw)
                .with_context(|| format!("failed to import {}", file.display()))?;
            let rendered = format_request_file(&request_file);
            emit_import_output(
                out.as_deref(),
                &rendered,
                request_file.requests.len(),
                &file.display().to_string(),
            )
        }
        ImportCommand::Har {
            file,
            out,
            name_prefix,
        } => {
            let bytes =
                fs::read(&file).with_context(|| format!("failed to read {}", file.display()))?;
            let raw = decode_har_input(&bytes)
                .with_context(|| format!("failed to decode {}", file.display()))?;
            let request_file = import_har(&raw, name_prefix.as_deref())
                .with_context(|| format!("failed to import {}", file.display()))?;
            let rendered = format_request_file(&request_file);
            emit_import_output(
                out.as_deref(),
                &rendered,
                request_file.requests.len(),
                &file.display().to_string(),
            )
        }
        ImportCommand::Curl {
            command,
            file,
            stdin,
            out,
            name,
        } => {
            let (raw, source_label) = match (command, file, stdin) {
                (Some(cmd), None, false) => (cmd, "<inline>".to_string()),
                (None, Some(path), false) => {
                    let text = fs::read_to_string(&path)
                        .with_context(|| format!("failed to read {}", path.display()))?;
                    (text, path.display().to_string())
                }
                (None, None, true) => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .context("failed to read curl command from stdin")?;
                    (buf, "<stdin>".to_string())
                }
                (None, None, false) => {
                    anyhow::bail!(
                        "provide a curl command as a positional argument, \
                         --file <path>, or --stdin"
                    );
                }
                _ => unreachable!("clap conflicts_with rules out the rest"),
            };
            let request_file = import_curl(&raw, name.as_deref())
                .with_context(|| format!("failed to import curl command from {source_label}"))?;
            let rendered = format_request_file(&request_file);
            emit_import_output(
                out.as_deref(),
                &rendered,
                request_file.requests.len(),
                &source_label,
            )
        }
    }
}

fn completions_command(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(())
}

fn emit_import_output(
    out: Option<&Path>,
    rendered: &str,
    request_count: usize,
    source_label: &str,
) -> Result<()> {
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create output directory {}", parent.display())
                    })?;
                }
            }
            fs::write(path, rendered)
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!(
                "Imported {request_count} request(s) from {source_label} → {}",
                path.display()
            );
        }
        None => print!("{rendered}"),
    }
    Ok(())
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

/// Validate every request in a parsed file, schema-aware where a cached
/// schema can be found. Each URL is resolved first so `load_cached_schema`
/// has something to match on; resolution degrades silently when env
/// interpolation can't produce a URL (same rule for `check` and `run-all`).
fn validate_whole_file(
    file: &Path,
    contents: &str,
    parsed: &zed_http_core::RequestFile,
    env: Option<&str>,
    worktree: Option<&Path>,
) -> Vec<ValidationIssue> {
    let mut schemas: Vec<Option<serde_json::Value>> = Vec::with_capacity(parsed.requests.len());
    for request in &parsed.requests {
        let resolved_url = prepare_request(
            file,
            contents,
            RequestSelector::Line(request.range.start_line),
            env,
            worktree,
        )
        .ok()
        .map(|resolved| resolved.url)
        .or_else(|| {
            if !request.url.contains("{{") {
                Some(request.url.clone())
            } else {
                None
            }
        });
        schemas.push(resolved_url.and_then(|url| load_cached_schema(file, worktree, &url)));
    }

    validate_request_file_with_schemas(parsed, |idx, _| {
        schemas.get(idx).and_then(|slot| slot.clone())
    })
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
    no_cookies: bool,
    cookie_jar: Option<&'a Path>,
    var_overrides: &'a VariableMap,
}

type VariableMap = indexmap::IndexMap<String, String>;

fn mask_capture(name: &str, value: &str) -> String {
    // Reuse the env-file masking heuristic so a captured `token` is shown
    // as `***` in both terminal output and the JSON envelope. Wire-side
    // requests still use the unmasked value.
    let lower = name.to_ascii_lowercase();
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

fn parse_var_overrides(pairs: &[String]) -> Result<VariableMap> {
    let mut map = VariableMap::new();
    for raw in pairs {
        let (name, value) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--var must be NAME=VALUE; got '{raw}'"))?;
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("--var name was empty in '{raw}'");
        }
        map.insert(name.to_string(), value.to_string());
    }
    Ok(map)
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
        no_cookies,
        cookie_jar,
        var_overrides,
    } = opts;
    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let selector = match (name, line) {
        (Some(name), _) => RequestSelector::Name(name),
        (None, Some(line)) => RequestSelector::Line(line),
        (None, None) => RequestSelector::First,
    };

    let resolved = prepare_request_with_extras(
        file,
        &contents,
        selector,
        env,
        worktree,
        Some(var_overrides),
    )
    .with_context(|| format!("failed to prepare request from {}", file.display()))?;

    if !no_validate {
        let parsed = parse_request_file(&contents)
            .with_context(|| format!("failed to parse {}", file.display()))?;
        let selected = select_block_for_validation(&parsed, name, line)?;
        let schema = load_cached_schema(file, worktree, &resolved.url);
        let issues = validate_request_with_schema(selected, schema.as_ref());
        if !issues.is_empty() {
            print_issues(file, &issues);
            return Err(ValidationFailure(
                "request failed validation; re-run with --no-validate to skip these checks"
                    .to_string(),
            )
            .into());
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

    let cookie_jar_target = resolve_cookie_jar_target(file, worktree, cookie_jar, no_cookies);
    let cookie_store = cookie_jar_target.as_ref().map(|path| load_cookie_jar(path));

    let client = build_client(&resolved, cookie_store.clone())?;
    let outcome = execute_resolved_request(&client, file, worktree, &resolved).await?;

    match output_mode {
        OutputMode::Pretty => {
            let summary = ResponseSummary {
                status: outcome.status_line.clone(),
                duration_ms: outcome.duration_ms,
                content_type: outcome.content_type.clone(),
                saved_path: outcome.saved_path.clone(),
                preview: outcome.body_preview.clone(),
            };
            print!("{}", format_pretty_response(&resolved, &summary));
            if let Some(path) = &outcome.redirect_path {
                println!("Response redirect:\n{}", path.display());
            }
            if !outcome.captured.is_empty() {
                println!("Captured:");
                for (key, value) in &outcome.captured {
                    println!("  {key} = {}", mask_capture(key, value));
                }
            }
            for warning in &outcome.capture_warnings {
                eprintln!(
                    "  {}:{}: capture {} skipped: {}",
                    file.display(),
                    warning.line,
                    warning.variable,
                    warning.message
                );
            }
            if !outcome.assertion_failures.is_empty() {
                eprintln!("\nAssertion failures:");
                for failure in &outcome.assertion_failures {
                    eprintln!("  {}:{}: {}", file.display(), failure.line, failure.message);
                }
            }
        }
        OutputMode::Raw => print!("{}", outcome.body_text),
        OutputMode::Json => {
            let body_value = serde_json::from_str::<serde_json::Value>(&outcome.body_text)
                .unwrap_or_else(|_| json!(outcome.body_text));
            let payload = json!({
                "request": {
                    "name": resolved.name,
                    "method": resolved.method.as_str(),
                    "url": resolved.url,
                    "line": resolved.range_start_line,
                },
                "response": {
                    "status": outcome.status_line,
                    "duration_ms": outcome.duration_ms,
                    "content_type": outcome.content_type,
                    "saved_path": outcome.saved_path,
                    "redirect_path": outcome.redirect_path,
                    "body": body_value,
                    "assertion_failures": outcome.assertion_failures
                        .iter()
                        .map(|f| json!({ "line": f.line, "message": f.message }))
                        .collect::<Vec<_>>(),
                    "captured": outcome.captured.iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::String(mask_capture(k, v))))
                        .collect::<serde_json::Map<_, _>>(),
                    "capture_warnings": outcome.capture_warnings
                        .iter()
                        .map(|w| json!({
                            "variable": w.variable,
                            "line": w.line,
                            "message": w.message,
                        }))
                        .collect::<Vec<_>>(),
                }
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    if let (Some(path), Some(store)) = (cookie_jar_target.as_ref(), cookie_store.as_ref()) {
        save_cookie_jar(path, store)
            .with_context(|| format!("failed to persist cookie jar to {}", path.display()))?;
    }

    if !outcome.assertion_failures.is_empty() {
        return Err(TestFailure(format!(
            "{} response assertion(s) failed",
            outcome.assertion_failures.len()
        ))
        .into());
    }

    Ok(())
}

struct RequestOutcome {
    status_code: u16,
    status_line: String,
    duration_ms: u128,
    content_type: Option<String>,
    saved_path: PathBuf,
    redirect_path: Option<PathBuf>,
    body_text: String,
    body_preview: String,
    assertion_failures: Vec<zed_http_core::AssertionFailure>,
    captured: VariableMap,
    capture_warnings: Vec<CaptureWarning>,
}

struct RunAllOptions<'a> {
    file: &'a Path,
    env: Option<&'a str>,
    worktree: Option<&'a Path>,
    output_mode: OutputMode,
    bail: bool,
    no_validate: bool,
    no_cookies: bool,
    cookie_jar: Option<&'a Path>,
    var_overrides: &'a VariableMap,
}

async fn run_all_command(opts: RunAllOptions<'_>) -> Result<()> {
    let RunAllOptions {
        file,
        env,
        worktree,
        output_mode,
        bail,
        no_validate,
        no_cookies,
        cookie_jar,
        var_overrides,
    } = opts;

    let contents =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    let parsed = parse_request_file(&contents)
        .with_context(|| format!("failed to parse {}", file.display()))?;

    if parsed.requests.is_empty() {
        anyhow::bail!("{} contains no requests", file.display());
    }

    if !no_validate {
        // Whole-file validation up front. Schema-aware checks need a
        // resolvable URL, so they degrade silently when env interpolation
        // can't produce one (same rules as `check`).
        let issues = validate_whole_file(file, &contents, &parsed, env, worktree);
        if !issues.is_empty() {
            print_issues(file, &issues);
            return Err(ValidationFailure(
                "validation failed; re-run with --no-validate to skip these checks".to_string(),
            )
            .into());
        }
    }

    let cookie_jar_target = resolve_cookie_jar_target(file, worktree, cookie_jar, no_cookies);
    let cookie_store = cookie_jar_target.as_ref().map(|path| load_cookie_jar(path));

    let total = parsed.requests.len();
    let mut results: Vec<RunAllEntry> = Vec::with_capacity(total);
    let mut bailed_at: Option<usize> = None;
    // Accumulated extras layer: CLI --var seeds it, captures from earlier
    // requests overlay on top.
    let mut extras: VariableMap = var_overrides.clone();

    let pretty = matches!(output_mode, OutputMode::Pretty);
    if pretty {
        println!("{}", file.display());
    }

    for (idx, request) in parsed.requests.iter().enumerate() {
        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("Unnamed request at line {}", request.range.start_line));

        let resolved = match prepare_request_with_extras(
            file,
            &contents,
            RequestSelector::Line(request.range.start_line),
            env,
            worktree,
            Some(&extras),
        ) {
            Ok(resolved) => resolved,
            Err(err) => {
                let message = format!("failed to prepare request: {err}");
                if pretty {
                    println!("  ✗ {:<30} {}", name, message);
                }
                results.push(RunAllEntry {
                    index: idx + 1,
                    name,
                    line: request.range.start_line,
                    method: request.method.as_str().to_string(),
                    url: request.url.clone(),
                    ok: false,
                    status_code: None,
                    status_line: None,
                    duration_ms: None,
                    error: Some(message),
                    assertion_failures: Vec::new(),
                    captured: Vec::new(),
                    capture_warnings: Vec::new(),
                });
                if bail {
                    bailed_at = Some(idx + 1);
                    break;
                }
                continue;
            }
        };

        let client = build_client(&resolved, cookie_store.clone())?;
        let outcome = match execute_resolved_request(&client, file, worktree, &resolved).await {
            Ok(outcome) => outcome,
            Err(err) => {
                let message = format!("request failed: {err}");
                if pretty {
                    println!("  ✗ {:<30} {}", name, message);
                }
                results.push(RunAllEntry {
                    index: idx + 1,
                    name,
                    line: request.range.start_line,
                    method: resolved.method.as_str().to_string(),
                    url: resolved.url.clone(),
                    ok: false,
                    status_code: None,
                    status_line: None,
                    duration_ms: None,
                    error: Some(message),
                    assertion_failures: Vec::new(),
                    captured: Vec::new(),
                    capture_warnings: Vec::new(),
                });
                if bail {
                    bailed_at = Some(idx + 1);
                    break;
                }
                continue;
            }
        };

        // Merge captures from this request into the running extras layer
        // so a later request can reference them via {{name}}.
        for (key, value) in &outcome.captured {
            extras.insert(key.clone(), value.clone());
        }
        let captured_for_entry: Vec<(String, String)> = outcome
            .captured
            .iter()
            .map(|(k, v)| (k.clone(), mask_capture(k, v)))
            .collect();
        let capture_warnings_for_entry = outcome.capture_warnings.clone();

        let ok = outcome.status_code < 400 && outcome.assertion_failures.is_empty();
        if pretty {
            let marker = if ok { "✓" } else { "✗" };
            println!(
                "  {marker} {:<30} {:<24} {:>5}ms",
                name, outcome.status_line, outcome.duration_ms
            );
            for failure in &outcome.assertion_failures {
                println!(
                    "      {}:{}: {}",
                    file.display(),
                    failure.line,
                    failure.message
                );
            }
            for (key, value) in &captured_for_entry {
                println!("      captured {key} = {value}");
            }
            for warning in &capture_warnings_for_entry {
                println!(
                    "      capture {} skipped: {}",
                    warning.variable, warning.message
                );
            }
        }
        let entry = RunAllEntry {
            index: idx + 1,
            name,
            line: request.range.start_line,
            method: resolved.method.as_str().to_string(),
            url: resolved.url.clone(),
            ok,
            status_code: Some(outcome.status_code),
            status_line: Some(outcome.status_line.clone()),
            captured: captured_for_entry,
            capture_warnings: capture_warnings_for_entry,
            duration_ms: Some(outcome.duration_ms),
            error: None,
            assertion_failures: outcome.assertion_failures.clone(),
        };
        let entry_ok = entry.ok;
        results.push(entry);
        if bail && !entry_ok {
            bailed_at = Some(idx + 1);
            break;
        }
    }

    if let (Some(path), Some(store)) = (cookie_jar_target.as_ref(), cookie_store.as_ref()) {
        save_cookie_jar(path, store)
            .with_context(|| format!("failed to persist cookie jar to {}", path.display()))?;
    }

    let passed = results.iter().filter(|r| r.ok).count();
    let failed = results.iter().filter(|r| !r.ok).count();
    let skipped = total - results.len();

    match output_mode {
        OutputMode::Pretty => {
            println!();
            print!("{} requests: {} passed, {} failed", total, passed, failed);
            if skipped > 0 {
                print!(", {} skipped", skipped);
            }
            println!();
            if let Some(at) = bailed_at {
                println!("bailed at request #{at}");
            }
        }
        OutputMode::Raw => {
            for entry in &results {
                println!(
                    "{}\t{}\t{}\t{}",
                    if entry.ok { "PASS" } else { "FAIL" },
                    entry
                        .status_line
                        .clone()
                        .unwrap_or_else(|| "error".to_string()),
                    entry.name,
                    entry.url
                );
            }
        }
        OutputMode::Json => {
            let payload = json!({
                "file": file.display().to_string(),
                "summary": {
                    "total": total,
                    "passed": passed,
                    "failed": failed,
                    "skipped": skipped,
                    "bailed_at": bailed_at,
                },
                "requests": results.iter().map(|entry| json!({
                    "index": entry.index,
                    "name": entry.name,
                    "line": entry.line,
                    "method": entry.method,
                    "url": entry.url,
                    "ok": entry.ok,
                    "status_code": entry.status_code,
                    "status": entry.status_line,
                    "duration_ms": entry.duration_ms,
                    "error": entry.error,
                    "assertion_failures": entry.assertion_failures.iter()
                        .map(|f| json!({ "line": f.line, "message": f.message }))
                        .collect::<Vec<_>>(),
                    "captured": entry.captured.iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                        .collect::<serde_json::Map<_, _>>(),
                    "capture_warnings": entry.capture_warnings.iter()
                        .map(|w| json!({
                            "variable": w.variable,
                            "line": w.line,
                            "message": w.message,
                        }))
                        .collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    if failed > 0 || bailed_at.is_some() {
        return Err(TestFailure(format!("{failed} request(s) failed")).into());
    }

    Ok(())
}

struct RunAllEntry {
    index: usize,
    name: String,
    line: usize,
    method: String,
    url: String,
    ok: bool,
    status_code: Option<u16>,
    status_line: Option<String>,
    duration_ms: Option<u128>,
    error: Option<String>,
    assertion_failures: Vec<zed_http_core::AssertionFailure>,
    captured: Vec<(String, String)>,
    capture_warnings: Vec<CaptureWarning>,
}

async fn execute_resolved_request(
    client: &reqwest::Client,
    file: &Path,
    worktree: Option<&Path>,
    resolved: &ResolvedRequest,
) -> Result<RequestOutcome> {
    let method = Method::from_bytes(resolved.http_method.as_bytes())?;
    let mut request = client.request(method, &resolved.url);
    for (header_name, header_value) in &resolved.headers {
        request = request.header(header_name, header_value);
    }
    if let Some(body) = &resolved.body {
        request = request.body(body.clone());
    }

    let started = Instant::now();
    let response = request.send().await?;
    let duration = started.elapsed();
    let status = response.status();
    let header_pairs: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), v.to_string()))
        })
        .collect();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body_bytes = response.bytes().await?;
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();

    let response_view = AssertionResponse {
        status: status.as_u16(),
        headers: &header_pairs,
        body: &body_text,
    };
    let assertion_failures = evaluate_assertions(&resolved.assertions, &response_view);
    let capture_outcome = evaluate_captures(&resolved.captures, &response_view);
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
    let body_preview = build_preview(content_type.as_deref(), &body_text);
    let status_line = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .trim()
    .to_string();

    Ok(RequestOutcome {
        status_code: status.as_u16(),
        status_line,
        duration_ms: duration.as_millis(),
        content_type,
        saved_path,
        redirect_path,
        body_text,
        body_preview,
        assertion_failures,
        captured: capture_outcome.captured,
        capture_warnings: capture_outcome.warnings,
    })
}

fn resolve_cookie_jar_target(
    file: &Path,
    worktree: Option<&Path>,
    cookie_jar: Option<&Path>,
    no_cookies: bool,
) -> Option<PathBuf> {
    if no_cookies {
        None
    } else {
        Some(
            cookie_jar
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cookie_jar_path(file, worktree)),
        )
    }
}

fn load_cookie_jar(path: &Path) -> Arc<CookieStoreMutex> {
    let store = if path.exists() {
        match fs::File::open(path) {
            Ok(file) => {
                let reader = std::io::BufReader::new(file);
                cookie_store::serde::json::load(reader).unwrap_or_default()
            }
            Err(_) => CookieStore::default(),
        }
    } else {
        CookieStore::default()
    };
    Arc::new(CookieStoreMutex::new(store))
}

fn save_cookie_jar(path: &Path, store: &CookieStoreMutex) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create cookie jar directory {}", parent.display())
            })?;
        }
    }
    let mut file = fs::File::create(path)?;
    let guard = store
        .lock()
        .map_err(|err| anyhow::anyhow!("cookie store mutex poisoned: {err}"))?;
    cookie_store::serde::json::save_incl_expired_and_nonpersistent(&guard, &mut file)
        .map_err(|err| anyhow::anyhow!("failed to serialise cookie jar: {err}"))?;
    Ok(())
}

fn build_client(
    resolved: &ResolvedRequest,
    cookie_jar: Option<Arc<CookieStoreMutex>>,
) -> Result<reqwest::Client> {
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
    if let Some(jar) = cookie_jar {
        builder = builder.cookie_provider(jar);
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
