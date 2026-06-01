# Repository Guidelines

## Project Structure & Module Organization

This is a Rust Cargo workspace for a Zed HTTP Client extension and companion CLI. Workspace crates live under `crates/`: `zed-http-core` contains parsing, environment loading, interpolation, validation, formatting, importers, and shared models; `zed-http-cli` contains the `zed-http` binary and black-box CLI tests. The Zed extension is self-contained under `extension/` (`extension.toml`, `languages/http-request/`, `snippets/`, and `LICENSE`), kept separate from the repo root so the extension directory has no `Cargo.toml` (Zed would otherwise try to compile the root CLI workspace as a WASM extension). Examples are in `examples/`, and Antora/AsciiDoc documentation in `src/docs/`. Product notes and historical PRDs live in `docs/prd/`.

## Build, Test, and Development Commands

- `cargo build --workspace --locked`: build all crates using the committed lockfile.
- `cargo run -p zed-http-cli -- --help`: run the local CLI.
- `cargo run -p zed-http-cli -- run --file examples/requests.http --line 4 --worktree .`: execute one example request.
- `cargo test --workspace --locked`: run the full test suite.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: run lints with CI-equivalent strictness.
- `cargo check --workspace --locked`: quick compile check; use Rust `1.74` when validating MSRV.

## Coding Style & Naming Conventions

Use Rust 2021 and preserve the declared MSRV of `1.74`. Format with `rustfmt`; do not hand-align code in ways `cargo fmt` will undo. Keep the CLI thin and place reusable behavior in `zed-http-core`. Update the module map in `crates/zed-http-core/src/lib.rs` when adding or renaming core modules. Use descriptive snake_case for Rust functions, modules, and tests. Keep crate versions and `extension.toml` version in sync for releases.

## Testing Guidelines

Most core tests are colocated in `#[cfg(test)]` modules next to the implementation. CLI integration tests live in `crates/zed-http-cli/tests/cli.rs` and use the compiled `zed-http` binary. Prefer offline tests; when HTTP behavior is required, follow the existing std-only localhost server pattern. For filesystem fixtures, use isolated directories under `std::env::temp_dir()` with a timestamp or nanos suffix.

## Commit & Pull Request Guidelines

Recent commits use short imperative subjects, often with a scope such as `docs:`, `CLI:`, or `HAR import:`. Keep subjects specific, for example `CLI: add validation exit code test`. Pull requests should describe behavior changes, list validation commands run, link related issues, and include screenshots only for visible Zed/editor UI changes.

## Security & Configuration Tips

Do not commit private environment files; use `examples/http-client.private.env.json.example` as the template. Treat `.zed-http/` outputs, cookies, schemas, and response captures as local artifacts unless intentionally documenting examples.
