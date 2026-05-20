# zed-http-client

A Zed extension and companion CLI for running IntelliJ-style HTTP and GraphQL request files inside Zed or from the terminal.

> [!WARNING]
> This project aims for practical compatibility with the JetBrains HTTP Client file format, but it is not affiliated with JetBrains.

## Status

Experimental. The first version focuses on a reliable terminal-first workflow:

- `.http` and `.rest` language support for Zed
- syntax highlighting and runnable detection
- a Rust CLI for parsing, interpolating, and executing requests
- IntelliJ-compatible environment file discovery
- response persistence under `.zed-http/responses`

## Repository layout

```text
zed-http-client/
  README.md
  LICENSE
  .gitignore
  extension.toml
  tasks.json.example
  Cargo.toml
  crates/
    zed-http-cli/
    zed-http-core/
  languages/
    http-request/
  grammars/
    tree-sitter-http-request/
  examples/
```

## Building the CLI

```bash
cargo build -p zed-http-cli
```

You can also run the CLI directly during development:

```bash
cargo run -p zed-http-cli -- --help
```

## CLI usage

The workspace builds a binary named `zed-http`.

```bash
zed-http run --file examples/requests.http --line 1
zed-http run --file examples/requests.http --line 20 --env dev
zed-http run --file examples/requests.http --name "Create user" --env dev
zed-http list --file examples/requests.http
zed-http envs --file examples/requests.http
zed-http format --file examples/requests.http              # print canonical form
zed-http format --file examples/requests.http --in-place   # rewrite in place
zed-http format --file examples/requests.http --check      # CI-friendly exit code
zed-http check --file examples/requests.http               # validate without sending
zed-http check --file examples/requests.http --env dev     # use env for schema lookup
zed-http introspect --file examples/requests.http --name "GraphQL user query"
zed-http schema list                                       # cached schemas
zed-http schema show --host countries.trevorblades.com
zed-http import postman --file collection.json --out requests.http
```

### Run a request

```bash
cargo run -p zed-http-cli -- run   --file examples/requests.http   --line 4   --env dev   --worktree .
```

Useful options:

- `--line <number>`: 1-based line number used to select the containing request block
- `--name <name>`: selects the request whose `###` heading matches (case-insensitive). Mutually exclusive with `--line`
- `--column <number>`: reserved for future use
- `--env <name>`: selects values from `http-client.env.json` and `http-client.private.env.json`
- `--worktree <path>`: stops environment lookup at the provided directory
- `--output <pretty|json|raw>`: controls terminal output format
- `--verbose`: prints masked, fully resolved request details before execution
- `--no-validate`: skip the pre-flight validation pass
- `--no-cookies`: don't read or write the cookie jar for this invocation
- `--cookie-jar <path>`: override the cookie jar location (default `<base>/.zed-http/cookies.json`)

### List requests

```bash
cargo run -p zed-http-cli -- list --file examples/requests.http
```

Example output:

```text
1. Health check        GET      line 4
2. List users          GET      line 8
3. Create user         POST     line 13
4. Update user         PATCH    line 22
5. Delete user         DELETE   line 30
6. GraphQL user query  GRAPHQL  line 34
```

### List environments

```bash
cargo run -p zed-http-cli -- envs --file examples/requests.http
```

### Validate a request file

```bash
cargo run -p zed-http-cli -- check --file examples/requests.http
```

Static validation. Today it covers:

- non-empty request URLs
- GraphQL bodies parse into `{query, variables, operationName}`
- GraphQL variable definitions on the operation match the JSON variables block
  (required-but-missing, required-but-null, declared-but-extra)
- Top-level field selections exist on the schema's root type, when a schema
  is cached at `<base>/.zed-http/schema/<host>.json` (populated by
  `zed-http introspect`)

`zed-http run` runs the same checks against the selected request before
sending. Pass `--no-validate` to skip them for ad-hoc debugging. Schema-aware
checks need a resolvable URL — `check --env <name>` resolves env-file
interpolation before looking up the cached schema; requests whose URL still
contains `{{vars}}` after that skip the schema step rather than reporting
false positives.

### Inspect cached schemas

```bash
cargo run -p zed-http-cli -- schema list
cargo run -p zed-http-cli -- schema show --host countries.trevorblades.com
```

### Response assertions

Add `# @expect-*` directives to a request and `run` will fail the invocation
if the response doesn't match:

```http
### Health check
# @expect-status 200,204
# @expect-header content-type application/json
# @expect-json /status ok
GET {{host}}/health
```

Failures appear as `path:line: <message>` on stderr and surface in JSON
output mode as `response.assertion_failures`.

### Persistent cookie jar

`zed-http run` automatically loads and saves cookies between invocations at
`<base>/.zed-http/cookies.json`. Both persistent and session-scoped cookies
are kept so multi-step login → action flows work across separate CLI runs.
Disable with `--no-cookies` or relocate the jar with `--cookie-jar <path>`.

### Import a Postman collection

```bash
cargo run -p zed-http-cli -- import postman --file collection.json
cargo run -p zed-http-cli -- import postman --file collection.json --out requests.http
```

Translates a Postman v2.1 collection into a canonical `.http` file. Nested
folders are flattened into request names (`Parent / Child`). Collection
variables become `@name = value` declarations. `raw` and `graphql` body
modes are supported; multipart/form-data bodies are skipped (deferred to a
later release).

### Include GraphQL fragments from another file

```http
### Spread
# @fragments ./fragments.graphql
GRAPHQL {{host}}/graphql

query GetUser($id: ID!) {
  user(id: $id) {
    ...UserFragment
  }
}

{ "id": "{{userId}}" }
```

