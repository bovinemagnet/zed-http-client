# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.5] - 2026-05-20

Closes the import-source trio (Postman + curl + HAR) and adds shell
completions so the CLI plays well with `cd`/tab muscle memory.

### Added

- `zed-http import har --file <path> [--out <path>] [--name-prefix <prefix>]`
  translates an HTTP Archive (HAR 1.2) export — the JSON shape browsers
  emit for "Save all as HAR with content" — into a multi-request `.http`
  file. Each `log.entries[].request` becomes one canonical request block.
  Default name shape is `<index>: <METHOD> <path>`; `--name-prefix
  "Smoke"` produces `Smoke / 1: GET /users` etc.
- HAR importer behaviour:
  - URLs keep their query strings.
  - HTTP/2 pseudo-headers (`:authority`, `:method`, `:path`, `:scheme`,
    `:status`) are stripped — they have no meaning when replayed.
  - `postData.text` becomes an inline body when present.
  - Multipart `postData.params` is recognised but the body is dropped
    with a `multipart body skipped` note in the request name, matching
    the curl importer's behaviour.
- `zed-http completions <shell>` emits clap-generated completion scripts
  for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. Pipe into your
  shell's completion directory or `eval` it for interactive use:
  - `zed-http completions bash | sudo tee /etc/bash_completion.d/zed-http`
  - `zed-http completions zsh | sudo tee /usr/local/share/zsh/site-functions/_zed-http`
  - `zed-http completions fish > ~/.config/fish/completions/zed-http.fish`
- Public core API: `import_har` (in new `har` module), re-exported at
  the crate root.

### Diagnostics

- HAR import errors as `HAR file missing /log/entries — is this a valid
  HAR 1.2 archive?` when the input has the wrong top-level shape.
- Invalid HAR JSON errors with the underlying serde_json message.

## [0.4.4] - 2026-05-20

Closes the response-capture gap that has been sitting next to `run-all`
since 0.4.2. Multi-step login → action flows now work end-to-end
without writing any glue script.

### Added

