//! Public CLI replay for Native 2.1.295 terminal provider-error results.
//!
//! The subprocess uses one loopback-only Anthropic Messages stub and checks the
//! actual `--print --output-format json` entrypoint. It compares the raw JSON
//! result string literal and fixed top-level value slices with the Native
//! capture. The generated duration and UUID remain untouched and have separate
//! format and relationship constraints; the complete stdout is not byte-equal.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const NATIVE_VERSION: &str = "2.1.295 (Claude Code)";
const NATIVE_SHA256: &str = "0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70";
const API_KEY: &str = "native-provider-error-295-dummy";
const MODEL: &str = "claude-sonnet-4-5";
const PROMPT: &str = "Return the fixed provider error fixture response.";
const ENTRYPOINT: &str = "--print --output-format json";

// These integration cases launch cold CLI subprocesses and bind independent
// loopback listeners. Serialize them so their startup and request-body writes
// do not compete against the stub's bounded socket read deadline.
static PROVIDER_ERROR_CLI_TEST_LOCK: Mutex<()> = Mutex::new(());

fn provider_error_cli_test_lock() -> MutexGuard<'static, ()> {
    PROVIDER_ERROR_CLI_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Deserialize)]
struct Native295Receipt {
    native_version: String,
    native_sha256: String,
    cases: Vec<Native295Case>,
}

#[derive(Deserialize)]
struct Native295Case {
    #[serde(rename = "case")]
    id: String,
    session_id: String,
    status: u16,
    exit_code: i32,
    requests: Vec<Native295Request>,
    stdout_sha256: String,
    response_sha256: String,
    no_retry_header: bool,
    terminal_reason: String,
    stop_reason: String,
}

#[derive(Deserialize)]
struct Native295Request {
    model: String,
    stream: bool,
    body_sha256: String,
    body_bytes: u64,
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
        let read = stream.read(&mut chunk).map_err(|error| {
            let framing = find_bytes(&request, b"\r\n\r\n")
                .map(|header_end| {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = request_header_value(&headers, "content-length");
                    let expected_body_bytes = content_length
                        .and_then(|value| value.parse::<usize>().ok());
                    format!(
                        "; request_line={:?}; content_length={content_length:?}; received_body_bytes={}; expected_body_bytes={expected_body_bytes:?}; transfer_encoding={:?}; expect={:?}",
                        headers.lines().next().unwrap_or_default(),
                        request.len().saturating_sub(header_end + 4),
                        request_header_value(&headers, "transfer-encoding"),
                        request_header_value(&headers, "expect"),
                    )
                })
                .unwrap_or_else(|| "; request headers are incomplete".to_string());
            std::io::Error::new(
                error.kind(),
                format!(
                    "{error}; received {} request bytes{framing}",
                    request.len()
                ),
            )
        })?;
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
    write!(stream, "Content-Type: {content_type}\r\n")?;
    write!(stream, "\r\n")?;
    stream.write_all(body)?;
    stream.flush()
}

