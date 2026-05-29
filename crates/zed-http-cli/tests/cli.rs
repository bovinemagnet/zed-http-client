//! Black-box integration tests for the `zed-http` binary.
//!
//! Each test shells out to the compiled binary via the `CARGO_BIN_EXE_*`
//! env var Cargo sets for integration tests, so no extra dev-dependency is
//! needed. Most subcommands exercised here are offline; request-sending
//! commands (`run`, `run-all`, `introspect`) are otherwise covered by the
//! core crate's unit tests. The one exception is the exit-code-2 test below,
//! which drives `run` against a std-only one-shot localhost server so the
//! CI-integration exit code is pinned end to end.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Output},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_zed-http")
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zed-http-cli-test-{tag}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn zed-http")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Spawn a one-shot localhost HTTP server that answers the next connection
/// with `status_line` (e.g. `"HTTP/1.1 200 OK"`) and a JSON `body`, then
/// exits. Std-only, so the no-extra-dev-dependency rule above still holds.
fn spawn_one_shot(
    status_line: &'static str,
    body: &'static str,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().expect("mock server addr");
    let url = format!("http://{addr}/");
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // drain the request headers
            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (url, handle)
}

#[test]
fn completions_generate_a_shell_script() {
    let output = run(&["completions", "bash"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("zed-http"));
}

#[test]
fn list_enumerates_requests_with_line_numbers() {
    let dir = temp_dir("list");
    let file = dir.join("requests.http");
    fs::write(
        &file,
        "### Health check\nGET https://example.com/health\n\n\
         ### Create user\nPOST https://example.com/users\n",
    )
    .unwrap();

    let output = run(&["list", "--file", file.to_str().unwrap()]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Health check"), "got: {text}");
    assert!(text.contains("Create user"), "got: {text}");
}

#[test]
fn envs_lists_environment_names() {
    let dir = temp_dir("envs");
    let file = dir.join("requests.http");
    fs::write(&file, "GET https://example.com\n").unwrap();
    fs::write(
        dir.join("http-client.env.json"),
        r#"{ "dev": {}, "prod": {} }"#,
    )
    .unwrap();

    let output = run(&["envs", "--file", file.to_str().unwrap()]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("dev"), "got: {text}");
    assert!(text.contains("prod"), "got: {text}");
}

#[test]
fn format_check_passes_on_canonical_output() {
    let dir = temp_dir("format-ok");
    let file = dir.join("requests.http");
    fs::write(&file, "###  Messy\nGET    https://example.com\n").unwrap();

    // The formatter's own stdout is canonical by definition.
    let canonical = run(&["format", "--file", file.to_str().unwrap()]);
    assert!(canonical.status.success());
    let canonical_file = dir.join("canonical.http");
    fs::write(&canonical_file, canonical.stdout).unwrap();

    let output = run(&[
        "format",
        "--file",
        canonical_file.to_str().unwrap(),
        "--check",
    ]);
    assert!(output.status.success());
}

#[test]
fn format_check_fails_on_non_canonical_file() {
    let dir = temp_dir("format-bad");
    let file = dir.join("requests.http");
    fs::write(&file, "###  Messy\nGET    https://example.com\n").unwrap();

    let output = run(&["format", "--file", file.to_str().unwrap(), "--check"]);

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn check_passes_on_a_valid_file() {
    let dir = temp_dir("check-ok");
    let file = dir.join("requests.http");
    fs::write(&file, "### Ping\nGET https://example.com/ping\n").unwrap();

    let output = run(&["check", "--file", file.to_str().unwrap()]);

    assert!(output.status.success());
}

#[test]
fn check_exits_3_on_validation_failure() {
    let dir = temp_dir("check-bad");
    let file = dir.join("requests.http");
    // A GRAPHQL operation declaring a required `$id` with an empty
    // variables block is a validation failure.
    fs::write(
        &file,
        "### GetUser\nGRAPHQL https://example.com/graphql\n\n\
         query GetUser($id: ID!) { user(id: $id) { id } }\n\n{}\n",
    )
    .unwrap();

    let output = run(&["check", "--file", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn import_curl_emits_a_request_block() {
    let output = run(&["import", "curl", "curl https://example.com/api"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("GET https://example.com/api"));
}

#[test]
fn run_exits_2_when_an_assertion_fails() {
    // The server answers 200 but the request asserts 500, so the response
    // assertion fails and `run` must exit with the test-failure code (2).
    let (url, server) = spawn_one_shot("HTTP/1.1 200 OK", "{}");
    let dir = temp_dir("run-assert-fail");
    let file = dir.join("requests.http");
    fs::write(
        &file,
        format!("### Health\n# @expect-status 500\nGET {url}\n"),
    )
    .unwrap();

    let output = run(&["run", "--file", file.to_str().unwrap(), "--no-cookies"]);
    server.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn malformed_var_override_is_a_generic_error() {
    let dir = temp_dir("var");
    let file = dir.join("requests.http");
    fs::write(&file, "GET https://example.com\n").unwrap();

    // `--var` without `=` is rejected before any request is sent.
    let output = run(&[
        "run",
        "--file",
        file.to_str().unwrap(),
        "--var",
        "noequalshere",
    ]);

    assert_eq!(output.status.code(), Some(1));
}
