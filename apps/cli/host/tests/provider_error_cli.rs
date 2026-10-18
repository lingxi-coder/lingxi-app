//! Public CLI replays for Native 2.1.295 provider errors, refusal projection,
//! and the first physical HTTP request in headless print-text mode.
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
use std::sync::{mpsc, Mutex, MutexGuard};
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
const NATIVE_REFUSAL_MODEL: &str = "claude-sonnet-4-5";
const NATIVE_REFUSAL_PROMPT: &str = "Return the fixed refusal stop-details fixture response.";
const NATIVE_REFUSAL_SAFETY_RETRY_USER_MESSAGE: &str = "Your response above was stopped by a safety classifier — this is not a tool or API error. The rest of it was withheld, and tool calls in it that had not finished did not run. Do not produce that content again, even reworded.";
const NATIVE_HTTP_LAYOUT_MODEL: &str = "claude-sonnet-5-5";
const NATIVE_HTTP_LAYOUT_SYSTEM_PROMPT: &str = "You are a local deterministic headless fixture.";
const NATIVE_HTTP_LAYOUT_PROMPT: &str = "Return HEADLESS_LOCAL_RESPONSE.";
const NATIVE_HTTP_LAYOUT_SESSION_ID: &str = "6b7ff52d-e492-4084-88e2-5ad153e3c053";
const PRODUCT_HTTP_LAYOUT_BRANDING: &[u8] =
    b"You are LingXi, an agentic command-line coding assistant.";
const NATIVE_HTTP_LAYOUT_BRANDING: &[u8] =
    b"You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

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
        Self::start_with_response(status, response_body, content_type, None)
    }

    fn start_raw_response(raw_response: Vec<u8>) -> Self {
        Self::start_with_response(200, Vec::new(), String::new(), Some(raw_response))
    }

    fn start_with_response(
        status: u16,
        response_body: Vec<u8>,
        content_type: String,
        raw_response: Option<Vec<u8>>,
    ) -> Self {
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
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(CLI_TIMEOUT)).unwrap();
                        let request = read_http_request(&mut stream).unwrap();
                        if let Some(response) = raw_response.as_ref() {
                            stream.write_all(response).unwrap();
                            stream.flush().unwrap();
                        } else {
                            write_http_response(&mut stream, status, &response_body, &content_type)
                                .unwrap();
                        }
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

struct CliCapture {
    output: std::process::Output,
    requests: Vec<Vec<u8>>,
    transcript: Option<Vec<u8>>,
    address: SocketAddr,
}

fn save_projection_capture(name: &str, capture: &CliCapture) {
    let Some(root) = std::env::var_os("LINGXI_R379_CAPTURE_DIR") else {
        return;
    };
    let root = PathBuf::from(root).join(name);
    std::fs::create_dir_all(&root)
        .unwrap_or_else(|error| panic!("create projection capture {}: {error}", root.display()));
    std::fs::write(root.join("stdout.raw"), &capture.output.stdout).unwrap();
    std::fs::write(root.join("stderr.raw"), &capture.output.stderr).unwrap();
    std::fs::write(
        root.join("process-result.txt"),
        format!("status={:?}\n", capture.output.status),
    )
    .unwrap();
    for (index, request) in capture.requests.iter().enumerate() {
        std::fs::write(root.join(format!("request-{index}.raw")), request).unwrap();
    }
    if let Some(transcript) = &capture.transcript {
        std::fs::write(root.join("session.jsonl.raw"), transcript).unwrap();
    }
}