fn invoke_cli(
    case: &Native295Case,
    response_body: &[u8],
    entrypoint: &str,
    no_stream: bool,
) -> (std::process::Output, Vec<Vec<u8>>) {
    let config = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let stub = LocalApiStub::start(
        case.status,
        response_body.to_vec(),
        "application/json".into(),
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
        .env("ANTHROPIC_MAX_RETRIES", "0")
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
        .args(["--setting-sources", "project", "--strict-mcp-config"]);
    command
        .args(entrypoint.split_whitespace())
        .args(["--tools", ""])
        .args([
            "--max-budget-usd",
            "1",
            "--model",
            MODEL,
            "--session-id",
            case.session_id.as_str(),
        ]);
    if no_stream {
        command.arg("--no-stream");
    }
    command
        .arg(PROMPT)
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

fn is_json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn skip_json_whitespace(raw: &[u8], mut index: usize) -> usize {
    while raw.get(index).is_some_and(|byte| is_json_whitespace(*byte)) {
        index += 1;
    }
    index
}

fn json_string_end(raw: &[u8], start: usize) -> usize {
    assert_eq!(
        raw.get(start),
        Some(&b'"'),
        "JSON string starts with a quote"
    );
    let mut index = start + 1;
    while let Some(byte) = raw.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    panic!("JSON string is terminated");
}

fn json_value_end(raw: &[u8], start: usize) -> usize {
    match raw.get(start).copied().expect("JSON value exists") {
        b'"' => json_string_end(raw, start),
        opening @ (b'{' | b'[') => {
            let mut closers = vec![if opening == b'{' { b'}' } else { b']' }];
            let mut index = start + 1;
            let mut in_string = false;
            while let Some(byte) = raw.get(index).copied() {
                if in_string {
                    match byte {
                        b'\\' => index += 2,
                        b'"' => {
                            in_string = false;
                            index += 1;
                        }
                        _ => index += 1,
                    }
                    continue;
                }
                match byte {
                    b'"' => in_string = true,
                    b'{' => closers.push(b'}'),
                    b'[' => closers.push(b']'),
                    b'}' | b']' => {
                        assert_eq!(closers.pop(), Some(byte), "JSON containers are balanced");
                        if closers.is_empty() {
                            return index + 1;
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
            panic!("JSON container is terminated");
        }
        _ => {
            let mut index = start;
            while raw.get(index).is_some_and(|byte| {
                !matches!(*byte, b',' | b'}' | b']') && !is_json_whitespace(*byte)
            }) {
                index += 1;
            }
            assert!(index > start, "JSON scalar is nonempty");
            index
        }
    }
}

#[derive(Debug)]
struct RawTopLevelField<'a> {
    key: &'a [u8],
    value: &'a [u8],
    field: &'a [u8],
}

fn raw_top_level_fields(raw: &[u8]) -> Vec<RawTopLevelField<'_>> {
    let mut index = skip_json_whitespace(raw, 0);
    assert_eq!(raw.get(index), Some(&b'{'), "stdout is a JSON object");
    index += 1;
    let mut fields = Vec::new();
    loop {
        index = skip_json_whitespace(raw, index);
        if raw.get(index) == Some(&b'}') {
            index += 1;
            break;
        }
        let key_start = index;
        let key_end = json_string_end(raw, key_start);
        index = skip_json_whitespace(raw, key_end);
        assert_eq!(raw.get(index), Some(&b':'), "JSON object field has a colon");
        let value_start = skip_json_whitespace(raw, index + 1);
        let value_end = json_value_end(raw, value_start);
        fields.push(RawTopLevelField {
            key: &raw[key_start..key_end],
            value: &raw[value_start..value_end],
            field: &raw[key_start..value_end],
        });
        index = skip_json_whitespace(raw, value_end);
        match raw.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => {
                index += 1;
                break;
            }
            _ => panic!("JSON object ends after each field"),
        }
    }
    assert!(
        raw[index..].iter().all(|byte| is_json_whitespace(*byte)),
        "stdout contains no bytes after its JSON object"
    );
    let mut keys = std::collections::HashSet::new();
    assert!(
        fields.iter().all(|field| keys.insert(field.key)),
        "top-level JSON field names are unique"
    );
    fields
}

fn raw_top_level_value<'a>(raw: &'a [u8], key: &[u8]) -> &'a [u8] {
    raw_top_level_fields(raw)
        .into_iter()
        .find(|field| field.key == key)
        .unwrap_or_else(|| panic!("top-level field {} exists", String::from_utf8_lossy(key)))
        .value
}

fn raw_unsigned_integer(raw: &[u8], key: &[u8]) -> u64 {
    let value = raw_top_level_value(raw, key);
    assert!(
        !value.is_empty() && value.iter().all(u8::is_ascii_digit),
        "{} is encoded as unsigned integer bytes",
        String::from_utf8_lossy(key)
    );
    std::str::from_utf8(value)
        .expect("unsigned integer bytes are ASCII")
        .parse::<u64>()
        .expect("unsigned integer fits u64")
}

fn assert_canonical_uuid_v4(value: &str, field: &str) {
    let parsed = uuid::Uuid::parse_str(value).expect("generated UUID is valid");
    assert_eq!(
        parsed.to_string(),
        value,
        "{field} uses canonical lowercase UUID format"
    );
    assert_eq!(value.as_bytes().get(14), Some(&b'4'), "{field} is UUID v4");
    let variant = value.as_bytes().get(19).copied().unwrap_or_default();
    assert!(
        matches!(variant, b'8' | b'9' | b'a' | b'b'),
        "{field} uses the RFC 4122 variant"
    );
}

fn assert_generated_field_constraints(
    raw: &[u8],
    frame: &Utf16JsonProjection,
    session_id: &str,
    case_id: &str,
) {
    let duration_ms = raw_unsigned_integer(raw, b"\"duration_ms\"");
    assert_eq!(
        frame.number_at("/duration_ms"),
        Some(duration_ms as f64),
        "case {case_id} duration projection matches its untouched raw integer"
    );
    let uuid_value = frame.string_at("/uuid").expect("result UUID exists");
    assert_canonical_uuid_v4(uuid_value, "result uuid");
    assert_ne!(
        uuid_value, session_id,
        "case {case_id} result UUID is distinct from its fixed session identity"
    );
}

fn assert_native_fixed_field_slices(actual: &[u8], native: &[u8], case_id: &str) {
    let actual_fields = raw_top_level_fields(actual);
    let native_fields = raw_top_level_fields(native);
    let actual_keys: Vec<_> = actual_fields.iter().map(|field| field.key).collect();
    let native_keys: Vec<_> = native_fields.iter().map(|field| field.key).collect();
    assert_eq!(
        actual_keys, native_keys,
        "case {case_id} top-level key order"
    );

    // These generated fields stay untouched in both buffers and are constrained
    // separately. All remaining raw top-level field slices, including result,
    // are compared literally; the complete stdout is not claimed byte-equal.
    for (actual_field, native_field) in actual_fields.iter().zip(&native_fields) {
        if actual_field.key == b"\"duration_ms\"" || actual_field.key == b"\"uuid\"" {
            continue;
        }
        assert_eq!(
            actual_field.field,
            native_field.field,
            "case {case_id} raw JSON field bytes for {}",
            String::from_utf8_lossy(actual_field.key)
        );
    }
}

fn native_receipt() -> Native295Receipt {
    serde_json::from_str(include_str!(
        "fixtures/provider-error-native-295/receipt.json"
    ))
    .expect("Native 2.1.295 receipt parses")
}

fn native_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-error-native-295")
}

