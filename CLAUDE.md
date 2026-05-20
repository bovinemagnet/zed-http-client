# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`zed-http-client` is a Zed editor extension plus a companion Rust CLI for running IntelliJ-style `.http`/`.rest` request files (including a `GRAPHQL` pseudo-method). It is a Cargo workspace with two crates, and ships extension assets (language config, Tree-sitter grammar, runnables) alongside the Rust code.

## Common commands

Build and run the CLI (binary is named `zed-http`, but it lives in the `zed-http-cli` package):

```bash
cargo build -p zed-http-cli
cargo run -p zed-http-cli -- --help
cargo run -p zed-http-cli -- run --file examples/requests.http --line 4 --env dev --worktree .
cargo run -p zed-http-cli -- list --file examples/requests.http
cargo run -p zed-http-cli -- envs --file examples/requests.http
cargo run -p zed-http-cli -- format --file examples/requests.http --check
cargo run -p zed-http-cli -- check --file examples/requests.http
cargo run -p zed-http-cli -- check --file examples/requests.http --env dev
cargo run -p zed-http-cli -- introspect --file examples/requests.http --name "GraphQL user query"
cargo run -p zed-http-cli -- schema list
cargo run -p zed-http-cli -- schema show --host countries.trevorblades.com
cargo run -p zed-http-cli -- import postman --file collection.json --out requests.http
```

Tests (most logic lives in `zed-http-core` as `#[cfg(test)]` modules next to each source file):

```bash
cargo test --workspace                       # all tests
cargo test -p zed-http-core                  # core crate only
cargo test -p zed-http-core parser::tests    # one module
cargo test -p zed-http-core parses_json_body # one test by name
```

Lint / format:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

When iterating on the Zed-side experience, the CLI is what Zed invokes via a task — see `tasks.json.example`, the project-local `.zed/tasks.json`, and the `$ZED_FILE` / `$ZED_ROW` / `$ZED_WORKTREE_ROOT` plumbing documented in `README.adoc` and `src/docs/modules/ROOT/pages/zed-integration.adoc`.

## Architecture

The CLI is intentionally a thin shell. Almost everything lives in `zed-http-core`, exposed through `lib.rs` re-exports. The pipeline for `zed-http run` is:

1. **Parse** (`parser.rs`) — splits the file on `###` markers into `RequestBlock`s, captures `@name = value` in-place variables, and records 1-based `SourceRange` for each block so `--line` can map cursor position back to a request. Section preambles can carry `# @timeout`, `# @connection-timeout`, `# @no-redirect` directives, populated into `RequestOptions`. Bodies are an enum: `RequestBody::Inline(String)` or `RequestBody::FromFile { path }` when the body is a single `< ./path` line. Trailing `>> path` / `>>! path` lines (last `>>` wins) become a `ResponseRedirect`. Unknown `# @directive` names are silently ignored for forward compatibility.
2. **Select** (`executor::RequestSelector` → `parser::select_request_by_{line,name}`) — picks the block by cursor line (`--line`), case-insensitive `### name` match (`--name`), or first request.
3. **Resolve variables** (`env.rs` + `interpolate.rs`) — `load_environment` walks from the request file's directory upward (stopping at `--worktree`) discovering `http-client.env.json` (public) and `http-client.private.env.json` (private). Private values overlay public, then in-file `@vars` overlay both. `resolve_variables` then expands `{{var}}` references *within* values (multi-pass), and `interpolate_text` substitutes into the URL, headers, body, and response-redirect path — failing on unknown names there but silently leaving unknowns inside variable definitions.
4. **Body materialisation** (`executor.rs`) — for `RequestBody::FromFile`, the path is interpolated then read relative to the `.http` file's directory before being passed through interpolation again. Inline bodies are interpolated directly.
5. **GraphQL transform** (`graphql.rs`) — for a `GRAPHQL` block, `render_graphql_json` splits the body on the last blank-line-separated `{ ... }` JSON suffix, treating it as `variables`, derives `operationName` from `query|mutation|subscription <Name>`, and emits the canonical `{query, variables, operationName}` payload. `Content-Type` and `Accept` are forced to `application/json`, but caller headers still override.
6. **Execute** — `main.rs::build_client` constructs the `reqwest::Client` honouring `RequestOptions` (`.timeout`, `.connect_timeout`, `.redirect(Policy::none())` when `@no-redirect`), sends the request, and feeds the response into `output.rs`.
7. **Output / persist** (`output.rs` + `main.rs::write_response_redirect`) — pretty / json / raw modes for stdout, plus an unconditional save to `<worktree-or-file-dir>/.zed-http/responses/<timestamp>-<slug>.<ext>` (extension derived from `Content-Type`). When the request has a `ResponseRedirect`, the body is additionally written to that path (relative to the `.http` file dir); `>>` refuses to overwrite existing files and `>>!` forces it.

Key types live in `model.rs`: `RequestFile`, `RequestBlock`, `RequestMethod` (note `GraphQl` is a *parser-level* method whose `http_method()` returns `"POST"`), `RequestBody`, `RequestOptions`, `ResponseRedirect`, `Header`, `InPlaceVariable`, `SourceRange`. All errors are funnelled through `HttpClientError` in `error.rs` and the CLI wraps them with the request file path via `anyhow::Context` before they reach the user.

Secret masking (`env::mask_value`) is a pure presentation concern used only by `--verbose` output; it lower-cases the key and matches any of `token`, `secret`, `password`, `apikey`, `api_key`, `authorization`.

## Zed extension side

- `extension.toml` declares the extension and pins the Tree-sitter grammar by commit SHA (currently a placeholder — update when bumping the grammar).
- `grammars/tree-sitter-http-request/grammar.js` is the in-repo grammar source mirrored to a separate published repo; the Zed extension fetches it from there, not from this directory.
- `languages/http-request/` holds Zed's view: `config.toml` (file suffixes `http`, `rest`), `highlights.scm`, `injections.scm`, and `runnables.scm`. The `runnables.scm` tag `http-client-request` is what Zed matches against the `tags` field in `tasks.json` — keep them in sync if renaming.

## Conventions

- Workspace edition is `2021`. Both crate versions are `0.1.0`. `Cargo.lock` is committed.
- Tests use `std::env::temp_dir().join(format!("...{nanos}"))` for isolation rather than the `tempfile` crate — follow that pattern when adding fixtures that touch the filesystem.
- Line numbers throughout the parser and selectors are **1-based** (matching `$ZED_ROW`); ranges are inclusive on both ends.
- The CLI's `--column` flag is accepted but currently unused — preserve it in the interface so Zed tasks don't break.
