# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.3] - 2026-05-20

First tagged release. Covers the v0.1 MVP from the initial PRD plus the v0.2
"daily-driver quality" tier.

### Added

- CLI binary `zed-http` with `run`, `list`, and `envs` subcommands.
- `.http` and `.rest` language support for Zed: file-type recognition,
  Tree-sitter grammar (`tree-sitter-http-request`), syntax highlighting, and
  runnable detection (`http-client-request` tag).
- IntelliJ-compatible `.http` parser supporting:
  - `###`-separated request blocks with optional names.
  - Methods `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`.
  - `GRAPHQL <url>` pseudo-method executed as HTTP `POST` with a canonical
    `{query, variables, operationName}` JSON payload.
  - In-file `@name = value` variables and `{{var}}` interpolation, including
    nested variable references resolved in multiple passes.
  - JSON request bodies.
  - Body sourced from a file via `< ./path` (path resolved relative to the
    `.http` file's directory; contents are interpolated).
  - Response redirect via `>> ./path` (refuses to overwrite) and `>>! ./path`
    (force overwrite).
  - Per-request options as comment directives: `# @timeout <ms>`,
    `# @connection-timeout <ms>`, `# @no-redirect`. Unknown directives are
    silently ignored for forward compatibility.
- IntelliJ-compatible environment file discovery: walks up from the request
  file's directory to `--worktree` looking for `http-client.env.json` (public)
  and `http-client.private.env.json` (private), with private overlaying public
  and in-file `@vars` overlaying both.
- `run` flags: `--line`, `--name` (case-insensitive), `--env`, `--worktree`,
  `--output {pretty,json,raw}`, `--verbose` (masks secret-looking variables in
  the resolved-request dump).
- HTTP client honours per-request `@timeout`, `@connection-timeout`, and
  `@no-redirect`.
- Responses are saved under `.zed-http/responses/<timestamp>-<slug>.<ext>`
  alongside the request file (extension inferred from response `Content-Type`).
- Pretty terminal output with status, duration, content-type, saved path,
  preview, and (when configured) response redirect path.
- Snippets file (`snippets/http-request.json`) with templates for GET, POST,
  PUT, PATCH, DELETE, GRAPHQL, `@variable`, and a request-with-options block.
- Example request file demonstrating env interpolation, body-from-file, and
  response redirect (`examples/requests.http`, `examples/create-user.json`).
- Diagnostics now wrap parser/executor errors with the originating request
  file path via `anyhow::Context`.

### Known limitations

- No multipart forms, request/response scripts, cookie jar, OAuth flow,
  client-certificate config, or custom Zed output pane (deferred to later
  milestones per the PRD).
- `--column` is accepted for forward compatibility but not yet used.
