# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-05-20

Continues the v0.3 GraphQL productivity tier with schema-free static
validation, exposed as both a standalone `check` command and a pre-flight
gate in `run`.

### Added

- `zed-http check --file <path>` parses every request in a file and reports
  validation issues (with file:line annotations) without making any HTTP
  request. Exits non-zero on any issue.
- `zed-http run` now validates the selected request before sending. Pass
  `--no-validate` to skip the check for ad-hoc debugging.
- GraphQL variables validation, schema-free:
  - Parses `($id: ID!, $limit: Int = 10)` variable definitions from the
    operation (anonymous operations and `query`/`mutation`/`subscription`
    keywords supported).
  - Reports required variables missing from the JSON variables block.
  - Reports required variables provided as `null`.
  - Reports extra variables in the JSON block that aren't declared on the
    operation.
- Public core API: `validate_request_file`, `validate_request`,
  `ValidationIssue`, `parse_variable_definitions`, `validate_variables`,
  `VariableDefinition`.

### Changed

- `run_command` internals refactored to take a `RunOptions` struct so the
  validation flag fits without tripping clippy's `too_many_arguments`.

## [0.1.0] - 2026-05-20

Starts the v0.3 GraphQL productivity tier and lands the long-promised
`format` command from PRD section 5.

### Added

- `zed-http format --file <path>` re-emits the parsed request file in a
  canonical layout (trimmed whitespace, single space after the method,
  `Header: value`, blank line before body, options before request line,
  response redirect after body). Variants:
  - default: print to stdout
  - `--in-place`: overwrite the source file
  - `--check`: exit non-zero if the file is not already canonical
- `zed-http introspect` runs the standard GraphQL introspection query against
  the URL and headers of a selected `GRAPHQL` request (`--line` / `--name` /
  first), reusing env-file interpolation so bearer tokens work out of the box.
  - Result `.data` is cached at `<base>/.zed-http/schema/<slug>.json` (slug
    derives from the request URL host), or at a custom `--output` path.
- Public core API: `format_request_file`, `INTROSPECTION_QUERY`,
  `introspection_payload`, `schema_root`.

### Known limitations

- `zed-http format` discards non-directive comments (anything other than
  `# @option …`) inside requests. Document-level commentary outside requests
  is also dropped. Round-tripping otherwise is structurally lossless.

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