The fragment file is read relative to the `.http` file, env-variable
interpolated, and concatenated onto the GraphQL query before sending.
Multiple `# @fragments` lines accumulate.

### Format a request file

```bash
cargo run -p zed-http-cli -- format --file examples/requests.http
cargo run -p zed-http-cli -- format --file examples/requests.http --in-place
cargo run -p zed-http-cli -- format --file examples/requests.http --check
```

Note: formatting preserves every parseable construct (variables, request
names, headers, bodies, body-from-file references, response redirects, option
directives) but does not preserve non-directive comments inside requests.

### Introspect a GraphQL endpoint

```bash
cargo run -p zed-http-cli -- introspect --file examples/requests.http \
  --name "GraphQL user query" --env dev
```

Runs the standard introspection query against the URL and headers of the
selected `GRAPHQL` request (env variables, bearer tokens, and in-file
`@vars` are honoured). The schema's `.data` payload is written to
`.zed-http/schema/<host>.json` under the worktree (or `--output <path>` to
override).

## Zed integration

The extension currently focuses on:

- file type recognition for `.http` and `.rest`
- Tree-sitter-based highlighting
- runnable request block detection
- task integration that executes the companion CLI in Zed's terminal

Use `tasks.json.example` as a starting point, or copy one of the examples below into your Zed tasks file.

### Development task using Cargo

```json
[
  {
    "label": "HTTP Client: run request under cursor",
    "command": "cargo",
    "args": [
      "run",
      "-p",
      "zed-http-cli",
      "--",
      "run",
      "--file",
      "$ZED_FILE",
      "--line",
      "$ZED_ROW",
      "--column",
      "$ZED_COLUMN",
      "--worktree",
      "$ZED_WORKTREE_ROOT"
    ],
    "reveal": "always",
    "hide": "never",
    "use_new_terminal": false,
    "allow_concurrent_runs": true,
    "save": "current",
    "tags": ["http-client-request"]
  }
]
```

### Installed binary task

```json
[
  {
    "label": "HTTP Client: run request under cursor",
    "command": "zed-http",
    "args": [
      "run",
      "--file",
      "$ZED_FILE",
      "--line",
      "$ZED_ROW",
      "--column",
      "$ZED_COLUMN",
      "--worktree",
      "$ZED_WORKTREE_ROOT"
    ],
    "reveal": "always",
    "hide": "never",
    "use_new_terminal": false,
    "allow_concurrent_runs": true,
    "save": "current",
    "tags": ["http-client-request"]
  }
]
```

## Supported IntelliJ HTTP Client syntax

The initial parser intentionally focuses on the most common primitives:

- request blocks separated by `###`
- optional request names after `###`
- HTTP methods: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`
- `GRAPHQL` requests that are executed as HTTP `POST`
- headers in `Name: Value` form
- blank-line-separated request bodies
- bodies sourced from a file using `< ./path.json`
- response redirects using `>> ./path` (refuses to overwrite) and `>>! ./path` (force overwrite)
- per-request options as `# @timeout <ms>`, `# @connection-timeout <ms>`, `# @no-redirect`
- file-level `# @env <name>` directive that selects the environment when `--env` isn't passed
- in-file variables like `@host = http://localhost:8080`
- interpolation using `{{variableName}}`

Example combining the new directives:

```http
### Slow upload
# @timeout 5000
# @connection-timeout 1000
# @no-redirect
POST {{host}}/api/users
Content-Type: application/json

< ./create-user.json

>>! ./.zed-http/last-create-user.json
```

Paths after `<`, `>>`, and `>>!` are resolved relative to the directory containing the request file.

## Supported GraphQL syntax

`GRAPHQL` requests are converted into JSON payloads before execution.

```http
GRAPHQL {{host}}/graphql
Authorization: Bearer {{token}}

query GetUser($id: ID!) {
  user(id: $id) {
    id
    name
  }
}

{
  "id": "123"
}
```

The CLI sends:

```json
{
  "query": "query GetUser($id: ID!) { user(id: $id) { id name } }",
  "variables": {
    "id": "123"
  },
  "operationName": "GetUser"
}
```

## Environment file compatibility

The CLI searches for IntelliJ-compatible environment files starting in the request file's directory and walking upward until the worktree root or filesystem root:

- `http-client.env.json`
- `http-client.private.env.json`

Resolution order:

1. load public values
2. overlay private values
3. overlay in-file `@variables`
4. apply future CLI overrides

The environment is selected via `--env <name>`. If no flag is passed, the
file may declare a default with `# @env <name>` at the top:

```http
# @env dev

@localToken = local-token

### Health check
GET {{host}}/health
```

CLI flag wins over the directive. The directive lets one `.http` file
travel with a sensible default that matches how the requests were written.

Secret-looking keys are masked in verbose output for names containing `token`, `secret`, `password`, `apikey`, `api_key`, or `authorization`.

## Output and saved responses

Requests print status, duration, content type, and a preview in the terminal. Full bodies are also saved to:

```text
.zed-http/responses/
```

File extensions are inferred from the response content type when possible.

## Current limitations

Not implemented yet:

- multipart forms
- request scripts
- response scripts
- cookie jars
- OAuth flows
- client certificates
- a custom rich Zed output pane

## Roadmap

- richer Tree-sitter highlighting and injections
- installed binary packaging for easier Zed setup
- CLI overrides for variables and headers
- richer response viewers inside Zed
- broader IntelliJ HTTP Client feature coverage
