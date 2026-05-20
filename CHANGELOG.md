# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.1] - 2026-05-20

Small follow-up that makes per-environment runs ergonomic to set up from
inside the `.http` file itself, rather than forcing the choice into the
Zed task entry.

### Added

- `# @env <name>` directive parsed from the file's top-of-file preamble
  (before the first `###` separator). Selects which environment from
  `http-client.env.json` / `http-client.private.env.json` is overlaid when
  no `--env` flag is passed.
- Precedence: `--env <name>` on the CLI wins; otherwise the directive
  applies; otherwise no environment is loaded and only in-file `@vars`
  resolve.
- `zed-http format` round-trips the directive — emits `# @env <name>` at
  the top of the formatted output.
- `zed-http check` honours the directive when looking up cached schemas
  for `{{host}}`-style URLs, so schema validation works in CI without
  having to thread `--env` through.

### Diagnostics

- Duplicate `# @env` declarations error with `@env is declared more than
  once`, line-numbered.
- `# @env` with no value errors with `@env requires an environment name`,
  line-numbered.

### Documentation

- Module-level doc comments added to every Rust source file across
  `zed-http-core` and `zed-http-cli`, summarising what each module does
  and what surrounding code can rely on. Inline commentary was kept to
  WHY-only per the repo style.

## [0.4.0] - 2026-05-20

Opens the v0.4 "IntelliJ parity push" tier from the initial PRD with three
tractable items: a persistent cookie jar, response assertions, and Postman
collection import. JS pre-/post-request scripts and the manual multipart
body syntax remain deferred.

### Added

- Persistent cookie jar shared between `zed-http run` invocations. Defaults
  to `<base>/.zed-http/cookies.json` (mirroring the other artifact dirs).
  Both persistent and session-scoped cookies are serialised so multi-step
  workflows survive across separate CLI invocations. Flags:
  - `--no-cookies` disables the jar for this invocation.
  - `--cookie-jar <path>` overrides the jar location.
- Response assertions via `# @` directives, parsed in the section preamble:
  - `# @expect-status <code[,code,...]>` — accepts any of the listed codes.
  - `# @expect-header <name> <substring>` — case-insensitive header lookup,
    substring match on the value.
  - `# @expect-json <pointer> <expected>` — JSON Pointer into the response
    body, equality match against the literal expected text (string, number,
    bool, or `null`).
  - Failures are printed with `<file>:<line>: <message>`, surfaced in JSON
    output as `response.assertion_failures`, and cause a non-zero exit.
  - The formatter (`zed-http format`) emits each directive back on
    re-serialisation.
- `zed-http import postman --file <collection.json> [--out <path>]`
  translates a Postman v2.1 collection into a canonical `.http` file
  (default stdout). Supports nested folders (joined as `parent / child` in
  request names), GET/POST/etc., `raw` JSON bodies, and `graphql` body mode
  (becomes a `GRAPHQL` request). Collection variables become `@var = value`.
  Multipart/form-data bodies are skipped (deferred to v0.5).
- Public core API: `cookie_jar_path`, `evaluate_assertions`,
  `AssertionFailure`, `AssertionResponse`, `ResponseAssertion`,
  `import_postman_collection`.

### Changed

- `RequestBlock` grows an `assertions: Vec<ResponseAssertion>` field.
- `RunOptions` now carries `no_cookies` and `cookie_jar` so the cookie jar
  state flows alongside the other run options.
- `zed-http-cli` depends on `cookie_store` (0.21) + `reqwest_cookie_store`
  (0.8). The `reqwest` feature set gains `cookies`.

## [0.3.0] - 2026-05-20

Completes the v0.3 GraphQL productivity tier from the initial PRD with
schema-aware validation, schema inspection, and fragment inclusion.

### Added

- Schema-aware validation: when a schema is cached at
  `<base>/.zed-http/schema/<host>.json` (populated by `introspect`), `check`
  and `run` walk each GRAPHQL request's top-level field selections and flag
  any field that isn't declared on the schema's root type for the operation
  kind (`query` / `mutation` / `subscription`). Inline-fragment and
  fragment-spread selections are deliberately skipped at this fidelity.
- `zed-http schema list [--worktree <path>]` prints every cached schema
  under `<base>/.zed-http/schema/` with byte sizes.
- `zed-http schema show --host <host> [--worktree <path>]` summarises a
  cached schema: root type names, total type count, and root field counts.
- `# @fragments <path>` request-option directive concatenates a fragment
  file (relative to the `.http` file's directory) onto the GraphQL query
  before sending. Multiple `# @fragments` directives accumulate. Fragment
  contents are env-variable-interpolated.
- `zed-http check` now accepts `--env <name>` and `--worktree <path>` so it
  can resolve request URLs against environment files when picking the
  matching schema. Requests with unresolved `{{vars}}` in their URL skip
  schema validation rather than reporting false positives.
- `zed-http run` schema validation: runs schema-aware checks against the
  *resolved* request URL after `prepare_request` (so env interpolation is
  honoured). Opt out with `--no-validate` as before.
- Public core API: `cached_schema_path`, `load_cached_schema`,
  `detect_operation_kind`, `schema_slug`, `validate_against_schema`,
  `validate_request_file_with_schemas`, `validate_request_with_schema`,
  `render_graphql_json_with_extras`.

### Changed

- `RequestOptions` gained a `fragment_paths: Vec<String>` field; the
  formatter emits one `# @fragments <path>` line per entry.
- `zed-http-core` now depends on the `url` crate (for host extraction in
  the schema slug).

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
