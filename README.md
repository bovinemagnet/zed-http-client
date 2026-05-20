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
