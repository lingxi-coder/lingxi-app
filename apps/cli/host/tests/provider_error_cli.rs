//! Public CLI replay for Native 2.1.293 terminal provider-error results.
//!
//! The subprocess uses one loopback-only Anthropic Messages stub and checks the
//! actual `--print --output-format json` entrypoint, including the final result
//! envelope and UTF-16 source text. Dynamic timing/UUID fields are shape-checked
//! rather than compared to fixed Native values.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Deserialize;
use sha2::{Digest, Sha256};
const API_KEY: &str = "native-provider-error-fixture-dummy-key";

#[derive(Deserialize)]
struct NativeReceipt {
    native_version: String,
    native_binary_sha256: String,
    provenance: Vec<NativeProvenance>,
    capture: NativeCapture,
    dynamic_json_fields: Vec<String>,
    cases: Vec<NativeCase>,
}

#[derive(Deserialize)]
struct NativeProvenance {
    source: String,
    sha256: String,
}

#[derive(Deserialize)]
struct NativeCapture {
    entrypoint: String,
    model: String,
    prompt: String,
    setting_sources: String,
    strict_mcp_config: bool,
    disable_tools: bool,
    max_budget_usd: u64,
    response_content_type: String,
    network_boundary: String,
    response_header_behavior: String,
}

#[derive(Deserialize)]
struct NativeCase {
    id: String,
    session_id: String,
    #[serde(rename = "http_status")]
    status: u16,
    max_retries: Option<u32>,
    native_exit_code: i32,
    native_stderr_sha256: String,
    request_count: usize,
    terminal_reason: String,
    stop_reason: String,
    response_body_source: String,
    response_body_file: String,
    response_body_sha256: String,
    native_stdout_source: String,
    stdout_file: String,
    stdout_sha256: String,
}

struct LocalApiStub {
    address: SocketAddr,
    done: mpsc::Sender<()>,
    thread: thread::JoinHandle<Vec<Vec<u8>>>,
}