fn run_local_cli(
    case_id: &str,
    session_id: &str,
    prompt: &str,
    api_key: &str,
    args: &[String],
    status: u16,
    response_body: &[u8],
    response_content_type: &str,
    raw_http_response: Option<&[u8]>,
    simple_mode: bool,
    read_transcript: bool,
) -> CliCapture {
    run_local_cli_with_env(
        case_id,
        session_id,
        prompt,
        api_key,
        args,
        status,
        response_body,
        response_content_type,
        raw_http_response,
        simple_mode,
        read_transcript,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn run_local_cli_with_env(
    case_id: &str,
    session_id: &str,
    prompt: &str,
    api_key: &str,
    args: &[String],
    status: u16,
    response_body: &[u8],
    response_content_type: &str,
    raw_http_response: Option<&[u8]>,
    simple_mode: bool,
    read_transcript: bool,
    extra_env: &[(&str, &str)],
) -> CliCapture {
    let config = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let stub = if let Some(raw_response) = raw_http_response {
        LocalApiStub::start_raw_response(raw_response.to_vec())
    } else {
        LocalApiStub::start(
            status,
            response_body.to_vec(),
            response_content_type.to_owned(),
        )
    };
    let address = stub.address;

    let mut command = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"));
    command
        // Match the retained Native capture's inheritedEnvironment=false.
        // Every required runtime and fixture variable is set below.
        .env_clear()
        .current_dir(cwd.path())
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", config.path())
        .env("TMPDIR", config.path())
        .env("LANG", "C.UTF-8")
        .env("TZ", "UTC")
        .env("TERM", "dumb")
        .env("LINGXI_CONFIG_DIR", config.path())
        .env("LINGXI_API_BASE_URL", format!("http://{address}"))
        .env(
            "LINGXI_NATIVE_ANTHROPIC_ENDPOINT",
            format!("http://{address}"),
        )
        .env_remove("LINGXI_R379_CAPTURE_DIR")
        .env("ANTHROPIC_API_KEY", api_key)
        .env("ANTHROPIC_MAX_RETRIES", "0")
        .env("CLAUDE_CONFIG_DIR", config.path())
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("DO_NOT_TRACK", "1")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_USE_BEDROCK")
        .env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .env_remove("CLAUDE_CODE_USE_ANTHROPIC_AWS")
        .env_remove("CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD")
        .env_remove("CLAUDE_CODE_USE_MANTLE")
        .env_remove("CLAUDE_CODE_USE_VERTEX");
    for (name, value) in extra_env {
        command.env(*name, *value);
    }
    if simple_mode {
        command.env("LINGXI_SIMPLE", "1");
    }
    command
        .args(args)
        .arg(prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("lingxi-cli timed out on local projection case {case_id}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let requests = stub.finish();
    let transcript = read_transcript.then(|| read_cli_transcript(config.path(), session_id));
    CliCapture {
        output,
        requests,
        transcript,
        address,
    }
}

fn read_cli_transcript(config_home: &Path, session_id: &str) -> Vec<u8> {
    let projects_root = config_home.join("projects");
    let mut matching = Vec::new();
    for entry in std::fs::read_dir(&projects_root).unwrap_or_else(|error| {
        panic!(
            "read isolated CLI projects directory {}: {error}",
            projects_root.display()
        )
    }) {
        let entry = entry.expect("read isolated CLI project entry");
        if !entry.file_type().expect("read project entry type").is_dir() {
            continue;
        }
        let path = entry.path().join(format!("{session_id}.jsonl"));
        if path.is_file() {
            matching.push(path);
        }
    }
    assert_eq!(matching.len(), 1, "one Product transcript for {session_id}");
    std::fs::read(&matching[0]).unwrap_or_else(|error| {
        panic!("read Product transcript {}: {error}", matching[0].display())
    })
}

fn invoke_cli(
    case: &Native295Case,
    response_body: &[u8],
    entrypoint: &str,
    no_stream: bool,
) -> (std::process::Output, Vec<Vec<u8>>) {
    // The provider-error receipt pins bytes but not argv. This explicit mode
    // avoids inheriting the fresh-session Auto fallback and model-gate notice.
    let mut args = [
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--permission-mode",
        "dontAsk",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    args.extend(entrypoint.split_whitespace().map(str::to_owned));
    args.extend(["--tools", ""].map(str::to_owned));
    args.extend(
        [
            "--max-budget-usd",
            "1",
            "--model",
            MODEL,
            "--session-id",
            case.session_id.as_str(),
        ]
        .map(str::to_owned),
    );
    if no_stream {
        args.push("--no-stream".into());
    }
    let capture = run_local_cli(
        &case.id,
        &case.session_id,
        PROMPT,
        API_KEY,
        &args,
        case.status,
        response_body,
        "application/json",
        None,
        true,
        false,
    );
    (capture.output, capture.requests)
}

fn invoke_projection_cli(
    case_id: &str,
    session_id: &str,
    prompt: &str,
    api_key: &str,
    args: &[&str],
    response_body: &[u8],
    response_content_type: &str,
    raw_http_response: Option<&[u8]>,
    simple_mode: bool,
    read_transcript: bool,
) -> CliCapture {
    run_local_cli(
        case_id,
        session_id,
        prompt,
        api_key,
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        200,
        response_body,
        response_content_type,
        raw_http_response,
        simple_mode,
        read_transcript,
    )
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

fn request_user_id_json(request: &[u8]) -> Utf16JsonProjection {
    let body = request_body_json(request);
    let encoded_user_id = body
        .string_at("/metadata/user_id")
        .expect("metadata.user_id is the SDK's JSON-encoded string");
    Utf16JsonProjection::parse(encoded_user_id).expect("metadata.user_id contains JSON")
}

fn refusal_retry_nudge_count(request: &[u8]) -> usize {
    let body = request_body_json(request);
    let mut count = 0;
    for message_index in 0..body.array_len("/messages").unwrap_or(0) {
        let message_path = format!("/messages/{message_index}");
        if body.string_at(&format!("{message_path}/role")) != Some("user") {
            continue;
        }
        let content_path = format!("{message_path}/content");
        if body.string_at(&content_path) == Some(NATIVE_REFUSAL_SAFETY_RETRY_USER_MESSAGE) {
            count += 1;
            continue;
        }
        for block_index in 0..body.array_len(&content_path).unwrap_or(0) {
            if body.string_at(&format!("{content_path}/{block_index}/text"))
                == Some(NATIVE_REFUSAL_SAFETY_RETRY_USER_MESSAGE)
            {
                count += 1;
            }
        }
    }
    count
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

fn refusal_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/refusal-stop-details-native-295")
}

fn refusal_retry_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/refusal-retry-native-295")
}

fn http_layout_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/http-layout-native-295/native-baseline/2.1.295")
}

fn native_http_fixture_api_key() -> String {
    let fixture_root = http_layout_fixture_root();
    let headers = fixture_file(&fixture_root, "native-a/request-headers.raw");
    let headers = std::str::from_utf8(&headers).expect("Native HTTP fixture headers are UTF-8");
    let api_key = request_header_value(headers, "x-api-key")
        .expect("Native HTTP fixture captures its dummy API key");
    assert!(
        api_key.to_ascii_lowercase().contains("dummy")
            || api_key.to_ascii_lowercase().contains("fixture")
            || api_key.to_ascii_lowercase().contains("test"),
        "the retained Native HTTP capture uses a synthetic fixture credential"
    );
    api_key.to_owned()
}

fn verify_sha256sum_manifest(fixture_root: &Path) {
    let manifest = std::fs::read_to_string(fixture_root.join("SHA256SUMS"))
        .expect("retained Native fixture SHA256SUMS");
    let mut checked = 0;
    for line in manifest.lines() {
        let (expected, relative_path) = line
            .split_once("  ")
            .expect("sha256sum entry uses two-space separator");
        let relative_path = Path::new(relative_path);
        assert!(
            !relative_path.is_absolute(),
            "fixture checksum path must stay relative: {relative_path:?}"
        );
        let raw = fixture_file(fixture_root, relative_path.to_str().unwrap());
        assert_eq!(
            sha256_hex(&raw),
            expected,
            "fixture artifact {relative_path:?}"
        );
        checked += 1;
    }
    assert!(checked > 0, "Native fixture checksum manifest is not empty");
}

fn verify_native_http_artifact_hashes(fixture_root: &Path) {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fixture_file(fixture_root, "artifact-sha256.json"))
            .expect("Native HTTP capture artifact manifest parses");
    assert_eq!(manifest["native_version"].as_str(), Some(NATIVE_VERSION));
    assert_eq!(manifest["native_sha256"].as_str(), Some(NATIVE_SHA256));
    let artifacts = manifest["artifacts"]
        .as_object()
        .expect("Native HTTP capture artifact entries");
    for (relative_path, expected) in artifacts {
        let path = Path::new(relative_path);
        assert!(
            !path.is_absolute(),
            "artifact path stays relative: {path:?}"
        );
        let raw = fixture_file(fixture_root, relative_path);
        assert_eq!(
            raw.len() as u64,
            expected["bytes"].as_u64().expect("artifact byte count"),
            "Native fixture artifact length: {relative_path}"
        );
        assert_eq!(
            sha256_hex(&raw),
            expected["sha256"].as_str().expect("artifact sha256"),
            "Native fixture artifact hash: {relative_path}"
        );
    }
    assert!(
        !artifacts.is_empty(),
        "HTTP fixture hash manifest is not empty"
    );
}

fn assert_raw_top_level_fields_match_except(
    actual: &[u8],
    native: &[u8],
    dynamic_fields: &[&[u8]],
    case_id: &str,
) {
    let actual_fields = raw_top_level_fields(actual);
    let native_fields = raw_top_level_fields(native);
    let actual_keys: Vec<_> = actual_fields.iter().map(|field| field.key).collect();
    let native_keys: Vec<_> = native_fields.iter().map(|field| field.key).collect();
    assert_eq!(
        actual_keys, native_keys,
        "case {case_id} top-level key order"
    );
    for (actual_field, native_field) in actual_fields.iter().zip(&native_fields) {
        if dynamic_fields.contains(&actual_field.key) {
            continue;
        }
        assert_eq!(
            actual_field.field,
            native_field.field,
            "case {case_id} deterministic raw field {}",
            String::from_utf8_lossy(actual_field.key)
        );
    }
}

fn normalize_loopback_host(headers: &[u8], expected_host: Option<SocketAddr>) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(headers.len());
    let mut host_count = 0;
    for line in headers.split_inclusive(|byte| *byte == b'\n') {
        let line_without_crlf = line.strip_suffix(b"\r\n").unwrap_or(line);
        let Some(colon) = line_without_crlf.iter().position(|byte| *byte == b':') else {
            normalized.extend_from_slice(line);
            continue;
        };
        let name = &line_without_crlf[..colon];
        let value = line_without_crlf[colon + 1..]
            .strip_prefix(b" ")
            .unwrap_or(&line_without_crlf[colon + 1..]);
        if name.eq_ignore_ascii_case(b"host") {
            host_count += 1;
            if let Some(expected_host) = expected_host {
                assert_eq!(
                    std::str::from_utf8(value).expect("Host header is UTF-8"),
                    expected_host.to_string(),
                    "Product request Host is the isolated loopback authority"
                );
            }
            normalized.extend_from_slice(name);
            normalized.extend_from_slice(b": <loopback-host>\r\n");
        } else {
            normalized.extend_from_slice(line);
        }
    }
    assert_eq!(host_count, 1, "HTTP request has exactly one Host header");
    normalized
}

fn count_raw_occurrences(raw: &[u8], needle: &[u8]) -> usize {
    assert!(!needle.is_empty(), "raw byte needle must not be empty");
    raw.windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn raw_literal_offset(raw: &[u8], needle: &[u8], description: &str) -> usize {
    assert_eq!(
        count_raw_occurrences(raw, needle),
        1,
        "exactly one {description} raw byte literal"
    );
    raw.windows(needle.len())
        .position(|window| window == needle)
        .expect("counted raw byte literal")
}

fn registered_branding_delta(actual: &[u8], native: &[u8]) -> usize {
    raw_literal_offset(
        actual,
        PRODUCT_HTTP_LAYOUT_BRANDING,
        "Product system prompt branding",
    );
    raw_literal_offset(
        native,
        NATIVE_HTTP_LAYOUT_BRANDING,
        "Native system prompt branding",
    );
    let delta = NATIVE_HTTP_LAYOUT_BRANDING.len() - PRODUCT_HTTP_LAYOUT_BRANDING.len();
    assert_eq!(
        actual.len().checked_add(delta),
        Some(native.len()),
        "the only permitted body-length delta is the registered branding literal"
    );
    delta
}

fn normalize_content_length_for_branding_delta(
    actual_headers: &[u8],
    native_headers: &[u8],
    actual_body_len: usize,
    native_body_len: usize,
    branding_delta: usize,
) -> Vec<u8> {
    assert_eq!(
        actual_body_len.checked_add(branding_delta),
        Some(native_body_len),
        "Content-Length may differ only by the registered branding delta"
    );
    let actual_line = format!("Content-Length: {actual_body_len}\r\n").into_bytes();
    let native_line = format!("Content-Length: {native_body_len}\r\n").into_bytes();
    let actual_offset = raw_literal_offset(
        actual_headers,
        &actual_line,
        "Product Content-Length header",
    );
    raw_literal_offset(native_headers, &native_line, "Native Content-Length header");

    let mut normalized = Vec::with_capacity(actual_headers.len() + branding_delta);
    normalized.extend_from_slice(&actual_headers[..actual_offset]);
    normalized.extend_from_slice(&native_line);
    normalized.extend_from_slice(&actual_headers[actual_offset + actual_line.len()..]);
    normalized
}

fn escaped_device_id_range(raw: &[u8], description: &str) -> (usize, usize) {
    const DEVICE_ID_MARKER: &[u8] = b"\\\"device_id\\\":\\\"";
    const ESCAPED_QUOTE: &[u8] = b"\\\"";
    let marker_offset = raw_literal_offset(raw, DEVICE_ID_MARKER, description);
    let start = marker_offset + DEVICE_ID_MARKER.len();
    let end = start
        + raw[start..]
            .windows(ESCAPED_QUOTE.len())
            .position(|window| window == ESCAPED_QUOTE)
            .expect("escaped device ID string terminator");
    assert_eq!(end - start, 64, "device ID keeps Native's 64-byte format");
    assert!(
        raw[start..end]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "device ID keeps Native's lowercase-hex format"
    );
    (start, end)
}

fn assert_raw_body_matches_except_registered_branding_and_device_id(
    actual: &[u8],
    native: &[u8],
) -> usize {
    assert_eq!(native.len(), 978, "Native 2.1.295 body length");
    let branding_delta = registered_branding_delta(actual, native);
    let branding_offset = raw_literal_offset(
        actual,
        PRODUCT_HTTP_LAYOUT_BRANDING,
        "Product system prompt branding",
    );
    let mut normalized = Vec::with_capacity(actual.len() + branding_delta);
    normalized.extend_from_slice(&actual[..branding_offset]);
    normalized.extend_from_slice(NATIVE_HTTP_LAYOUT_BRANDING);
    normalized.extend_from_slice(&actual[branding_offset + PRODUCT_HTTP_LAYOUT_BRANDING.len()..]);
    assert_eq!(
        normalized.len(),
        native.len(),
        "body length after branding transform"
    );

    let (actual_device_start, actual_device_end) =
        escaped_device_id_range(&normalized, "Product escaped device_id");
    let (native_device_start, native_device_end) =
        escaped_device_id_range(native, "Native escaped device_id");
    assert_eq!(
        actual_device_end - actual_device_start,
        native_device_end - native_device_start
    );
    normalized[actual_device_start..actual_device_end]
        .copy_from_slice(&native[native_device_start..native_device_end]);
    assert_eq!(
        normalized.as_slice(),
        native,
        "raw body bytes match after only the registered branding literal and dynamic device ID"
    );
    branding_delta
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
        // Native 2.1.295 emits this fixed client-metadata tuple; x-app is the
        // protocol identifier, not the product binary name.
        assert_eq!(request_header_value(&request, "x-app"), Some("cli"));
        assert_eq!(
            request_header_value(&request, "x-stainless-lang"),
            Some("js")
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-runtime"),
            Some("node")
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-package-version"),
            Some("0.128.0")
        );
        assert_eq!(
            request_header_value(&request, "x-stainless-runtime-version"),
            Some("v26.3.0")
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
    assert!(
        request_body_json(&requests[0])
            .at_pointer("/stream")
            .is_none(),
        "direct batched messages.create omits stream; only streaming requests add stream=true"
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

#[test]
fn public_cli_refusal_projection_preserves_native_295_result_and_source_rows() {
    let _test_lock = provider_error_cli_test_lock();
    let root = refusal_fixture_root();
    verify_sha256sum_manifest(&root);

    for case_id in ["refusal-cyber", "refusal-generic"] {
        let fixture =
            |name: &str| fixture_file(&root, &format!("native-baseline/{case_id}/{name}"));
        let receipt: serde_json::Value = serde_json::from_slice(&fixture("native-receipt.json"))
            .expect("Native refusal receipt parses");
        assert_eq!(receipt["native_version"].as_str(), Some(NATIVE_VERSION));
        assert_eq!(receipt["native_sha256"].as_str(), Some(NATIVE_SHA256));
        assert!(receipt["public_entrypoint"]
            .as_str()
            .unwrap()
            .contains("--print --output-format json"));
        assert_eq!(
            receipt["fixed_inputs"]["model"].as_str(),
            Some(NATIVE_REFUSAL_MODEL)
        );
        assert_eq!(
            receipt["fixed_inputs"]["prompt"].as_str(),
            Some(NATIVE_REFUSAL_PROMPT)
        );
        let session_id = receipt["fixed_inputs"]["session_id"].as_str().unwrap();
        let response_id = receipt["fixed_inputs"]["response_id"].as_str().unwrap();
        let response = fixture("native-response-01.sse.raw");
        let native_stdout = fixture("native.stdout.raw");
        let native_stderr = fixture("native.stderr.raw");
        let native_rows = fixture("session-stop-details-rows.raw.jsonl");
        let explanation: Vec<u16> =
            serde_json::from_value(receipt["response_fixture"]["explanation_utf16_units"].clone())
                .expect("Native explanation UTF-16 units");
        assert_eq!(receipt["native_exit_code"].as_i64(), Some(1));
        assert_eq!(
            sha256_hex(&native_stdout),
            receipt["stdout_sha256"].as_str().unwrap()
        );
        assert!(native_stderr.is_empty());
        assert_eq!(
            sha256_hex(&native_rows),
            receipt["stop_details_observation"]["raw_session_stop_details_rows_sha256"]
                .as_str()
                .unwrap()
        );

        let native_frame =
            Utf16JsonProjection::parse(std::str::from_utf8(&native_stdout).unwrap().trim_end())
                .expect("Native stdout JSON parses without reserialization");
        assert_eq!(native_frame.string_at("/type"), Some("result"));
        assert_eq!(
            native_frame.string_at("/terminal_reason"),
            Some("api_error")
        );
        assert_eq!(native_frame.string_at("/stop_reason"), Some("refusal"));
        assert_eq!(native_frame.string_at("/session_id"), Some(session_id));
        assert_eq!(
            native_frame.string_units("/result").unwrap(),
            serde_json::from_value::<Vec<u16>>(
                receipt["stop_details_observation"]["native_cli_result_utf16_units"].clone()
            )
            .unwrap()
        );
        let native_requests = receipt["requests"].as_array().unwrap();
        assert_eq!(native_requests.len(), 2);
        assert!(native_requests
            .iter()
            .all(
                |request| request["path"].as_str() == Some("/v1/messages?beta=true")
                    && request["stream_requested"].as_bool() == Some(true)
                    && request["response_bytes"].as_u64() == Some(response.len() as u64)
                    && request["response_sha256"].as_str() == Some(sha256_hex(&response).as_str())
            ));

        let args = [
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--permission-mode",
            "dontAsk",
            "--print",
            "--output-format",
            "json",
            "--tools",
            "",
            "--max-budget-usd",
            "1",
            "--max-turns",
            "1",
            "--model",
            NATIVE_REFUSAL_MODEL,
            "--session-id",
            session_id,
        ];
        // The refusal receipt confirms a fixed dummy credential but does not
        // retain its header bytes. Use the exact synthetic value captured by
        // the companion Native HTTP fixture; do not normalize Product's header.
        let api_key = native_http_fixture_api_key();
        let capture = invoke_projection_cli(
            case_id,
            session_id,
            NATIVE_REFUSAL_PROMPT,
            &api_key,
            &args,
            &response,
            "text/event-stream",
            None,
            false,
            true,
        );
        save_projection_capture(&format!("refusal/{case_id}"), &capture);
        let output = capture.output;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stderr, native_stderr);
        for request in &capture.requests {
            let body = request_body_json(request);
            let user_id = request_user_id_json(request);
            assert_eq!(body.string_at("/model"), Some(NATIVE_REFUSAL_MODEL));
            assert_eq!(
                body.at_pointer("/stream").and_then(|value| value.as_bool()),
                Some(true)
            );
            assert_eq!(user_id.string_at("/session_id"), Some(session_id));
            assert_eq!(user_id.string_at("/account_uuid"), Some(""));
            let device_id = user_id
                .string_at("/device_id")
                .expect("Product request has device ID");
            assert_eq!(device_id.len(), 64);
            assert!(device_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
            let headers = std::str::from_utf8(request).unwrap();
            assert_eq!(
                request_header_value(headers, "x-api-key"),
                Some(api_key.as_str())
            );
            assert_eq!(
                request_header_value(headers, "x-claude-code-session-id"),
                Some(session_id)
            );
        }

        let actual_frame =
            Utf16JsonProjection::parse(std::str::from_utf8(&output.stdout).unwrap().trim_end())
                .expect("Product stdout JSON parses without reserialization");
        assert_eq!(actual_frame.string_at("/type"), Some("result"));
        assert_eq!(
            actual_frame.string_at("/terminal_reason"),
            Some("api_error")
        );
        assert_eq!(actual_frame.string_at("/stop_reason"), Some("refusal"));
        assert_eq!(actual_frame.string_at("/session_id"), Some(session_id));
        assert_eq!(
            actual_frame.string_units("/result"),
            native_frame.string_units("/result")
        );
        assert_eq!(
            raw_top_level_value(&output.stdout, b"\"result\""),
            raw_top_level_value(&native_stdout, b"\"result\"")
        );
        assert!(!output
            .stdout
            .windows(b"\\ud800".len())
            .any(|window| window == b"\\ud800"));

        let generated = [
            b"\"duration_api_ms\"" as &[u8],
            b"\"duration_ms\"",
            b"\"uuid\"",
        ];
        assert_raw_top_level_fields_match_except(
            &output.stdout,
            &native_stdout,
            &generated,
            case_id,
        );
        let api_ms = raw_unsigned_integer(&output.stdout, b"\"duration_api_ms\"");
        let elapsed_ms = raw_unsigned_integer(&output.stdout, b"\"duration_ms\"");
        assert!(api_ms <= elapsed_ms && elapsed_ms <= CLI_TIMEOUT.as_millis() as u64);
        let uuid = actual_frame.string_at("/uuid").unwrap();
        assert_canonical_uuid_v4(uuid, "Product refusal result UUID");
        assert_ne!(uuid, session_id);

        let category = receipt["response_fixture"]["stop_details"]["category"].as_str();
        let mut persisted_source_rows = 0;
        let mut persisted_api_error = false;
        for line in capture
            .transcript
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let row = Utf16JsonProjection::parse(std::str::from_utf8(line).unwrap())
                .expect("Product JSONL row parses without reserialization");
            let is_refusal_api_error = row.string_at("/type") == Some("assistant")
                && row.string_at("/error") == Some("invalid_request")
                && row
                    .at_pointer("/isApiErrorMessage")
                    .and_then(|value| value.as_bool())
                    == Some(true);
            if is_refusal_api_error {
                assert_eq!(row.string_at("/refusedMessageId"), Some(response_id));
                assert_eq!(row.string_at("/message/stop_reason"), Some("refusal"));
                assert_eq!(row.string_at("/message/stop_details/type"), Some("refusal"));
                assert_eq!(row.string_at("/message/stop_details/category"), category);
                persisted_api_error = true;
            }
            if row.string_at("/type") != Some("assistant")
                || row.at_pointer("/message/stop_details").is_none()
            {
                continue;
            }
            if !is_refusal_api_error {
                persisted_source_rows += 1;
            }
            assert_eq!(row.string_at("/message/stop_details/type"), Some("refusal"));
            assert_eq!(
                row.string_units("/message/stop_details/explanation"),
                Some(explanation.clone())
            );
            assert_eq!(row.string_at("/message/stop_details/category"), category);
            if category.is_none() {
                assert!(row.at_pointer("/message/stop_details/category").is_none());
            }
        }
        assert_eq!(
            persisted_source_rows,
            native_requests.len(),
            "Product persists each provider refusal source row separately from the synthetic API-error row: {case_id}"
        );
        assert!(
            persisted_api_error,
            "Product persists the synthetic refusal API-error row and its refusedMessageId: {case_id}"
        );
        assert_eq!(
            capture.requests.len(),
            native_requests.len(),
            "Product and Native provider request count for {case_id}"
        );
    }
}

#[test]
fn public_cli_refusal_retry_honors_native_gate_without_stop_details() {
    let _test_lock = provider_error_cli_test_lock();
    let fixture_root = refusal_retry_fixture_root();
    let session_id = "6b7ff52d-e492-4084-88e2-5ad153e3c053";
    let api_key = native_http_fixture_api_key();
    let args = [
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--permission-mode",
        "dontAsk",
        "--print",
        "--output-format",
        "json",
        "--tools",
        "",
        "--max-budget-usd",
        "1",
        "--max-turns",
        "1",
        "--model",
        NATIVE_REFUSAL_MODEL,
        "--session-id",
        session_id,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let refusal_receipt: serde_json::Value = serde_json::from_slice(&fixture_file(
        &refusal_fixture_root().join("native-baseline/refusal-generic"),
        "native-receipt.json",
    ))
    .expect("Native generic refusal receipt parses");
    let response_id = refusal_receipt["fixed_inputs"]["response_id"]
        .as_str()
        .expect("Native receipt binds the fixed response ID");

    for (case, disabled, expected_nudges) in [
        ("enabled", false, vec![0_usize, 1]),
        ("disabled", true, vec![0]),
    ] {
        let case_root = fixture_root.join(case);
        verify_sha256sum_manifest(&case_root);
        let response = fixture_file(&case_root, "no-stop-details.sse.raw");
        let extra_env = if disabled {
            vec![("CLAUDE_CODE_DISABLE_REFUSAL_RETRY", "1")]
        } else {
            Vec::new()
        };
        let capture = run_local_cli_with_env(
            &format!("refusal-retry-{case}"),
            session_id,
            NATIVE_REFUSAL_PROMPT,
            &api_key,
            &args,
            200,
            &response,
            "text/event-stream",
            None,
            false,
            true,
            &extra_env,
        );

        assert_eq!(capture.output.status.code(), Some(1), "{case}");
        assert_eq!(capture.requests.len(), expected_nudges.len(), "{case}");
        assert_eq!(
            capture
                .requests
                .iter()
                .map(|request| refusal_retry_nudge_count(request))
                .collect::<Vec<_>>(),
            expected_nudges,
            "the retry gate is per query and is disabled by any nonempty raw env value: {case}"
        );
        let output = Utf16JsonProjection::parse(
            std::str::from_utf8(&capture.output.stdout)
                .expect("Product CLI stdout is UTF-8")
                .trim_end(),
        )
        .expect("Product CLI result frame is valid JSON");
        assert_eq!(output.string_at("/type"), Some("result"));
        assert_eq!(output.string_at("/stop_reason"), Some("refusal"));
        assert!(
            capture.transcript.is_some(),
            "session capture is available: {case}"
        );
        let mut nudge_rows = 0;
        for line in capture
            .transcript
            .as_deref()
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let row = Utf16JsonProjection::parse(std::str::from_utf8(line).unwrap())
                .expect("Product retry JSONL row parses without reserialization");
            if row.string_at("/type") != Some("user") {
                continue;
            }
            let content_path = "/message/content";
            let contains_nudge = row.string_at(content_path)
                == Some(NATIVE_REFUSAL_SAFETY_RETRY_USER_MESSAGE)
                || (0..row.array_len(content_path).unwrap_or(0)).any(|block_index| {
                    row.string_at(&format!("{content_path}/{block_index}/text"))
                        == Some(NATIVE_REFUSAL_SAFETY_RETRY_USER_MESSAGE)
                });
            if !contains_nudge {
                continue;
            }
            nudge_rows += 1;
            assert_eq!(
                row.at_pointer("/isMeta").and_then(|value| value.as_bool()),
                Some(true),
                "safety retry nudge is transcript metadata: {case}"
            );
            assert_eq!(
                row.at_pointer("/turnCompanion")
                    .and_then(|value| value.as_bool()),
                Some(true),
                "safety retry nudge is a turn companion: {case}"
            );
            assert_eq!(
                row.string_at("/refusedMessageId"),
                Some(response_id),
                "safety retry nudge carries the originating response ID: {case}"
            );
        }
        assert_eq!(
            nudge_rows,
            usize::from(!disabled),
            "only the enabled case persists the refusal safety retry nudge: {case}"
        );
    }
}

#[test]
fn public_cli_print_text_matches_native_295_raw_http_layout() {
    let _test_lock = provider_error_cli_test_lock();
    let fixture_root = http_layout_fixture_root();
    verify_native_http_artifact_hashes(&fixture_root);
    let comparison: serde_json::Value = serde_json::from_slice(&fixture_file(
        &fixture_root,
        "native-comparison-293-vs-295.json",
    ))
    .expect("Native HTTP comparison receipt parses");
    assert_eq!(
        comparison["native_295"]["version"].as_str(),
        Some("2.1.295")
    );
    assert_eq!(
        comparison["native_295"]["sha256"].as_str(),
        Some(NATIVE_SHA256)
    );
    assert_eq!(
        comparison["capture_outcome"]["messages_requests_per_run"][0].as_u64(),
        Some(1)
    );
    assert_eq!(
        comparison["capture_outcome"]["process_exit_codes"][0].as_i64(),
        Some(0)
    );

    let native_headers = fixture_file(&fixture_root, "native-a/request-headers.raw");
    let native_body = fixture_file(&fixture_root, "native-a/request-body.raw");
    let native_request = fixture_file(&fixture_root, "native-a/request.raw");
    let native_response = fixture_file(&fixture_root, "native-a/offered-response.raw");
    let native_actual_response = fixture_file(&fixture_root, "native-a/response.raw");
    let native_stdout = fixture_file(&fixture_root, "native-a/stdout.raw");
    let native_stderr = fixture_file(&fixture_root, "native-a/stderr.raw");
    assert_eq!(
        native_request,
        [native_headers.as_slice(), native_body.as_slice()].concat()
    );
    assert_eq!(
        native_response, native_actual_response,
        "Native offered and observed HTTP response bytes"
    );
    assert_eq!(native_stdout, b"HEADLESS_LOCAL_RESPONSE\n");
    assert!(native_stderr.is_empty());
    let native_headers_text = std::str::from_utf8(&native_headers).expect("Native headers UTF-8");
    let native_api_key = request_header_value(native_headers_text, "x-api-key")
        .expect("Native capture contains its fixed dummy API key");
    assert!(
        native_api_key.to_ascii_lowercase().contains("dummy")
            || native_api_key.to_ascii_lowercase().contains("fixture")
            || native_api_key.to_ascii_lowercase().contains("test"),
        "the retained Native capture uses a synthetic fixture credential"
    );

    let args = [
        "--bare",
        "--print",
        "--output-format",
        "text",
        "--model",
        NATIVE_HTTP_LAYOUT_MODEL,
        "--system-prompt",
        NATIVE_HTTP_LAYOUT_SYSTEM_PROMPT,
        "--tools",
        "",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
        "--session-id",
        NATIVE_HTTP_LAYOUT_SESSION_ID,
    ];
    let capture = invoke_projection_cli(
        "http-layout-295",
        NATIVE_HTTP_LAYOUT_SESSION_ID,
        NATIVE_HTTP_LAYOUT_PROMPT,
        native_api_key,
        &args,
        &native_response,
        "application/octet-stream",
        Some(&native_response),
        false,
        false,
    );
    save_projection_capture("http-layout-295", &capture);
    let output = capture.output;
    let requests = capture.requests;
    let address = capture.address;
    assert_eq!(
        output.status.code(),
        Some(0),
        "Product print-text exit code"
    );
    assert_eq!(
        output.stdout, native_stdout,
        "Product client-observed result bytes"
    );
    assert_eq!(output.stderr, native_stderr, "Product stderr bytes");
    assert_eq!(requests.len(), 1, "one Product Messages request");
    let actual_request = &requests[0];
    let header_end = find_bytes(actual_request, b"\r\n\r\n").expect("Product HTTP headers end");
    let actual_headers = &actual_request[..header_end + 4];
    let actual_body = &actual_request[header_end + 4..];
    let native_line_end =
        find_bytes(&native_headers, b"\r\n").expect("Native request line end") + 2;
    let actual_line_end =
        find_bytes(actual_headers, b"\r\n").expect("Product request line end") + 2;
    assert_eq!(
        &actual_headers[..actual_line_end],
        &native_headers[..native_line_end],
        "Product and Native raw request line"
    );
    let branding_delta =
        assert_raw_body_matches_except_registered_branding_and_device_id(actual_body, &native_body);
    let adjusted_actual_headers = normalize_content_length_for_branding_delta(
        &actual_headers[actual_line_end..],
        &native_headers[native_line_end..],
        actual_body.len(),
        native_body.len(),
        branding_delta,
    );
    assert_eq!(
        normalize_loopback_host(&adjusted_actual_headers, Some(address)),
        normalize_loopback_host(&native_headers[native_line_end..], None),
        "Product raw header order/bytes, allowing only loopback Host and registered Content-Length branding delta"
    );
    let actual_json = request_body_json(actual_request);
    assert_eq!(
        actual_json.string_at("/model"),
        Some(NATIVE_HTTP_LAYOUT_MODEL)
    );
    assert_eq!(
        actual_json
            .at_pointer("/stream")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    let actual_user_id = request_user_id_json(actual_request);
    assert_eq!(
        actual_user_id.string_at("/session_id"),
        Some(NATIVE_HTTP_LAYOUT_SESSION_ID)
    );
    assert_eq!(actual_user_id.string_at("/account_uuid"), Some(""));
    let actual_device_id = actual_user_id
        .string_at("/device_id")
        .expect("Product request device id");
    assert_eq!(actual_device_id.len(), 64);
    let (device_id_start, device_id_end) =
        escaped_device_id_range(actual_body, "Product escaped device_id");
    assert_eq!(
        actual_device_id.as_bytes(),
        &actual_body[device_id_start..device_id_end],
        "the parsed dynamic device_id is exactly the one raw 64-byte span"
    );
    let headers_text = std::str::from_utf8(actual_headers).expect("Product HTTP headers UTF-8");
    let expected_host = format!("127.0.0.1:{}", address.port());
    assert_eq!(
        request_header_value(headers_text, "host"),
        Some(expected_host.as_str())
    );
    assert_eq!(
        request_header_value(headers_text, "x-api-key"),
        Some(native_api_key)
    );
}