- `# @capture <variable> <source>` request-option directive lifts a
  value out of the response into a variable that later requests in the
  same `run-all` invocation can reference as `{{name}}`. Three source
  forms:
  - `json:<pointer>` — JSON Pointer into the response body. Strings,
    numbers, booleans, and `null` are stringified as-is; arrays and
    objects are re-serialised as JSON so the raw snippet can still be
    threaded through interpolation.
  - `header:<name>` — case-insensitive header lookup. Multiple matching
    headers are joined with `, ` (matches the standard merge semantics
    HTTP uses for cacheable headers).
  - `status` — three-digit status code as a string.
  - Captures whose key contains `token`, `secret`, `password`, `apikey`,
    `api_key`, or `authorization` are masked as `***` in terminal and
    JSON output. The wire request still uses the unmasked value.
  - Unresolvable captures (pointer didn't resolve, header missing,
    body wasn't JSON) emit a `<file>:<line>: capture <name> skipped:
    <reason>` warning but do *not* fail the run.
- `--var name=value` CLI flag on `run` and `run-all`, repeatable.
  Overrides every other layer in the variable stack (env files, in-file
  `@vars`, dynamic vars). Useful for seeding a fresh token from outside:
  `zed-http run-all --file login.http --var apiKey=$(security find-...)`.
- The variable resolution stack is now five layers, low → high
  precedence: dynamic vars, env-file public, env-file private, in-file
  `@vars`, then `--var` / captures.
- `zed-http format` round-trips `# @capture` directives.
- Public core API: `evaluate_captures`, `CaptureOutcome`,
  `CaptureWarning`, `CaptureDirective`, `CaptureSource`,
  `prepare_request_with_extras` (the new entry point that accepts an
  `extra_vars` overlay; `prepare_request` keeps its old signature and
  delegates).

### Diagnostics

- `@capture <variable> <source>` rejects missing variable name with
  `@capture variable name was empty`, line-numbered.
- Unknown source spec emits `@capture source must be one of
  json:<pointer>, header:<name>, status (got '<spec>')`, line-numbered.
- `--var name=value` errors at CLI parse time if the equals sign is
  missing or the name is blank.

## [0.4.3] - 2026-05-20

Adds curl import. Paste the "Copy as cURL" output from browser devtools
and get a canonical `.http` block back, ready to drop into any file.

### Added

- `zed-http import curl` translates a curl command into a single-request
  `.http` block. Three input shapes:
  - Inline positional argument: `zed-http import curl 'curl https://...'`.
  - From a file: `zed-http import curl --file paste.txt`.
  - From stdin: `pbpaste | zed-http import curl --stdin`.
  - `--out <path>` writes the result to a file (default stdout).
  - `--name <name>` overrides the imported request's name.
- Recognised curl flags: `-X` / `--request`, `-H` / `--header`,
  `-d` / `--data` / `--data-raw` / `--data-binary` / `--data-ascii` /
  `--data-urlencode`, `-u` / `--user` (base64-encoded into a
  `Authorization: Basic ...` header), `-A` / `--user-agent`,
  `-e` / `--referer`, `-b` / `--cookie` (multiple `-b` flags concatenate
  into one `Cookie:` header), `-G` / `--get` (moves `-d` data into the
  query string), `--url`, the bare URL, and backslash line continuations
  for multi-line shapes from Chrome/Firefox devtools.
- Acceptable but ignored: `--compressed`, `-L` / `--location`,
  `-s` / `--silent`, `-k` / `--insecure`, `-v` / `--verbose`, etc.
- Multipart (`-F` / `--form`) is recognised but the body is skipped
  with a note in the request name — the runtime doesn't render
  multipart yet (deferred to v0.5).
- `-d @path` syntax becomes a `< ./path` body-from-file directive.
- `--data-urlencode` values are percent-encoded with the
  `name=value & special chars` shape expected by web APIs.
- Public core API: `import_curl` (in new `curl` module), re-exported at
  the crate root.

### Diagnostics

- Unterminated quoted strings in the input emit `curl command had an
  unterminated quoted string`.
- Unknown HTTP methods passed to `-X` error with `curl -X used an
  unknown HTTP method '<text>'`.
- Missing URL emits `curl command had no URL`.

## [0.4.2] - 2026-05-20

Two practical additions for everyday use: IntelliJ-style dynamic
variables for unique IDs and timestamps, and a `run-all` subcommand
that executes every request in a file with a pass/fail summary.

### Added

- IntelliJ-compatible dynamic variables, generated fresh per
  `prepare_request` call but consistent within a single request so the
  same `{{$uuid}}` can be replayed in both a header and a body:
  - `{{$uuid}}` — UUID v7 (time-ordered, RFC 9562), lowercase, hyphenated.
  - `{{$timestamp}}` — current Unix time in seconds.
  - `{{$isoTimestamp}}` — current time as RFC 3339 / ISO 8601 with `Z`.
  - `{{$randomInt}}` — uniform integer in `[0, 1000]`.
  - User-defined variables of the same name shadow the dynamic value
    (`@$timestamp = 1700000000` for deterministic tests).
- `zed-http run-all` runs every request in a file in declaration order.
  - Per-request status line with `✓` / `✗`, status text, and duration.
  - Final summary line with passed/failed/skipped counts.
  - `--bail` stops at the first failure (skipped count reflects what
    didn't run).
  - `--env <name>`, `--worktree <path>`, `--no-validate`, `--no-cookies`,
    and `--cookie-jar <path>` flags mirror `run`.
  - One shared cookie jar across all iterations, persisted once at the
    end, so login → action flows survive between requests.
  - Pretty / JSON / Raw output modes. JSON envelope includes the full
    per-request structure with assertion failures.
  - Exit code is non-zero if any request fails.
- Public core API: `build_dynamic_variables` (in new `dynamic` module),
  re-exported at the crate root.
- Interpolation regex updated to recognise `$` in variable names so
  `{{$uuid}}` resolves.

### Changed

- `RequestOutcome` extracted from the body of `run_command` into a
  shared internal helper so `run-all` can reuse the same execute →
  capture → assert → persist pipeline without code duplication.

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
