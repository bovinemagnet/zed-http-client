# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`zed-http-client` is a Zed editor extension plus a companion Rust CLI for running IntelliJ-style `.http`/`.rest` request files (including a `GRAPHQL` pseudo-method). It is a Cargo workspace with two crates under `crates/` (`zed-http-core`, `zed-http-cli`), a self-contained Zed extension under `extension/` (`extension.toml`, `languages/http-request/`, `snippets/`, `LICENSE`), and Antora docs (`src/docs/`). The Tree-sitter grammar is *not* vendored here — it is pinned by commit SHA in `extension/extension.toml` and cloned + compiled by Zed into `extension/grammars/` (gitignored) at install time.

## Common commands

Build and run the CLI (binary is named `zed-http`, but it lives in the `zed-http-cli` package):

```bash
cargo build -p zed-http-cli                  # crate lives at crates/zed-http-cli
cargo run -p zed-http-cli -- --help
cargo run -p zed-http-cli -- run --file examples/requests.http --line 4 --env dev --worktree .
cargo run -p zed-http-cli -- run --file examples/requests.http --name "Create user" --var token=abc123
cargo run -p zed-http-cli -- run-all --file examples/requests.http --env dev --bail
cargo run -p zed-http-cli -- list --file examples/requests.http
cargo run -p zed-http-cli -- envs --file examples/requests.http
cargo run -p zed-http-cli -- format --file examples/requests.http --check
cargo run -p zed-http-cli -- check --file examples/requests.http --env dev
cargo run -p zed-http-cli -- introspect --file examples/requests.http --name "GraphQL user query"
cargo run -p zed-http-cli -- schema list
cargo run -p zed-http-cli -- schema show --host countries.trevorblades.com
cargo run -p zed-http-cli -- import postman --file collection.json --out requests.http
cargo run -p zed-http-cli -- import har --file capture.har --out requests.http
cargo run -p zed-http-cli -- import curl 'curl https://example.com/api' --out requests.http
cargo run -p zed-http-cli -- completions zsh
```

The two send-and-run commands: `run` fires a single request (selected by `--line`, `--name`, or first); `run-all` fires every block in order, accumulating `# @capture` values so later requests can reference earlier responses. Both honour `--var name=value` (repeatable, wins over every other layer), `--no-validate` (skip pre-flight), `--no-cookies` / `--cookie-jar`, and `--output pretty|json|raw`.

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

The CLI (`crates/zed-http-cli/src/main.rs`) is intentionally a thin shell. Almost everything lives in `zed-http-core` (`crates/zed-http-core/src/`), exposed through `lib.rs` re-exports — `lib.rs` carries an authoritative module map in its doc comment, keep it in sync when adding modules. The pipeline for `zed-http run` is:

1. **Parse** (`parser.rs`) — splits the file on `###` markers into `RequestBlock`s, captures `@name = value` in-place variables, and records 1-based `SourceRange` for each block so `--line` can map cursor position back to a request. Section preambles can carry `# @timeout`, `# @connection-timeout`, `# @no-redirect` directives (→ `RequestOptions`), plus `# @expect-*` assertions (→ `ResponseAssertion`) and `# @capture` directives (→ `CaptureDirective`). Bodies are an enum: `RequestBody::Inline(String)` or `RequestBody::FromFile { path }` when the body is a single `< ./path` line. Trailing `>> path` / `>>! path` lines (last `>>` wins) become a `ResponseRedirect`. Unknown `# @directive` names are silently ignored for forward compatibility.
2. **Select** (`executor::RequestSelector` → `parser::select_request_by_{line,name}`) — picks the block by cursor line (`--line`), case-insensitive `### name` match (`--name`), or first request.
3. **Resolve variables** (`env.rs` + `dynamic.rs` + `interpolate.rs`) — `load_environment` walks from the request file's directory upward (stopping at `--worktree`) discovering `http-client.env.json` (public) and `http-client.private.env.json` (private). Layering, lowest-to-highest precedence: dynamic vars (`build_dynamic_variables`: `$uuid`, `$timestamp`, `$isoTimestamp`, `$randomInt`) → public env → private env → in-file `@vars` → `--var` CLI overrides → `run-all` captures from earlier requests. `resolve_variables` then expands `{{var}}` references *within* values (multi-pass), and `interpolate_text` substitutes into the URL, headers, body, and response-redirect path — failing on unknown names there but silently leaving unknowns inside variable definitions.
4. **Validate** (`validate.rs`, unless `--no-validate`) — pre-flight checks (variable completeness, and schema-aware GraphQL field validation when a cached introspection schema exists). Failures here exit with code **3**.
5. **Body materialisation** (`executor.rs`) — for `RequestBody::FromFile`, the path is interpolated then read relative to the `.http` file's directory before being passed through interpolation again. Inline bodies are interpolated directly.
6. **GraphQL transform** (`graphql.rs`) — for a `GRAPHQL` block, `render_graphql_json` splits the body on the last blank-line-separated `{ ... }` JSON suffix, treating it as `variables`, derives `operationName` from `query|mutation|subscription <Name>`, and emits the canonical `{query, variables, operationName}` payload. `Content-Type` and `Accept` are forced to `application/json`, but caller headers still override.
7. **Execute** — `main.rs::build_client` constructs the `reqwest::Client` honouring `RequestOptions` (`.timeout`, `.connect_timeout`, `.redirect(Policy::none())` when `@no-redirect`) and a persistent cookie jar (unless `--no-cookies`; jar path from `--cookie-jar` or the `.zed-http/cookies/` convention), sends the request, and feeds the response into `output.rs`.
8. **Assert / capture** (`assertion.rs` + `capture.rs`) — `evaluate_assertions` checks each `# @expect-*` against the response; a failure exits with code **2** (also the exit for a non-2xx in `run-all`). `evaluate_captures` lifts JSON-pointer / header / status values into named variables that feed the next request in a `run-all`.
9. **Output / persist** (`output.rs` + `main.rs::write_response_redirect`) — pretty / json / raw modes for stdout, plus an unconditional save to `<worktree-or-file-dir>/.zed-http/responses/<timestamp>-<slug>.<ext>` (extension derived from `Content-Type`). When the request has a `ResponseRedirect`, the body is additionally written to that path (relative to the `.http` file dir); `>>` refuses to overwrite existing files and `>>!` forces it.

