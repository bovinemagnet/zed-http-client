# Build an initial Zed HTTP + GraphQL Client extension repository

Create an initial repository for a Zed editor extension named **zed-http-client**.

The goal is to build a Zed extension that supports IntelliJ HTTP Client-style `.http` / `.rest` files and allows users to run HTTP and GraphQL requests from inside Zed.

The extension should behave similarly to the IntelliJ HTTP Client and Altair GraphQL Client, but using Zed-native concepts where possible.

The first version should focus on:

1. `.http` and `.rest` language support.
2. Syntax highlighting for HTTP request files.
3. Detecting runnable request blocks.
4. A companion CLI that executes the request under the cursor.
5. Compatibility with `http-client.env.json` and `http-client.private.env.json`.
6. HTTP methods: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`.
7. GraphQL requests using the `GRAPHQL` keyword.
8. Pretty terminal output.
9. Saving responses to `.zed-http/responses`.

Do not attempt to build a custom rich Zed output pane yet. For now, task output should go to the Zed terminal, and full responses should also be saved to files.

---

## Repository structure

Create this structure:

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
      Cargo.toml
      src/
        main.rs
    zed-http-core/
      Cargo.toml
      src/
        lib.rs
        model.rs
        parser.rs
        env.rs
        interpolate.rs
        executor.rs
        graphql.rs
        output.rs
        error.rs

  languages/
    http-request/
      config.toml
      highlights.scm
      injections.scm
      runnables.scm

  grammars/
    tree-sitter-http-request/
      README.md
      grammar.js
      package.json
      queries/
        highlights.scm

  examples/
    requests.http
    http-client.env.json
    http-client.private.env.json.example
```

Use Rust for the CLI and core library.

The Zed extension itself should be minimal initially: language registration, Tree-sitter grammar configuration, syntax highlighting, and runnable request detection.

The CLI should be independently usable from the terminal.

---

## Main commands

Create a binary named:

```bash
zed-http
```

It should support these commands:

```bash
zed-http run --file examples/requests.http --line 1
zed-http run --file examples/requests.http --line 20 --env dev
zed-http list --file examples/requests.http
zed-http envs --file examples/requests.http
```

Initial CLI behaviour:

### `run`

Runs the request that contains the given line number.

Arguments:

```text
--file <path>       Required. Path to .http/.rest file.
--line <number>     Optional. 1-based line number. Defaults to first request.
--column <number>   Optional. Future-use only.
--env <name>        Optional. Environment name.
--worktree <path>   Optional. Root directory used when searching env files.
--output <mode>     Optional. One of: pretty, json, raw. Default: pretty.
--verbose           Optional. Print resolved request details.
```

### `list`

Lists all request blocks in the file.

Output example:

```text
1. List users          GET      line 5
2. Create user         POST     line 14
3. Get user GraphQL    GRAPHQL  line 28
```

### `envs`

Lists available environments discovered from `http-client.env.json` and `http-client.private.env.json`.

Output example:

```text
dev
test
prod
```

---

## HTTP request file format

Support request blocks separated by `###`.

Example:

```http
### List users
GET {{host}}/api/users
Authorization: Bearer {{token}}
Accept: application/json

### Create user
POST {{host}}/api/users
Authorization: Bearer {{token}}
Content-Type: application/json

{
  "name": "Alice",
  "email": "alice@example.com"
}

### Get user via GraphQL
GRAPHQL {{host}}/graphql
Authorization: Bearer {{token}}

query GetUser($id: ID!) {
  user(id: $id) {
    id
    name
    email
  }
}

{
  "id": "123"
}
```

Also support in-place variables:

```http
@host = http://localhost:8080
@token = local-dev-token

### List users
GET {{host}}/api/users
Authorization: Bearer {{token}}
```

Variable interpolation should use `{{variableName}}`.

---

## Environment files

Support IntelliJ-style files:

```text
http-client.env.json
http-client.private.env.json
```

Example public env file:

```json
{
  "dev": {
    "host": "http://localhost:8080",
    "token": ""
  },
  "prod": {
    "host": "https://api.example.com",
    "token": ""
  }
}
```

Example private env file:

```json
{
  "dev": {
    "token": "dev-secret-token"
  },
  "prod": {
    "token": "prod-secret-token"
  }
}
```

Resolution rules:

1. Start in the directory of the `.http` file.
2. Search for `http-client.env.json`.
3. Search for `http-client.private.env.json`.
4. If not found, walk up parent directories until the worktree root or filesystem root.
5. Load public values first.
6. Overlay private values.
7. Overlay in-place variables from the `.http` file.
8. Apply CLI overrides later, but CLI overrides can be stubbed for now.

Private values should override public values.

Do not print secret values in verbose output by default. Mask values for keys containing:

```text
token
secret
password
apikey
api_key
authorization
```

---

## Parser requirements

Implement a tolerant parser in Rust.

Create models similar to:

```rust
pub struct RequestFile {
    pub variables: Vec<InPlaceVariable>,
    pub requests: Vec<RequestBlock>,
}

pub struct InPlaceVariable {
    pub name: String,
    pub value: String,
    pub line: usize,
}

pub struct RequestBlock {
    pub name: Option<String>,
    pub method: RequestMethod,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: Option<String>,
    pub range: SourceRange,
}

pub enum RequestMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    GraphQl,
}

pub struct Header {
    pub name: String,
    pub value: String,
    pub line: usize,
}

pub struct SourceRange {
    pub start_line: usize,
    pub end_line: usize,
}
```

Parsing rules:

* Request blocks are separated by `###`.
* A name may appear after `###`.
* Ignore blank lines before request line.
* Ignore comments before request line.
* Request line may be:

  * `GET https://example.com`
  * `POST {{host}}/api/users`
  * `GRAPHQL {{host}}/graphql`
* Headers continue until the first blank line.
* Everything after the blank line is the body.
* If no blank line and no body exists, request has no body.
* In-place variables are lines like `@name = value`.
* In-place variables outside request blocks apply globally.

For the first version, do not implement:

* multipart forms
* request scripts
* response scripts
* cookie jars
* OAuth
* client certificates

Stub those as future TODOs in the README.

---

## GraphQL handling

A `GRAPHQL` request should execute as an HTTP `POST`.

For example:

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

Should become:

```json
{
  "query": "query GetUser($id: ID!) { user(id: $id) { id name } }",
  "variables": {
    "id": "123"
  },
  "operationName": "GetUser"
}
```

Rules:

* The first GraphQL document is the query/mutation/subscription text.
* If a trailing JSON object exists after the GraphQL operation, treat it as variables.
* Try to detect `operationName` from:

  * `query GetUser`
  * `mutation CreateUser`
  * `subscription UserEvents`
* Set headers automatically:

  * `Content-Type: application/json`
  * `Accept: application/json`
* User-supplied headers should override defaults.

The GraphQL parser can be simple in v1. It does not need full GraphQL AST parsing yet.

---

## HTTP executor

Use `reqwest` with async Tokio runtime.

Support:

```text
GET
POST
PUT
PATCH
DELETE
HEAD
OPTIONS
GRAPHQL as POST
```

Executor should:

1. Parse the file.
2. Select request by line number.
3. Load environment values.
4. Interpolate variables in URL, headers, and body.
5. Execute the request.
6. Measure duration.
7. Print pretty output.
8. Save response body to `.zed-http/responses`.

Response files should be named safely, for example:

```text
.zed-http/responses/2026-05-20T14-32-10-list-users.json
.zed-http/responses/2026-05-20T14-32-10-create-user.txt
```

Use content type to choose extension:

```text
application/json       .json
application/graphql    .graphql
text/html              .html
text/plain             .txt
default                .body
```

Pretty output example:

```text
▶ List users
GET http://localhost:8080/api/users

Status: 200 OK
Duration: 87 ms
Content-Type: application/json

Response saved:
.zed-http/responses/2026-05-20T14-32-10-list-users.json

{
  "users": [
    {
      "id": 1,
      "name": "Alice"
    }
  ]
}
```

For JSON responses, pretty-print JSON.

For non-JSON responses, print the first reasonable chunk to terminal and save the full body to file.

---

## Zed extension files

Create `extension.toml`:

```toml
id = "http-client"
name = "HTTP Client"
version = "0.1.0"
schema_version = 1
authors = ["Paul Snow <bovinemagnet@gmail.com>"]
description = "Run IntelliJ-compatible HTTP and GraphQL request files from Zed"
repository = "https://github.com/bovinemagnet/zed-http-client"

[grammars.http-request]
repository = "https://github.com/bovinemagnet/tree-sitter-http-request"
rev = "REPLACE_WITH_COMMIT_SHA"
```

Create `languages/http-request/config.toml`:

```toml
name = "HTTP Request"
grammar = "http-request"
path_suffixes = ["http", "rest"]
line_comments = ["#", "//"]

brackets = [
  { start = "{", end = "}", close = true, newline = true },
  { start = "[", end = "]", close = true, newline = true },
  { start = "(", end = ")", close = true, newline = false }
]
```

Create initial `languages/http-request/highlights.scm` for:

```text
HTTP methods
GRAPHQL keyword
headers
URLs
variables
request separators
comments
JSON-ish strings/numbers when possible
```

Create `languages/http-request/runnables.scm`.

The exact node names may depend on the Tree-sitter grammar, but aim for something like:

```scheme
(
  (request
    (request_line) @run)
  (#set! tag http-client-request)
)
```

Create `languages/http-request/injections.scm` to inject GraphQL and JSON bodies later. It can be minimal initially with TODOs.

---

## Tree-sitter grammar

Create a minimal Tree-sitter grammar in `grammars/tree-sitter-http-request`.

It should identify:

```text
source_file
variable_declaration
request
request_separator
request_name
request_line
method
url
header
header_name
header_value
body
comment
```

The grammar should be good enough for highlighting and runnable detection. It does not need to be perfect for semantic execution because the Rust CLI parser is the source of truth for execution.

Support file examples like:

```http
@host = http://localhost:8080

### List users
GET {{host}}/api/users
Accept: application/json

### GraphQL example
GRAPHQL {{host}}/graphql
Content-Type: application/json

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

---

## Example files

Create `examples/requests.http`:

```http
@localToken = local-token-from-file

### Health check
GET {{host}}/actuator/health
Accept: application/json

### List users
GET {{host}}/api/users
Authorization: Bearer {{token}}
Accept: application/json

### Create user
POST {{host}}/api/users
Authorization: Bearer {{token}}
Content-Type: application/json

{
  "name": "Alice",
  "email": "alice@example.com"
}

### Update user
PATCH {{host}}/api/users/123
Authorization: Bearer {{token}}
Content-Type: application/json

{
  "name": "Alice Updated"
}

### Delete user
DELETE {{host}}/api/users/123
Authorization: Bearer {{token}}

### GraphQL user query
GRAPHQL {{host}}/graphql
Authorization: Bearer {{token}}

query GetUser($id: ID!) {
  user(id: $id) {
    id
    name
    email
  }
}

{
  "id": "123"
}
```

Create `examples/http-client.env.json`:

```json
{
  "dev": {
    "host": "http://localhost:8080",
    "token": ""
  },
  "test": {
    "host": "https://test.example.com",
    "token": ""
  },
  "prod": {
    "host": "https://api.example.com",
    "token": ""
  }
}
```

Create `examples/http-client.private.env.json.example`:

```json
{
  "dev": {
    "token": "replace-me"
  },
  "test": {
    "token": "replace-me"
  },
  "prod": {
    "token": "replace-me"
  }
}
```

Add `http-client.private.env.json` to `.gitignore`.

---

## Cargo workspace

Use a Cargo workspace:

```toml
[workspace]
members = [
  "crates/zed-http-core",
  "crates/zed-http-cli"
]
resolver = "2"
```

Core crate dependencies:

```toml
[dependencies]
anyhow = "1"
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
regex = "1"
chrono = { version = "0.4", features = ["serde"] }
indexmap = "2"
```

CLI crate dependencies:

```toml
[dependencies]
zed-http-core = { path = "../zed-http-core" }
anyhow = "1"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
serde_json = "1"
```

Prefer `rustls-tls` over native TLS for predictable cross-platform behaviour.

---

## Tests

Add unit tests for the parser.

Test cases:

1. Single GET request.
2. Multiple requests separated by `###`.
3. Request names.
4. In-place variables.
5. Headers.
6. JSON body.
7. GraphQL body with variables.
8. Selecting request by line number.
9. Environment public/private overlay.
10. Variable interpolation.

Example parser test:

```rust
#[test]
fn parses_named_get_request() {
    let input = r#"
### List users
GET {{host}}/api/users
Accept: application/json
"#;

    let file = parse_request_file(input).unwrap();

    assert_eq!(file.requests.len(), 1);
    assert_eq!(file.requests[0].name.as_deref(), Some("List users"));
    assert_eq!(file.requests[0].url, "{{host}}/api/users");
}
```

---

## README content

The README should explain:

```text
What this project is
Current status: experimental
How to build the CLI
How to run a request
How to configure Zed tasks
Supported IntelliJ HTTP Client syntax
Supported GraphQL syntax
Environment file compatibility
Current limitations
Roadmap
```

Include this warning:

```text
This project aims for practical compatibility with the JetBrains HTTP Client file format, but it is not affiliated with JetBrains.
```

Include an example Zed task:

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

Also include a future installed-binary version:

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

---

## Implementation notes

Keep the initial implementation pragmatic.

The Rust semantic parser should be the source of truth for execution.

The Tree-sitter grammar is only for editor support.

Do not spend too much time making the Tree-sitter grammar perfect in the first commit.

Prioritise a working flow:

```text
Open examples/requests.http in Zed
Place cursor inside a request
Run task
CLI finds request by line
CLI loads env
CLI interpolates variables
CLI executes request
CLI prints response
CLI saves response file
```

---

## Acceptance criteria

The initial repository is complete when:

1. `cargo test` passes.
2. `cargo run -p zed-http-cli -- list --file examples/requests.http` lists all example requests.
3. `cargo run -p zed-http-cli -- envs --file examples/requests.http` lists `dev`, `test`, and `prod`.
4. `cargo run -p zed-http-cli -- run --file examples/requests.http --line 4 --env dev` attempts to run the first request.
5. Parser tests cover HTTP, GraphQL, variables, and env merging.
6. README explains how to use the project from terminal and Zed.
7. `.gitignore` prevents committing private env files and generated responses.
8. The Zed language folder exists with initial `config.toml`, `highlights.scm`, `runnables.scm`, and `injections.scm`.

---

## Important design decisions

Use this approach:

```text
Zed extension:
  - editor integration
  - highlighting
  - runnables
  - file type recognition

Rust CLI:
  - parsing
  - env loading
  - interpolation
  - HTTP execution
  - GraphQL conversion
  - output formatting
  - response persistence
```

Avoid this approach for now:

```text
Trying to build a full IntelliJ/Altair-style custom UI panel inside Zed.
```

The first version should be reliable, scriptable, and testable. A richer UI can come later.
