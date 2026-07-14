//! Core library for the `zed-http` CLI and the companion Zed extension.
//!
//! The crate is intentionally CLI-agnostic: it parses IntelliJ-style `.http`
//! files, resolves `http-client.env.json` overlays, applies `{{var}}`
//! interpolation, validates GraphQL operations, formats the file back out
//! canonically, and imports Postman collections. Everything that actually
//! sends an HTTP request lives in `zed-http-cli`; this crate stops at
//! producing a fully resolved [`executor::ResolvedRequest`].
//!
//! Module map:
//!
//! - [`parser`] — tokenises a `.http` file into a [`model::RequestFile`].
//! - [`model`] — the typed AST used by every other module.
//! - [`env`] — discovers and overlays `http-client.env.json` files.
//! - [`interpolate`] — substitutes `{{var}}` references; supports nested vars.
//! - [`executor`] — glues parse → env lookup → interpolation → GraphQL render
//!   into a [`executor::ResolvedRequest`]. The CLI's `run` command takes it
//!   from there.
//! - [`graphql`] — splits a GraphQL body into `{query, variables,
//!   operationName}`, plus the introspection query bundled with the binary.
//! - [`schema`] — locates a cached introspection schema for a given request
//!   URL and reports unknown top-level field selections.
//! - [`validate`] — pre-flight checks before a request fires (variable
//!   completeness, schema-aware field validation when a cache exists).
//! - [`assertion`] — `# @expect-*` directives evaluated against a response.
//! - [`capture`] — `# @capture` directives that lift JSON-pointer / header
//!   / status values out of a response into variables, so later requests
//!   in the same `run-all` invocation can reference them as `{{name}}`.
//! - [`dynamic`] — IntelliJ-compatible `$`-prefixed variables (`$uuid`,
//!   `$timestamp`, `$isoTimestamp`, `$randomInt`) generated fresh per
//!   request and overridable by user-defined variables of the same name.
//! - [`format`] — re-emits a parsed file in canonical layout, used by
//!   `zed-http format` and by the importers. `format_request_file_checked`
//!   re-parses its own output first, so importer data that the format cannot
//!   express fails loudly instead of writing a corrupted file.
//! - [`output`] — pretty-printers, response persistence under `.zed-http/`,
//!   and the conventional paths for the response/schema/cookie artefact
//!   directories.
//! - [`postman`] — translates Postman v2.1 collection JSON into a
//!   [`model::RequestFile`] for use with the formatter.
//! - [`curl`] — translates a `curl` command (the "Copy as cURL" shape
//!   from browser devtools) into a single-request [`model::RequestFile`].
//! - [`har`] — translates an HTTP Archive (HAR 1.2) export into a
//!   multi-request [`model::RequestFile`]; the bulk equivalent of the
//!   curl importer.
//! - [`error`] — single [`error::HttpClientError`] type returned everywhere.

pub mod assertion;
pub mod capture;
pub mod curl;
pub mod dynamic;
pub mod env;
pub mod error;
pub mod executor;
pub mod format;
pub mod graphql;
pub mod har;
pub mod interpolate;
pub mod model;
pub mod output;
pub mod parser;
pub mod postman;
pub mod schema;
pub mod validate;

pub use assertion::{evaluate_assertions, AssertionFailure, AssertionResponse};
pub use capture::{evaluate_captures, CaptureOutcome, CaptureWarning};
pub use curl::import_curl;
pub use dynamic::build_dynamic_variables;
pub use env::{
    is_sensitive_key, list_environments, load_environment, mask_value, mask_variables,
    redact_secrets, VariableMap,
};
pub use error::HttpClientError;
pub use executor::{
    parse_and_select_request, prepare_request, prepare_request_with_extras, RequestSelector,
    ResolvedRequest,
};
pub use format::{format_request_file, format_request_file_checked};
pub use graphql::{
    build_graphql_payload, introspection_payload, parse_variable_definitions, render_graphql_json,
    render_graphql_json_with_extras, validate_variables, VariableDefinition, INTROSPECTION_QUERY,
};
pub use har::{decode_har_input, import_har};
pub use interpolate::{interpolate_text, resolve_variables};
pub use model::{
    CaptureDirective, CaptureSource, Header, InPlaceVariable, RequestBlock, RequestBody,
    RequestFile, RequestMethod, RequestOptions, ResponseAssertion, ResponseRedirect, SourceRange,
};
pub use output::{
    build_preview, cookie_jar_path, format_pretty_response, response_root, save_response,
    schema_root, ResponseSummary,
};
pub use parser::{parse_request_file, select_request_by_line, select_request_by_name};
pub use postman::import_collection as import_postman_collection;
pub use schema::{
    cached_schema_path, detect_operation_kind, load_cached_schema, schema_slug,
    validate_against_schema,
};
pub use validate::{
    validate_request, validate_request_file, validate_request_file_with_schemas,
    validate_request_with_schema, ValidationIssue,
};
