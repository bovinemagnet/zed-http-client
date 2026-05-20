# Initial PRD Overview

This is feasible, but I would **not** try to build it as a “pure Zed UI plugin” first. The current Zed extension surface is strongest for **language support, Tree-sitter syntax/runnables, language servers, debuggers, snippets, themes, MCP/agent integrations**, not full arbitrary IntelliJ-style tool windows. Zed tasks already run commands and show output through the integrated terminal, and task variables expose the current file, line, column, selection, language, and worktree. That gives you a solid path for “run request under cursor” with an output panel-like experience. ([Zed][1])

My recommended architecture is:

```text
zed-http-client/
  extension/                 # Zed extension: syntax, run buttons, task wiring
  cli/                       # Companion executable: parser + executor
  crates/parser/             # IntelliJ .http parser
  crates/env/                # http-client.env.json compatibility
  crates/executor/           # HTTP/GraphQL execution
  crates/output/             # pretty response rendering
```

## 1. Build it as a Zed language extension + companion CLI

The Zed extension should provide:

```text
.http / .rest language support
syntax highlighting
GraphQL body injection/highlighting
runnable markers next to each request
task integration to run the current request
optional snippets for GET/POST/GRAPHQL
```

The companion CLI should do the real work:

```bash
zed-http run --file requests.http --line 42 --env dev
```

The reason I’d split it this way: Zed extensions are Rust/WASM-based, but request execution, TLS config, cookies, streaming, file IO, env-file lookup, and future OAuth support are much easier and more portable in a normal native executable. Zed’s own docs show extensions with `extension.toml`, optional Rust/WASM code, and language folders, and the extension API is centred around language servers, slash commands, context servers, and debug adapter integration rather than arbitrary custom panes. ([Zed][1])

## 2. Use IntelliJ HTTP Client syntax as your file format

IntelliJ’s format is a good choice because it is already familiar and text-based:

```http
### List users
GET {{host}}/api/users
Authorization: Bearer {{token}}

### Create user
POST {{host}}/api/users
Content-Type: application/json

{
  "name": "Alice"
}

### GraphQL query
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

JetBrains documents the core syntax as:

```text
Method Request-URI HTTP-Version
Header-field: Header-value

Request-Body
```

and also supports the `GRAPHQL` keyword followed by a server address, with the GraphQL operation in the request body and an optional JSON variables block after it. ([JetBrains][2])

For v1, I would support this compatibility set:

| Feature                                                    | Support in v1? | Notes                                                            |
| ---------------------------------------------------------- | -------------: | ---------------------------------------------------------------- |
| `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS` |            Yes | Use normal HTTP method parsing.                                  |
| `GRAPHQL url`                                              |            Yes | Treat as GraphQL-over-HTTP, usually `POST`.                      |
| Headers                                                    |            Yes | Including variable interpolation.                                |
| JSON body                                                  |            Yes | Pretty-print request/response.                                   |
| GraphQL variables JSON block                               |            Yes | Parse operation + JSON block.                                    |
| `http-client.env.json`                                     |            Yes | Required for compatibility.                                      |
| `http-client.private.env.json`                             |            Yes | Private overrides public.                                        |
| In-place variables, `@name = value`                        |            Yes | Useful and easy.                                                 |
| Request names after `###`                                  |            Yes | Useful for task labels/history.                                  |
| Body from file using `< ./file.json`                       |            Yes | JetBrains supports it.                                           |
| `@no-redirect`, `@timeout`, `@connection-timeout`          |        Yes-ish | Good early additions.                                            |
| Multipart                                                  |          Later | Parser/executor complexity.                                      |
| Cookies jar                                                |          Later | Needs policy decisions.                                          |
| JS pre-request/response handlers                           |          Later | Requires embedded JS runtime.                                    |
| OAuth2 config                                              |          Later | Even JetBrains CLI documents OAuth2 as unsupported.              |
| Client cert config                                         |          Later | JetBrains CLI currently documents SSL config as unsupported too. |

JetBrains’ environment file behaviour is important: `http-client.env.json` is intended for shared/public values, `http-client.private.env.json` is for secrets, and private values override public ones. It also searches current/parent directories for environment files related to the current `.http` file. ([JetBrains][3])

## 3. Zed extension structure

A starting structure:

```text
zed-http-client/
  extension.toml
  Cargo.toml
  src/
    lib.rs
  languages/
    http-request/
      config.toml
      highlights.scm
      injections.scm
      runnables.scm
  snippets/
    http-request.json
```

Example `extension.toml`:

```toml
id = "http-client"
name = "HTTP Client"
version = "0.1.0"
schema_version = 1
authors = ["Paul Snow <bovinemagnet@gmail.com>"]
description = "Run IntelliJ-compatible HTTP and GraphQL request files from Zed"
repository = "https://github.com/YOUR_USER/zed-http-client"

[grammars.http-request]
repository = "https://github.com/YOUR_USER/tree-sitter-http-request"
rev = "PINNED_COMMIT_SHA"
```