impl LocalApiStub {
    fn start(status: u16, response_body: Vec<u8>, content_type: String) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (done, done_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            let started = Instant::now();
            let mut finished = false;
            let mut finish_deadline = None;
            loop {
                if !finished && done_rx.try_recv().is_ok() {
                    finished = true;
                    finish_deadline = Some(Instant::now() + Duration::from_millis(250));
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let request = read_http_request(&mut stream).unwrap();
                        write_http_response(&mut stream, status, &response_body, &content_type)
                            .unwrap();
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if finish_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            break;
                        }
                        assert!(
                            started.elapsed() < Duration::from_secs(35),
                            "CLI did not finish its local provider request"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("loopback listener failed: {error}"),
                }
            }
            requests
        });
        Self {
            address,
            done,
            thread,
        }
    }

    fn finish(self) -> Vec<Vec<u8>> {
        self.done.send(()).unwrap();
        self.thread.join().unwrap()
    }
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = find_bytes(&request, b"\r\n\r\n") else {
            continue;
        };
        let header_text = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header_text
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let expected = header_end + 4 + content_length;
        if request.len() >= expected {
            request.truncate(expected);
            return Ok(request);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "connection closed before the HTTP request body completed",
    ))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn request_header_value<'a>(request: &'a str, target: &str) -> Option<&'a str> {
    request
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .map(|(_, value)| value.trim())
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    content_type: &str,
) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        413 => "Payload Too Large",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.0 {status} {reason}\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    if !body.is_empty() {
        write!(stream, "Content-Type: {content_type}\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(body)?;
    stream.flush()
}

fn invoke_cli(
    case: &NativeCase,
    capture: &NativeCapture,
    response_body: &[u8],
    entrypoint: &str,
    no_stream: bool,
) -> (std::process::Output, Vec<Vec<u8>>) {
    let config = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let max_budget_usd = capture.max_budget_usd.to_string();
    let stub = LocalApiStub::start(
        case.status,
        response_body.to_vec(),
        capture.response_content_type.clone(),
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"));
    command
        .current_dir(cwd.path())
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", config.path())
        .env("TMPDIR", config.path())
        .env("LANG", "C.UTF-8")
        .env("TZ", "UTC")
        .env("TERM", "dumb")
        .env("LINGXI_CONFIG_DIR", config.path())
        .env("LINGXI_API_BASE_URL", format!("http://{}", stub.address))
        // The loopback base is deliberately not the production Anthropic URL;
        // opt this test endpoint into the same source-backed Native request
        // policy the CLI uses for an explicitly configured custom endpoint.
        .env(
            "LINGXI_NATIVE_ANTHROPIC_ENDPOINT",
            format!("http://{}", stub.address),
        )
        .env("ANTHROPIC_API_KEY", API_KEY)
        .env("LINGXI_SIMPLE", "1")
        .env("CLAUDE_CONFIG_DIR", config.path())
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DO_NOT_TRACK", "1")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_USE_BEDROCK")
        .env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .env_remove("CLAUDE_CODE_USE_ANTHROPIC_AWS")
        .env_remove("CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD")
        .env_remove("CLAUDE_CODE_USE_MANTLE")
        .env_remove("CLAUDE_CODE_USE_VERTEX")
        .args(["--setting-sources", capture.setting_sources.as_str()]);
    if let Some(max_retries) = case.max_retries {
        command.env("ANTHROPIC_MAX_RETRIES", max_retries.to_string());
    }
    if capture.strict_mcp_config {
        command.arg("--strict-mcp-config");
    }
    command
        .args(entrypoint.split_whitespace())
        .args(if capture.disable_tools {
            vec!["--tools", ""]
        } else {
            Vec::new()
        })
        .args([
            "--max-budget-usd",
            max_budget_usd.as_str(),
            "--model",
            capture.model.as_str(),
            "--session-id",
            case.session_id.as_str(),
        ]);
    if no_stream {
        command.arg("--no-stream");
    }
    command
        .arg(capture.prompt.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("lingxi-cli timed out on local provider case {}", case.id);
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let requests = stub.finish();
    (output, requests)
}

fn fixture_file(root: &Path, name: &str) -> Vec<u8> {
    let path = root.join(name);
    assert!(path.starts_with(root), "fixture path escaped its directory");
    std::fs::read(path).unwrap()
}

fn request_body_json(request: &[u8]) -> Utf16JsonProjection {
    let header_end = find_bytes(request, b"\r\n\r\n").expect("HTTP request headers end");
    let body = std::str::from_utf8(&request[header_end + 4..]).expect("JSON request is UTF-8");
    Utf16JsonProjection::parse(body).expect("request body is JSON")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unique_field_span(raw: &[u8], marker: &[u8]) -> (usize, usize) {
    let marker_start = find_bytes(raw, marker).expect("dynamic field marker exists");
    assert!(
        find_bytes(&raw[marker_start + 1..], marker).is_none(),
        "dynamic field marker is unique"
    );
    let value_start = marker_start + marker.len();
    (marker_start, value_start)
}

fn mask_duration_ms(raw: &[u8]) -> Vec<u8> {
    let (_, mut value_start) = unique_field_span(raw, b"\"duration_ms\":");
    while raw[value_start].is_ascii_whitespace() {
        value_start += 1;
    }
    let mut value_end = value_start;
    while raw.get(value_end).is_some_and(u8::is_ascii_digit) {
        value_end += 1;
    }
    assert!(
        value_end > value_start,
        "duration_ms is an unsigned integer"
    );
    let value = std::str::from_utf8(&raw[value_start..value_end]).unwrap();
    value.parse::<u64>().expect("duration_ms is a u64");
    let mut masked = Vec::with_capacity(raw.len() + 1);
    masked.extend_from_slice(&raw[..value_start]);
    masked.push(b'0');
    masked.extend_from_slice(&raw[value_end..]);
    masked
}

fn mask_uuid(raw: &[u8]) -> Vec<u8> {
    let (_, value_start) = unique_field_span(raw, b"\"uuid\":\"");
    let value_end = value_start
        + raw[value_start..]
            .iter()
            .position(|byte| *byte == b'\"')
            .expect("uuid string closes");
    let uuid = std::str::from_utf8(&raw[value_start..value_end]).unwrap();
    assert!(uuid::Uuid::parse_str(uuid).is_ok(), "uuid is valid");
    let replacement = b"00000000-0000-4000-8000-000000000000";
    assert_eq!(raw[value_start..value_end].len(), replacement.len());
    let mut masked = raw.to_vec();
    masked[value_start..value_end].copy_from_slice(replacement);
    masked
}

fn mask_dynamic_native_fields(raw: &[u8]) -> Vec<u8> {
    let duration_masked = mask_duration_ms(raw);
    mask_uuid(&duration_masked)
}

#[test]
fn print_json_provider_errors_match_captured_native_293_stdout_bytes() {
    let receipt: NativeReceipt = serde_json::from_str(include_str!(
        "fixtures/provider-error-native-293/receipt.json"
    ))
    .unwrap();
    assert_eq!(receipt.native_version, "2.1.293");
    assert_eq!(
        receipt.native_binary_sha256,
        "4e21122a227857da1178aca3299700c1fd7f2b77c93f12e73c2c76db796a105e"
    );
    assert_eq!(receipt.provenance.len(), 2);
    assert_eq!(
        receipt.provenance[0].source,
        "harness-runtime/docs/parity/current-alignment/fixtures/provider-error/native-baseline/native-receipt.json"
    );
    assert_eq!(
        receipt.provenance[0].sha256,
        "bb39f4b76730a03e0ae868d2d3af8f3416bd28ad1b6a693d9e690a3796923e67"
    );
    assert_eq!(
        receipt.provenance[1].source,
        "harness-runtime/docs/parity/current-alignment/fixtures/provider-error/native-baseline/status-controls/receipt.json"
    );
    assert_eq!(
        receipt.provenance[1].sha256,
        "9b8f235b0100cfb3edbe65085460d3e49bebb16c296b9df7b96ee02c74b48ffe"
    );
    assert_eq!(
        receipt.dynamic_json_fields,
        vec!["/duration_ms".to_owned(), "/uuid".to_owned()]
    );
    assert!(receipt.capture.network_boundary.contains("127.0.0.1"));
    assert!(receipt.capture.network_boundary.contains("no upstream"));
    assert_eq!(
        receipt.capture.response_header_behavior,
        "HTTP/1.0; Content-Type is sent only for nonempty response bodies"
    );
    assert_eq!(receipt.cases.len(), 4, "keep all three 400 cases plus 413");
    assert_eq!(
        receipt
            .cases
            .iter()
            .map(|case| case.status)
            .collect::<Vec<_>>(),
        [400, 400, 400, 413]
    );

    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-error-native-293");
    for case in receipt.cases {
        assert!(case.native_stdout_source.starts_with("provider-error/"));
        let response_source = case
            .response_body_source
            .strip_prefix("provider-error/native-baseline/")
            .expect("response source is under the Native baseline");
        assert!(!response_source.split('/').any(|part| part == ".."));
        let response_body = fixture_file(&fixture_root, &case.response_body_file);
        assert_eq!(
            sha256_hex(&response_body),
            case.response_body_sha256,
            "Native response fixture hash for {}",
            case.id
        );
        let native_stdout = fixture_file(&fixture_root, &case.stdout_file);
        assert_eq!(
            sha256_hex(&native_stdout),
            case.stdout_sha256,
            "Native stdout fixture hash for {}",
            case.id
        );
        assert_eq!(
            case.native_exit_code, 1,
            "captured Native exit status for {}",
            case.id
        );
        let native_stdout_text = std::str::from_utf8(&native_stdout).unwrap();
        let native_frame = Utf16JsonProjection::parse(native_stdout_text.trim_end()).unwrap();
        assert_eq!(native_frame.string_at("/type"), Some("result"));
        assert_eq!(native_frame.string_at("/subtype"), Some("success"));
        assert_eq!(native_frame.bool_at("/is_error"), Some(true));
        assert_eq!(
            native_frame.unsigned_integer_at("/api_error_status"),
            Some(u64::from(case.status))
        );
        assert_eq!(
            native_frame.string_at("/terminal_reason"),
            Some(case.terminal_reason.as_str())
        );
        assert_eq!(
            native_frame.string_at("/stop_reason"),
            Some(case.stop_reason.as_str())
        );
        assert_eq!(
            native_frame.string_at("/session_id"),
            Some(case.session_id.as_str())
        );
        assert_eq!(native_frame.unsigned_integer_at("/num_turns"), Some(1));
        assert!(native_frame.unsigned_integer_at("/duration_ms").is_some());
        assert!(uuid::Uuid::parse_str(native_frame.string_at("/uuid").unwrap()).is_ok());
        if case.id == "nested-error-message-lone-surrogate" {
            assert!(native_frame
                .string_units("/result")
                .unwrap()
                .contains(&0xd800));
            assert!(native_stdout
                .windows(b"\\ud800".len())
                .any(|window| window == b"\\ud800"));
        }

        let (output, requests) = invoke_cli(
            &case,
            &receipt.capture,
            &response_body,
            &receipt.capture.entrypoint,
            false,
        );
        assert_eq!(
            output.status.code(),
            Some(case.native_exit_code),
            "case {}",
            case.id
        );
        assert_eq!(
            sha256_hex(&output.stderr),
            case.native_stderr_sha256,
            "case {} stderr bytes",
            case.id
        );
        assert_eq!(requests.len(), case.request_count, "case {}", case.id);
        assert_eq!(
            request_body_json(&requests[0]).bool_at("/stream"),
            Some(true),
            "default print-json request must use streaming transport: {}",
            case.id
        );
        let request = String::from_utf8_lossy(&requests[0]);
        assert_eq!(request_header_value(&request, "x-app"), Some("lingxi-cli"));
        assert_eq!(
            request_header_value(&request, "x-stainless-lang"),
            Some("rust")
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-runtime"),
            Some("lingxi")
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-package-version"),
            Some(lingxi_llm_client::PACKAGE_VERSION)
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-runtime-version"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            request_header_value(&request, "x-claude-code-session-id"),
            Some(case.session_id.as_str())
        );
        assert!(
            request.starts_with("POST /v1/messages?beta=true "),
            "case {}",
            case.id
        );
        assert!(
            request.contains(API_KEY),
            "case {} missed dummy auth",
            case.id
        );
        assert!(
            request.contains(receipt.capture.model.as_str()),
            "case {} missed the captured model",
            case.id
        );

        let actual_stdout_text = std::str::from_utf8(&output.stdout).unwrap();
        assert_eq!(
            actual_stdout_text.lines().count(),
            1,
            "case {} emitted extra frames",
            case.id
        );
        let actual_frame = Utf16JsonProjection::parse(actual_stdout_text.trim_end()).unwrap();
        assert_eq!(actual_frame.string_at("/type"), Some("result"));
        assert_eq!(actual_frame.string_at("/subtype"), Some("success"));
        assert_eq!(actual_frame.bool_at("/is_error"), Some(true));
        assert_eq!(
            actual_frame.unsigned_integer_at("/api_error_status"),
            Some(u64::from(case.status))
        );
        assert_eq!(
            actual_frame.string_at("/terminal_reason"),
            Some(case.terminal_reason.as_str())
        );
        assert_eq!(
            actual_frame.string_at("/stop_reason"),
            Some(case.stop_reason.as_str())
        );
        assert_eq!(
            actual_frame.string_units("/result"),
            native_frame.string_units("/result"),
            "case {} result UTF-16 units",
            case.id
        );
        assert!(actual_frame.unsigned_integer_at("/duration_ms").is_some());
        assert!(uuid::Uuid::parse_str(actual_frame.string_at("/uuid").unwrap()).is_ok());

        assert_eq!(
            mask_dynamic_native_fields(&output.stdout),
            mask_dynamic_native_fields(&native_stdout),
            "case {} differs from captured Native stdout outside /duration_ms and /uuid",
            case.id
        );
    }
}

#[test]
fn explicit_no_stream_uses_batched_messages_create() {
    let receipt: NativeReceipt = serde_json::from_str(include_str!(
        "fixtures/provider-error-native-293/receipt.json"
    ))
    .unwrap();
    let case = receipt
        .cases
        .iter()
        .find(|case| case.status == 400)
        .expect("fixture contains a 400 provider error");
    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-error-native-293");
    let response_body = fixture_file(&fixture_root, &case.response_body_file);

    let (output, requests) = invoke_cli(
        case,
        &receipt.capture,
        &response_body,
        &receipt.capture.entrypoint,
        true,
    );

    assert_eq!(output.status.code(), Some(case.native_exit_code));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_body_json(&requests[0]).bool_at("/stream"),
        Some(false),
        "--no-stream must preserve the batched messages_create path"
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("CLI stdout is UTF-8");
    let result = Utf16JsonProjection::parse(stdout.trim_end()).expect("one JSON result frame");
    assert_eq!(result.string_at("/type"), Some("result"));
    assert_eq!(result.bool_at("/is_error"), Some(true));
}

#[test]
fn default_print_text_uses_streaming_provider_request() {
    let receipt: NativeReceipt = serde_json::from_str(include_str!(
        "fixtures/provider-error-native-293/receipt.json"
    ))
    .unwrap();
    let case = receipt
        .cases
        .iter()
        .find(|case| case.status == 400)
        .expect("fixture contains a 400 provider error");
    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-error-native-293");
    let response_body = fixture_file(&fixture_root, &case.response_body_file);

    let (output, requests) = invoke_cli(
        case,
        &receipt.capture,
        &response_body,
        "--print --no-session-persistence",
        false,
    );

    assert_eq!(output.status.code(), Some(case.native_exit_code));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_body_json(&requests[0]).bool_at("/stream"),
        Some(true),
        "default text print must use streaming provider transport"
    );
}