fn selected_native_cases(receipt: &Native295Receipt) -> Vec<&Native295Case> {
    const CASE_IDS: [&str; 4] = [
        "nested-error-message-lone-surrogate",
        "empty-response-body",
        "nested-error-message-null",
        "http-413",
    ];
    let selected: Vec<_> = receipt
        .cases
        .iter()
        .filter(|case| CASE_IDS.contains(&case.id.as_str()))
        .collect();
    assert_eq!(
        selected.len(),
        CASE_IDS.len(),
        "all selected 2.1.295 cases exist"
    );
    assert_eq!(
        selected
            .iter()
            .map(|case| case.id.as_str())
            .collect::<Vec<_>>(),
        CASE_IDS.to_vec()
    );
    assert_eq!(
        selected.iter().map(|case| case.status).collect::<Vec<_>>(),
        [400, 400, 400, 413]
    );
    selected
}

#[test]
fn print_json_provider_errors_match_native_295_raw_result_and_fixed_fields() {
    let _test_lock = provider_error_cli_test_lock();
    let receipt = native_receipt();
    assert_eq!(receipt.native_version, NATIVE_VERSION);
    assert_eq!(receipt.native_sha256, NATIVE_SHA256);
    let fixture_root = native_fixture_root();

    for case in selected_native_cases(&receipt) {
        assert_eq!(case.exit_code, 1, "Native exit status for {}", case.id);
        assert!(!case.no_retry_header, "selected cases use max retries zero");
        assert_eq!(
            case.requests.len(),
            1,
            "Native request count for {}",
            case.id
        );
        let native_request = &case.requests[0];
        assert_eq!(native_request.model, MODEL, "Native model for {}", case.id);
        assert!(
            native_request.stream,
            "Native request streams for {}",
            case.id
        );

        let response_body = fixture_file(
            &fixture_root,
            &format!("{}/http-response.body.raw", case.id),
        );
        assert_eq!(
            sha256_hex(&response_body),
            case.response_sha256,
            "Native response fixture hash for {}",
            case.id
        );
        let captured_request = fixture_file(
            &fixture_root,
            &format!("{}/request-0.body.raw.json", case.id),
        );
        assert_eq!(
            sha256_hex(&captured_request),
            native_request.body_sha256,
            "Native request fixture hash for {}",
            case.id
        );
        assert_eq!(
            captured_request.len() as u64,
            native_request.body_bytes,
            "Native request fixture length for {}",
            case.id
        );
        let native_stdout = fixture_file(&fixture_root, &format!("{}/native.stdout.raw", case.id));
        assert_eq!(
            sha256_hex(&native_stdout),
            case.stdout_sha256,
            "Native stdout fixture hash for {}",
            case.id
        );
        let native_stderr = fixture_file(&fixture_root, &format!("{}/native.stderr.raw", case.id));
        assert_eq!(
            sha256_hex(&native_stderr),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "Native stderr fixture hash for {}",
            case.id
        );
        let native_stdout_text = std::str::from_utf8(&native_stdout).unwrap();
        assert_eq!(
            native_stdout_text.lines().count(),
            1,
            "Native result frame count"
        );
        let native_frame = Utf16JsonProjection::parse(native_stdout_text.trim_end()).unwrap();
        assert_eq!(native_frame.string_at("/type"), Some("result"));
        assert_eq!(native_frame.string_at("/subtype"), Some("success"));
        assert_eq!(
            native_frame
                .at_pointer("/is_error")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            Some(raw_unsigned_integer(
                &native_stdout,
                b"\"api_error_status\""
            )),
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
        assert_eq!(
            Some(raw_unsigned_integer(&native_stdout, b"\"num_turns\"")),
            Some(1)
        );
        assert_eq!(
            Some(raw_unsigned_integer(&native_stdout, b"\"duration_api_ms\"")),
            Some(0)
        );
        assert_generated_field_constraints(
            &native_stdout,
            &native_frame,
            &case.session_id,
            &case.id,
        );
        if case.id == "nested-error-message-lone-surrogate" {
            assert!(native_frame
                .string_units("/result")
                .unwrap()
                .contains(&0xd800));
            assert!(native_stdout
                .windows(b"\\ud800".len())
                .any(|window| window == b"\\ud800"));
        }

        let (output, requests) = invoke_cli(case, &response_body, ENTRYPOINT, false);
        assert_eq!(
            output.status.code(),
            Some(case.exit_code),
            "case {}",
            case.id
        );
        assert_eq!(
            output.stderr, native_stderr,
            "case {} stderr bytes",
            case.id
        );
        assert_eq!(requests.len(), case.requests.len(), "case {}", case.id);
        let actual_request = request_body_json(&requests[0]);
        assert_eq!(
            actual_request.string_at("/model"),
            Some(native_request.model.as_str()),
            "case {} model",
            case.id
        );
        assert_eq!(
            actual_request
                .at_pointer("/stream")
                .and_then(|value| value.as_bool()),
            Some(native_request.stream),
            "default print-json request must use Native streaming transport: {}",
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
            request.contains(MODEL),
            "case {} missed the captured model",
            case.id
        );

        let actual_stdout_text = std::str::from_utf8(&output.stdout).unwrap();
        assert_eq!(
            actual_stdout_text.lines().count(),
            1,
            "case {} emitted extra result frames",
            case.id
        );
        let actual_frame = Utf16JsonProjection::parse(actual_stdout_text.trim_end()).unwrap();
        assert_eq!(actual_frame.string_at("/type"), Some("result"));
        assert_eq!(actual_frame.string_at("/subtype"), Some("success"));
        assert_eq!(
            actual_frame
                .at_pointer("/is_error")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            Some(raw_unsigned_integer(
                &output.stdout,
                b"\"api_error_status\""
            )),
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
            actual_frame.string_at("/session_id"),
            Some(case.session_id.as_str())
        );
        assert_eq!(
            Some(raw_unsigned_integer(&output.stdout, b"\"num_turns\"")),
            Some(1)
        );
        assert_eq!(
            Some(raw_unsigned_integer(&output.stdout, b"\"duration_api_ms\"")),
            Some(0)
        );
        assert_eq!(
            actual_frame.string_units("/result"),
            native_frame.string_units("/result"),
            "case {} result UTF-16 units",
            case.id
        );
        assert_generated_field_constraints(
            &output.stdout,
            &actual_frame,
            &case.session_id,
            &case.id,
        );

        assert_eq!(
            raw_top_level_value(&output.stdout, b"\"result\""),
            raw_top_level_value(&native_stdout, b"\"result\""),
            "case {} raw JSON result string literal bytes",
            case.id
        );
        assert_native_fixed_field_slices(&output.stdout, &native_stdout, &case.id);
    }
}

#[test]
fn explicit_no_stream_uses_batched_messages_create_with_native_295_response() {
    let _test_lock = provider_error_cli_test_lock();
    let receipt = native_receipt();
    let case = selected_native_cases(&receipt)
        .into_iter()
        .find(|case| case.status == 400)
        .expect("fixture contains a 400 provider error");
    let fixture_root = native_fixture_root();
    let response_body = fixture_file(
        &fixture_root,
        &format!("{}/http-response.body.raw", case.id),
    );

    let (output, requests) = invoke_cli(case, &response_body, ENTRYPOINT, true);

    assert_eq!(output.status.code(), Some(case.exit_code));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_body_json(&requests[0])
            .at_pointer("/stream")
            .and_then(|value| value.as_bool()),
        Some(false),
        "--no-stream must preserve the batched messages_create path"
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("CLI stdout is UTF-8");
    let result = Utf16JsonProjection::parse(stdout.trim_end()).expect("one JSON result frame");
    assert_eq!(result.string_at("/type"), Some("result"));
    assert_eq!(
        result
            .at_pointer("/is_error")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
}

#[test]
fn default_print_text_uses_streaming_provider_request_with_native_295_response() {
    let _test_lock = provider_error_cli_test_lock();
    let receipt = native_receipt();
    let case = selected_native_cases(&receipt)
        .into_iter()
        .find(|case| case.status == 400)
        .expect("fixture contains a 400 provider error");
    let fixture_root = native_fixture_root();
    let response_body = fixture_file(
        &fixture_root,
        &format!("{}/http-response.body.raw", case.id),
    );

    let (output, requests) = invoke_cli(
        case,
        &response_body,
        "--print --no-session-persistence",
        false,
    );

    assert_eq!(output.status.code(), Some(case.exit_code));
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_body_json(&requests[0])
            .at_pointer("/stream")
            .and_then(|value| value.as_bool()),
        Some(true),
        "default text print must use streaming provider transport"
    );
}