Example `languages/http-request/config.toml`:

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

Zed language extensions use Tree-sitter; Zed’s docs explicitly call out Tree-sitter queries for syntax highlighting, bracket matching, code outline, code injections, text redactions, and runnable code detection. ([Zed][4])

## 4. Runnable request execution

The key Zed feature you want is `runnables.scm`. It lets you mark request blocks as runnable and tag them. Zed then matches those tags to tasks. Zed documents `@run` captures and says additional captures are exposed as `ZED_CUSTOM_...` environment variables when running the code. ([Zed][4])

Illustrative `runnables.scm`, assuming your grammar exposes `request` and `request_line` nodes:

```scheme
(
  (request
    (request_line) @run)
  (#set! tag http-client-request)
)
```

Then a user or extension-provided task can call your CLI:

```json
[
  {
    "label": "HTTP Client: run request under cursor",
    "command": "zed-http",
    "args": [
      "run",
      "--file", "$ZED_FILE",
      "--line", "$ZED_ROW",
      "--column", "$ZED_COLUMN",
      "--worktree", "$ZED_WORKTREE_ROOT"
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

Zed tasks can use `$ZED_FILE`, `$ZED_ROW`, `$ZED_COLUMN`, `$ZED_WORKTREE_ROOT`, `$ZED_SELECTED_TEXT`, and other editor context variables, and task output goes through Zed’s integrated terminal. ([Zed][5])

For an output-window-like experience, print something structured:

```text
HTTP Client: List users
GET https://api.example.com/users

Status: 200 OK
Duration: 124 ms
Content-Type: application/json

Response saved:
.zed-http/responses/2026-05-20T14-32-10-list-users.response.json
```

Zed terminal output detects file paths and makes them clickable, so writing responses to files and printing the path gives users a decent workflow without needing a custom UI panel. ([Zed][6])

## 5. CLI command design

I’d make the CLI independent of Zed so it can also be used in CI:

```bash
zed-http run requests.http
zed-http run --file requests.http --line 42
zed-http run --file requests.http --name "List users"
zed-http run --file requests.http --env dev
zed-http run --file requests.http --env dev --verbose
zed-http list --file requests.http
zed-http envs --file requests.http
zed-http format --file requests.http
```

The internal model should look roughly like this:

```rust
struct RequestFile {
    variables: Vec<InPlaceVariable>,
    requests: Vec<RequestBlock>,
}

struct RequestBlock {
    name: Option<String>,
    range: SourceRange,
    kind: RequestKind,
    url: TemplateString,
    http_version: Option<String>,
    headers: Vec<Header>,
    body: Option<RequestBody>,
    options: RequestOptions,
    response_handler: Option<ResponseHandler>,
    response_redirect: Option<ResponseRedirect>,
}

enum RequestKind {
    Http { method: HttpMethod },
    GraphQl,
}

struct EffectiveEnvironment {
    name: Option<String>,
    values: Map<String, JsonValue>,
}
```

For GraphQL:

```rust
struct GraphQlBody {
    operation: String,
    variables: Option<serde_json::Value>,
    operation_name: Option<String>,
}
```

Execution rule for `GRAPHQL`:

```text
GRAPHQL {{host}}/graphql

query GetUser($id: ID!) {
  user(id: $id) { id name }
}

{ "id": "123" }
```

becomes:

```http
POST {{host}}/graphql
Content-Type: application/json
Accept: application/json

{
  "query": "...",
  "variables": { "id": "123" },
  "operationName": "GetUser"
}
```

That gives you Altair-like GraphQL execution while staying compatible with JetBrains’ textual format.

## 6. Env-file compatibility rules

Implement this exactly and document it clearly:

```text
1. Start at the directory containing the .http file.
2. Look for http-client.env.json and http-client.private.env.json.
3. If not found, walk up parent directories until the worktree root.
4. Load the selected environment from public file.
5. Overlay the same environment from private file.
6. Apply in-place variables from the .http file.
7. Apply CLI overrides, if any.
```

Example:

```json
// http-client.env.json
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

```json
// http-client.private.env.json
{
  "dev": {
    "token": "dev-secret"
  },
  "prod": {
    "token": "prod-secret"
  }
}
```

Then:

```http
GET {{host}}/api/users
Authorization: Bearer {{token}}
```

This should resolve with private `token` overriding public `token`, matching the JetBrains behaviour. ([JetBrains][3])

## 7. Do not try to clone all of Altair first

For a good v1, I would avoid building a full schema explorer. Instead:

1. Run GraphQL operations from `.http`.
2. Support variables.
3. Support introspection request generation.
4. Cache downloaded schema as `schema.graphql` or `.zed-http/schema.json`.
5. Add GraphQL syntax highlighting inside request bodies.
6. Later, add schema-aware completion through a GraphQL language server.