Three importers convert external formats into a `RequestFile` for the formatter (they never send anything): `postman.rs` (Postman v2.1 JSON), `har.rs` (HAR 1.2 archives, gzip-aware via `flate2`), and `curl.rs` (a single "Copy as cURL" command).

Key types live in `model.rs`: `RequestFile`, `RequestBlock`, `RequestMethod` (note `GraphQl` is a *parser-level* method whose `http_method()` returns `"POST"`), `RequestBody`, `RequestOptions`, `ResponseRedirect`, `ResponseAssertion`, `CaptureDirective` / `CaptureSource`, `Header`, `InPlaceVariable`, `SourceRange`. All errors are funnelled through `HttpClientError` in `error.rs` and the CLI wraps them with the request file path via `anyhow::Context` before they reach the user.

Secret masking (`env::mask_variables`) is a pure presentation concern used only by `--verbose` output; it lower-cases the key and matches any of `token`, `secret`, `password`, `apikey`, `api_key`, `authorization`.

## Zed extension side

- The extension lives entirely under `extension/` — **deliberately separate from the repo root**. A Zed extension directory must contain *no* `Cargo.toml`: Zed treats any extension dir with a `Cargo.toml` as a Rust/WASM extension and tries to compile it to `wasm32`. The repo root has a `Cargo.toml` (the CLI workspace), which can't build for wasm, so a dev-extension install pointed at the root fails with "error compiling rust extension". Install by pointing `zed: install dev extension` at the `extension/` directory. This is a pure language extension — it needs no Rust/WASM code.
- `extension/extension.toml` declares the extension and pins the Tree-sitter grammar by commit SHA (`rev` field — update it when bumping the grammar).
- The Tree-sitter grammar lives only in its own repository, https://github.com/bovinemagnet/tree-sitter-http-request — this repo keeps no copy. Zed clones it at the pinned `rev` at install time. Grammar changes are made, committed, and tagged there, then the new SHA is copied into `extension/extension.toml`.
- `extension/languages/http-request/` holds Zed's view: `config.toml` (file suffixes `http`, `rest`), `highlights.scm`, and `runnables.scm`. The `runnables.scm` tag `http-client-request` is what Zed matches against the `tags` field in `tasks.json` — keep them in sync if renaming.

## Conventions

- Workspace edition is `2021`; MSRV is `1.74` (declared via `rust-version` in each crate and verified by the `MSRV 1.74` CI job, which runs `cargo check --workspace --locked`). The two crate versions move in lockstep with the extension version in `extension.toml` — bump them together. `Cargo.lock` is committed and CI tests run `--locked`.
- Process exit codes are load-bearing for CI: **2** = response assertion failed or a `run-all` request returned non-2xx (`TestFailure`), **3** = pre-flight validation rejected the file before sending (`ValidationFailure`), **1** = everything else. See `exit_code_for` in `main.rs`.
- Tests use `std::env::temp_dir().join(format!("...{nanos}"))` for isolation rather than the `tempfile` crate — follow that pattern when adding fixtures that touch the filesystem.
- Line numbers throughout the parser and selectors are **1-based** (matching `$ZED_ROW`); ranges are inclusive on both ends.
- The CLI's `--column` flag is accepted but currently unused — preserve it in the interface so Zed tasks don't break.
- Filesystem paths in request files — `< ./body` bodies, `# @fragments`, and `>>`/`>>!` response redirects — are resolved relative to the `.http` file's directory (absolute paths are used verbatim). They trust developer-authored input and are deliberately **not** sandboxed against `..` traversal, matching IntelliJ's HTTP Client.