The bigger win is to make the text-file workflow excellent. Altair’s GUI is valuable, but in Zed the “native” feeling will come from run buttons, keyboard shortcuts, response files, and fast terminal output.

## 8. Parser strategy

I would use two parsers, not one giant regex system:

```text
Tree-sitter grammar:
  used by Zed for syntax highlighting, request boundaries, run buttons, injections

Rust parser in CLI:
  used for actual execution, validation, interpolation, and error reporting
```

Do not rely on the Tree-sitter parse tree for execution. Tree-sitter is excellent for editor features, but your CLI needs stable, well-tested semantic parsing with helpful diagnostics.

The CLI parser should be tolerant:

```text
- comments before requests
- request separators with ###
- request names
- blank lines
- headers split from body by blank line
- GraphQL body + optional variables JSON
- multi-line URLs
- body file references
- response redirect syntax
```

## 9. Output format

Have three output modes:

```bash
zed-http run --output pretty
zed-http run --output json
zed-http run --output raw
```

Pretty terminal output:

```text
▶ List users
GET http://localhost:8080/api/users

200 OK  87 ms
content-type: application/json

{
  "users": [
    {
      "id": 1,
      "name": "Alice"
    }
  ]
}
```

JSON output for tooling:

```json
{
  "requestName": "List users",
  "method": "GET",
  "url": "http://localhost:8080/api/users",
  "status": 200,
  "durationMillis": 87,
  "headers": {
    "content-type": ["application/json"]
  },
  "bodyFile": ".zed-http/responses/list-users.json"
}
```

This will make it much easier later to add tests, snapshot comparisons, or a future Zed-native viewer if the extension API expands.

## 10. Opinionated implementation choice

Given your Java/Spring background, Java 21 would be comfortable for the CLI, but I’d still choose **Rust for both the extension and CLI** here.

Reasons:

```text
- Zed extension code is Rust/WASM anyway.
- reqwest + rustls gives you good HTTP/TLS support.
- serde_json is ideal for env files and response rendering.
- clap gives clean CLI parsing.
- tree-sitter grammar tooling fits the Rust/Zed ecosystem.
- single static-ish binaries are easier to distribute than a Java CLI.
```

A Java CLI is reasonable if you want to reuse libraries or move quickly, but it introduces JDK/GraalVM/distribution friction. JetBrains’ own HTTP Client CLI requires JDK 21 for the ZIP distribution, which is fine for developers but less nice for a lightweight editor extension. ([JetBrains][7])

## 11. Suggested roadmap

### v0.1 — usable MVP

```text
- .http/.rest syntax highlighting
- request block detection
- run request under cursor
- GET/POST/PUT/PATCH/DELETE
- GRAPHQL
- http-client.env.json
- http-client.private.env.json
- in-place variables
- pretty terminal output
- response saved to .zed-http/responses/
```

### v0.2 — daily-driver quality

```text
- request names
- run by name
- list requests
- environment listing
- timeouts
- redirect control
- body from file
- response redirect to file
- better diagnostics with line/column
- GraphQL operationName detection
```

### v0.3 — GraphQL productivity

```text
- introspection command
- schema cache
- GraphQL validation
- fragments from nearby files
- GraphQL variables validation
- optional GraphQL LSP integration
```

### v0.4 — IntelliJ parity push

```text
- cookies jar
- multipart/form-data
- response assertions
- JS pre-request scripts
- JS response handlers
- import Postman collections
```

## 12. The honest limitation

The one feature I would adjust in your mental model is the “output window”. In Zed today, the practical output window is the **task terminal** or a generated response file opened in the editor. A custom rich response panel like IntelliJ/Altair is not the right first target unless you are willing to contribute to Zed core or wait for a broader UI extension API. The task-terminal approach is still useful and native enough: run button beside the request, output in terminal, response saved as JSON, clickable file link, rerun support. Zed tasks and terminals are explicitly designed for command execution and integrated output. ([Zed][5])

My strong recommendation: **ship the CLI first, then wrap it with the Zed language extension.** That gives you something testable, CI-friendly, and usable outside Zed, while the Zed extension adds the editor-native experience.

[1]: https://zed.dev/docs/extensions/developing-extensions "Developing Extensions | Developing Extensions"
[2]: https://www.jetbrains.com/help/idea/exploring-http-syntax.html "Exploring the HTTP request syntax | IntelliJ IDEA Documentation"
[3]: https://www.jetbrains.com/help/idea/http-client-variables.html "HTTP Client variables | IntelliJ IDEA Documentation"
[4]: https://zed.dev/docs/extensions/languages "Language Extensions | Language Extensions"
[5]: https://zed.dev/docs/tasks "Tasks | Tasks - Run Commands in Zed"
[6]: https://zed.dev/docs/terminal "Terminal | Built-in Terminal - Zed"
[7]: https://www.jetbrains.com/help/idea/http-client-cli.html "HTTP Client CLI | IntelliJ IDEA Documentation"
