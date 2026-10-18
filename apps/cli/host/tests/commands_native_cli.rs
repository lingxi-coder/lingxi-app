//! Public CLI replay for the five Native 2.1.295 slash-command/Skill cases.
//!
//! The test starts the real `lingxi-cli` binary, feeds the captured argv or
//! stream-json input, captures its raw Anthropic request and session JSONL, and
//! answers from a 127.0.0.1-only Messages stub. The Native request/session
//! bodies are intentionally kept outside the checkout; curated fixed fields
//! and the Native stdout event sample retain the byte/source assertions.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const FIXTURE_PATH: &str = "tests/fixtures/commands-native-295-rerun";
const API_KEY: &str = "native-commands-fixture-dummy-key";

#[derive(Debug, Deserialize)]
struct CommandInputs {
    native_version: String,
    cases: Vec<CommandInputCase>,
}

#[derive(Debug, Deserialize)]
struct CommandInputCase {
    id: String,
    kind: String,
    prompt: Option<String>,
    prompt_utf16le_hex: Option<String>,
    session_id: String,
    skill_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeRerunReceipt {
    native_version: String,
    native_sha256: String,
    fixed_inputs: NativeFixedInputs,
    all_requests_accepted_locally: bool,
    no_upstream_relay: bool,
    cases: Vec<NativeReceiptCase>,
}

#[derive(Debug, Deserialize)]
struct NativeFixedInputs {
    cwd: String,
}

#[derive(Debug, Deserialize)]
struct NativeReceiptCase {
    id: String,
    kind: String,
    session_id: String,
    input_file: Option<String>,
    argv: Vec<String>,
    native_exit_code: i32,
    stdout_file: String,
    stdout_bytes: usize,
    stdout_sha256: String,
    stderr_file: String,
    stderr_bytes: usize,
    stderr_sha256: String,
    loopback_requests: Vec<NativeRequestReceipt>,
    session_persistence: NativeSessionReceipt,
}

#[derive(Debug, Deserialize)]
struct NativeRequestReceipt {
    method: String,
    path: String,
    accepted: bool,
    dummy_key_matched: bool,
    bytes: usize,
    sha256: String,
    stream: bool,
    tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NativeSessionReceipt {
    bytes: usize,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct NativeFixedFields {
    native_version: String,
    native_sha256: String,
    source_cases_sha256: String,
    source_native_receipt_sha256: String,
    rerun_receipt_sha256: String,
    dynamic_fields: Vec<String>,
    cases: Vec<NativeFixedCase>,
}

#[derive(Debug, Deserialize)]
struct NativeFixedCase {
    id: String,
    request_sha256: String,
    request_bytes: usize,
    request_count: usize,
    request_model: String,
    request_user_text_blocks: Vec<String>,
    session_sha256: String,
    session_bytes: usize,
    session_user_rows_sha256: String,
    session_user_rows_bytes: usize,
    session_user_content_units: Vec<Vec<u16>>,
    stdout_sha256: String,
    stdout_bytes: usize,
    stderr_sha256: String,
    stderr_bytes: usize,
    event_types: Vec<EventSignature>,
    init_session_id: Option<String>,
    init_skill_names: Vec<String>,
    init_slash_skill_names: Vec<String>,
    assistant_message_id: Option<String>,
    assistant_model: Option<String>,
    assistant_text: String,
    result_fields: NativeResultFields,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct EventSignature {
    #[serde(rename = "type")]
    type_name: String,
    subtype: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeResultFields {
    #[serde(rename = "type")]
    type_name: Option<String>,
    subtype: Option<String>,
    is_error: Option<bool>,
    stop_reason: Option<String>,
    num_turns: Option<u64>,
    result: Option<String>,
    session_id: Option<String>,
}

struct LocalApiStub {
    address: SocketAddr,
    done: mpsc::Sender<()>,
    thread: thread::JoinHandle<Vec<Vec<u8>>>,
}

impl LocalApiStub {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (done, done_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            let started = Instant::now();
            let mut finish_deadline = None;
            loop {
                if done_rx.try_recv().is_ok() {
                    finish_deadline = Some(Instant::now() + Duration::from_millis(250));
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let request = read_http_request(&mut stream).unwrap();
                        write_http_response(&mut stream, &fixed_response_stream()).unwrap();
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if finish_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            break;
                        }
                        assert!(
                            started.elapsed() < Duration::from_secs(75),
                            "CLI never connected to its local Messages stub"
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

fn fixed_response_stream() -> Vec<u8> {
    let events = [
        (
            "message_start",
            serde_json::json!({
                "type":"message_start",
                "message":{
                    "id":"msg_native_commands_fixture",
                    "type":"message",
                    "role":"assistant",
                    "model":"claude-sonnet-4-5-20250929",
                    "content":[],
                    "stop_reason":null,
                    "stop_sequence":null,
                    "usage":{
                        "input_tokens":5,
                        "output_tokens":0,
                        "cache_creation_input_tokens":0,
                        "cache_read_input_tokens":0
                    }
                }
            }),
        ),
        (
            "content_block_start",
            serde_json::json!({
                "type":"content_block_start",
                "index":0,
                "content_block":{"type":"text","text":""}
            }),
        ),
        (
            "content_block_delta",
            serde_json::json!({
                "type":"content_block_delta",
                "index":0,
                "delta":{"type":"text_delta","text":"fixed native stub result"}
            }),
        ),
        (
            "content_block_stop",
            serde_json::json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            serde_json::json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                "usage":{"output_tokens":4}
            }),
        ),
        ("message_stop", serde_json::json!({"type":"message_stop"})),
    ];
    let mut output = Vec::new();
    for (event, data) in events {
        writeln!(output, "event: {event}").unwrap();
        writeln!(output, "data: {}", serde_json::to_string(&data).unwrap()).unwrap();
        output.push(b'\n');
    }
    output
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
        "connection closed before the request body completed",
    ))
}

fn write_http_response(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_PATH)
}

fn fixture_file(root: &Path, path: &str) -> Vec<u8> {
    let path = root.join(path);
    assert!(path.starts_with(root), "fixture path escaped its directory");
    std::fs::read(path).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode_utf16le_hex(raw: &str) -> Vec<u16> {
    assert_eq!(raw.len() % 4, 0, "UTF-16LE hex has complete code units");
    raw.as_bytes()
        .chunks_exact(4)
        .map(|digits| {
            let byte =
                |pair: &[u8]| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
            u16::from_le_bytes([byte(&digits[..2]), byte(&digits[2..])])
        })
        .collect()
}

fn create_project_skills(cwd: &Path) {
    // The 2.1.295 capture keeps Native's raw `.claude/skills` inputs. The
    // actual LingXi CLI discovers project skills under its branded
    // `<cwd>/<DOT_DIR>/skills` root; comparisons below translate only the
    // resulting skill base-directory prefix back to Native's `.claude` root.
    let skills_root = cwd.join(branding::DOT_DIR).join("skills");
    for (name, description) in [
        ("skill-🧪-probe", "Unicode-scalar Skill fixture"),
        ("skill.alpha+beta", "Punctuation Skill fixture"),
    ] {
        let directory = skills_root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let contents = format!(
            "---\nname: {name}\ndescription: {description}\n---\nReturn the fixed Skill body for {name}.\n"
        );
        std::fs::write(directory.join("SKILL.md"), contents).unwrap();
    }
    assert!(
        skills_root.is_dir(),
        "project skill fixture must exist under the production brand root"
    );
}

fn map_project_skill_base_path_to_native(text: &str) -> String {
    let Some(body) = text.strip_prefix("Base directory for this skill: ") else {
        return text.to_owned();
    };
    let project_skill_root = format!("<PROJECT_ROOT>/{}/skills/", branding::DOT_DIR);
    let body = body
        .strip_prefix(&project_skill_root)
        .expect("the actual skill body must use the branded project skill root");
    format!("Base directory for this skill: <PROJECT_ROOT>/.claude/skills/{body}")
}

fn map_project_skill_base_path_units_to_native(units: &[u16]) -> Vec<u16> {
    let project_skill_root = format!(
        "Base directory for this skill: <PROJECT_ROOT>/{}/skills/",
        branding::DOT_DIR
    );
    let project_prefix: Vec<_> = project_skill_root.encode_utf16().collect();
    assert!(
        units.starts_with(&project_prefix),
        "only the exact skill base-directory prefix is normalized"
    );

    let native_prefix: Vec<_> = "Base directory for this skill: <PROJECT_ROOT>/.claude/skills/"
        .encode_utf16()
        .collect();
    native_prefix
        .into_iter()
        .chain(units[project_prefix.len()..].iter().copied())
        .collect()
}

fn relevant_request_text_blocks(
    case: &CommandInputCase,
    request: &Value,
    cwd: &str,
) -> Vec<String> {
    let mut text_blocks = Vec::new();
    for message in request["messages"].as_array().into_iter().flatten() {
        if message["role"].as_str() != Some("user") {
            continue;
        }
        match &message["content"] {
            Value::String(text) => text_blocks.push(text.clone()),
            Value::Array(blocks) => {
                for block in blocks {
                    if let Some(text) = block["text"].as_str() {
                        text_blocks.push(text.to_owned());
                    }
                }
            }
            _ => {}
        }
    }

    text_blocks
        .into_iter()
        .filter(|text| match case.id.as_str() {
            "slash-only" => text == "/",
            "unknown-slash-command" => text.contains("/__native_unknown_293__"),
            "unicode-skill-name" | "punctuation-skill-name" => {
                text.contains("<command-message>")
                    || text.contains("Base directory for this skill:")
            }
            "unpaired-surrogate-skill-name" => text.starts_with("/skill_"),
            _ => false,
        })
        .map(|text| {
            let text = text.replace(cwd, "<PROJECT_ROOT>");
            if matches!(
                case.id.as_str(),
                "unicode-skill-name" | "punctuation-skill-name"
            ) {
                map_project_skill_base_path_to_native(&text)
            } else {
                text
            }
        })
        .collect()
}

fn unknown_reminder_sections(text: &str) -> (&str, &str, &str) {
    const COMMANDS_MARKER: &str = "Slash commands available in this session: ";
    const AFTER_COMMANDS: &str =
        ". Treat the message as a plain request and do the task with the tools you have.";
    let (prefix, commands_and_suffix) = text
        .split_once(COMMANDS_MARKER)
        .expect("unknown reminder includes the available-command section");
    let (commands, _) = commands_and_suffix
        .split_once(AFTER_COMMANDS)
        .expect("unknown reminder includes its plain-request guidance");
    (prefix, commands, &commands_and_suffix[commands.len()..])
}

fn parse_runtime_command_preview(list: &str) -> (Vec<String>, Option<usize>) {
    let mut names = Vec::new();
    let mut remaining = None;
    for entry in list.split(", ") {
        if let Some(count) = entry
            .strip_prefix("and ")
            .and_then(|value| value.strip_suffix(" more"))
        {
            assert!(remaining.is_none(), "one remainder entry is allowed");
            remaining = Some(count.parse::<usize>().expect("numeric remainder"));
        } else {
            names.push(
                entry
                    .strip_prefix('/')
                    .expect("Native preview names start with a slash")
                    .to_owned(),
            );
        }
    }
    (names, remaining)
}

fn assert_runtime_command_preview(actual: &str, native: &str) {
    const PRODUCT_ONLY_SKILLS: &[&str] = &["create-local-app"];

    let (actual_names, actual_remainder) = parse_runtime_command_preview(actual);
    let (native_names, native_remainder) = parse_runtime_command_preview(native);
    let expected_product_names = native_names;
    let comparable_actual_names = actual_names
        .iter()
        .filter(|name| !PRODUCT_ONLY_SKILLS.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let product_only_names = actual_names
        .iter()
        .filter(|name| !expected_product_names.contains(name))
        .map(String::as_str)
        .collect::<Vec<_>>();

    assert!(
        product_only_names
            .iter()
            .all(|name| PRODUCT_ONLY_SKILLS.contains(name)),
        "unexpected extra prompt commands: {product_only_names:?}"
    );
    assert_eq!(
        comparable_actual_names, expected_product_names,
        "the headless preview keeps Native command filtering and relative source order, with only the named product variants omitted"
    );
    assert_eq!(
        actual_remainder, native_remainder,
        "the remainder counts available non-prompt commands and must match the Native source; actual listed names={actual_names:?}, implied available count={}, Native listed names={expected_product_names:?}, implied available count={}",
        actual_names.len() + actual_remainder.unwrap_or_default(),
        expected_product_names.len() + native_remainder.unwrap_or_default(),
    );
}

fn session_user_content_units(raw_jsonl: &str, cwd: &str) -> Vec<Vec<u16>> {
    let mut content = Vec::new();
    for line in raw_jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let row = Utf16JsonProjection::parse(line).unwrap();
        if row.string_at("/type") != Some("user") {
            continue;
        }
        let message_content = "/message/content";
        if let Some(units) = row.string_units(message_content) {
            content.push(replace_units(&units, cwd, "<PROJECT_ROOT>"));
            continue;
        }
        let Some(block_count) = row.array_len(message_content) else {
            continue;
        };
        for index in 0..block_count {
            let text_path = format!("{message_content}/{index}/text");
            if let Some(units) = row.string_units(&text_path) {
                content.push(replace_units(&units, cwd, "<PROJECT_ROOT>"));
            } else {
                let block_path = format!("{message_content}/{index}");
                if let Some(units) = row.string_units(&block_path) {
                    content.push(replace_units(&units, cwd, "<PROJECT_ROOT>"));
                }
            }
        }
    }
    content
}

fn replace_units(input: &[u16], needle: &str, replacement: &str) -> Vec<u16> {
    let needle: Vec<u16> = needle.encode_utf16().collect();
    let replacement: Vec<u16> = replacement.encode_utf16().collect();
    if needle.is_empty() {
        return input.to_vec();
    }
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor..].starts_with(&needle) {
            output.extend_from_slice(&replacement);
            cursor += needle.len();
        } else {
            output.push(input[cursor]);
            cursor += 1;
        }
    }
    output
}

fn session_user_message_path(config: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    session::session_path(config, &cwd.display().to_string(), session_id)
}

struct CliFixtureEnvironment {
    _home: tempfile::TempDir,
    config: tempfile::TempDir,
    _cwd: tempfile::TempDir,
    cwd_path: PathBuf,
}

impl CliFixtureEnvironment {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("canonicalize CLI fixture cwd");
        create_project_skills(&cwd_path);
        Self {
            _home: home,
            config,
            _cwd: cwd,
            cwd_path,
        }
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        session_user_message_path(self.config.path(), &self.cwd_path, session_id)
    }

    fn run_cli(
        &self,
        argv: &[String],
        input: &[u8],
        case_id: &str,
    ) -> (std::process::Output, Vec<Vec<u8>>) {
        let stub = LocalApiStub::start();
        let mut command = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"));
        command
            .env_clear()
            .current_dir(&self.cwd_path)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self._home.path())
            .env("TMPDIR", self.config.path().parent().unwrap())
            .env("LANG", "C.UTF-8")
            .env("TZ", "UTC")
            .env("TERM", "dumb")
            .env("LINGXI_CONFIG_DIR", self.config.path())
            .env("CLAUDE_CONFIG_DIR", self.config.path())
            .env("LINGXI_API_BASE_URL", format!("http://{}", stub.address))
            .env(
                "LINGXI_NATIVE_ANTHROPIC_ENDPOINT",
                format!("http://{}", stub.address),
            )
            .env("ANTHROPIC_API_KEY", API_KEY)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DO_NOT_TRACK", "1")
            .args(argv.iter().skip(1).map(String::as_str))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("lingxi-cli timed out in command case {case_id}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        let requests = stub.finish();
        (output, requests)
    }
}

fn run_cli_case(
    case: &CommandInputCase,
    receipt: &NativeReceiptCase,
    fixture_root: &Path,
) -> (std::process::Output, Vec<Vec<u8>>, String, Vec<u8>) {
    let environment = CliFixtureEnvironment::new();
    let input = receipt
        .input_file
        .as_deref()
        .map(|path| fixture_file(fixture_root, path))
        .unwrap_or_default();
    let (output, requests) = environment.run_cli(&receipt.argv, &input, &case.id);
    let transcript_path = environment.session_path(&case.session_id);
    let transcript = std::fs::read(&transcript_path).unwrap_or_else(|error| {
        panic!(
            "case {} session transcript missing at {}: {error}; exit status: {:?}; stdout: {}; stderr: {}; loopback requests: {}",
            case.id,
            transcript_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            requests.len()
        )
    });
    (
        output,
        requests,
        environment.cwd_path.to_str().unwrap().to_owned(),
        transcript,
    )
}

fn request_parts(request: &[u8]) -> (String, Vec<u8>) {
    let header_end = find_bytes(request, b"\r\n\r\n").expect("request headers terminate");
    let headers = String::from_utf8_lossy(&request[..header_end]);
    (
        headers.lines().next().unwrap_or_default().to_owned(),
        request[header_end + 4..].to_vec(),
    )
}

fn event_signature(frame: &Value) -> EventSignature {
    EventSignature {
        type_name: frame["type"].as_str().unwrap_or_default().to_owned(),
        subtype: frame["subtype"].as_str().map(str::to_owned),
    }
}

fn stream_frames(raw: &[u8]) -> Vec<Value> {
    std::str::from_utf8(raw)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn actual_cli_matches_native_2_1_295_slash_skill_and_stream_json_fixed_fields() {
    let fixture_root = fixture_root();
    let input_fixture: CommandInputs =
        serde_json::from_slice(&fixture_file(&fixture_root, "cases.json")).unwrap();
    let receipt_bytes = fixture_file(&fixture_root, "native-rerun-receipt.json");
    let receipt: NativeRerunReceipt = serde_json::from_slice(&receipt_bytes).unwrap();
    let fixed_bytes = fixture_file(&fixture_root, "native-fixed-fields.json");
    let fixed: NativeFixedFields = serde_json::from_slice(&fixed_bytes).unwrap();
    assert_eq!(input_fixture.native_version, "2.1.293");
    assert_eq!(receipt.native_version, "2.1.295");
    assert_eq!(fixed.native_version, "2.1.295");
    assert_eq!(fixed.native_version, receipt.native_version);
    assert_eq!(fixed.native_sha256, receipt.native_sha256);
    assert_eq!(
        receipt.native_sha256,
        "0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70"
    );
    assert!(receipt.all_requests_accepted_locally);
    assert!(receipt.no_upstream_relay);
    assert_eq!(
        fixed.source_cases_sha256,
        sha256_hex(&fixture_file(&fixture_root, "cases.json"))
    );
    assert_eq!(
        fixed.source_native_receipt_sha256,
        sha256_hex(&fixture_file(
            &fixture_root,
            "source/native-receipt-293.json"
        ))
    );
    assert_eq!(fixed.rerun_receipt_sha256, sha256_hex(&receipt_bytes));
    assert_eq!(input_fixture.cases.len(), 5);
    assert_eq!(receipt.cases.len(), input_fixture.cases.len());
    assert_eq!(fixed.cases.len(), input_fixture.cases.len());
    assert!(fixed
        .dynamic_fields
        .iter()
        .any(|field| field == "cwd paths"));

    for ((case, native), captured) in input_fixture
        .cases
        .iter()
        .zip(&receipt.cases)
        .zip(&fixed.cases)
    {
        assert_eq!(case.id, native.id);
        assert_eq!(case.id, captured.id);
        assert_eq!(case.kind, native.kind);
        assert_eq!(case.session_id, native.session_id);
        assert_eq!(native.native_exit_code, 0);
        assert_eq!(native.loopback_requests.len(), captured.request_count);
        assert_eq!(native.loopback_requests.len(), 1);
        assert_eq!(native.loopback_requests[0].method, "POST");
        assert_eq!(native.loopback_requests[0].path, "/v1/messages?beta=true");
        assert!(native.loopback_requests[0].accepted);
        assert!(native.loopback_requests[0].dummy_key_matched);
        assert!(native.loopback_requests[0].stream);
        assert!(native.loopback_requests[0].tools.is_empty());
        assert_eq!(native.loopback_requests[0].sha256, captured.request_sha256);
        assert_eq!(native.loopback_requests[0].bytes, captured.request_bytes);
        assert_eq!(native.session_persistence.sha256, captured.session_sha256);
        assert_eq!(native.session_persistence.bytes, captured.session_bytes);
        let native_user_rows = fixture_file(
            &fixture_root,
            &format!("native-session-user-rows/{}.jsonl", case.id),
        );
        assert_eq!(
            native_user_rows.len(),
            captured.session_user_rows_bytes,
            "case {} Native user-row slice byte count",
            case.id
        );
        assert_eq!(
            sha256_hex(&native_user_rows),
            captured.session_user_rows_sha256,
            "case {} Native user-row slice hash",
            case.id
        );
        assert_eq!(
            session_user_content_units(
                std::str::from_utf8(&native_user_rows).unwrap(),
                &receipt.fixed_inputs.cwd,
            ),
            captured.session_user_content_units,
            "case {} curated code units must come from the raw Native JSONL rows",
            case.id
        );

        if case.kind == "argv" {
            assert_eq!(
                native.argv.last().map(String::as_str),
                case.prompt.as_deref()
            );
            assert!(native.input_file.is_none());
        } else {
            assert!(case.prompt.is_none());
            let input_file = native.input_file.as_deref().unwrap();
            let input_bytes = fixture_file(&fixture_root, input_file);
            let input_line = std::str::from_utf8(&input_bytes).unwrap().trim_end();
            let input = Utf16JsonProjection::parse(input_line).unwrap();
            let expected_units = decode_utf16le_hex(case.prompt_utf16le_hex.as_deref().unwrap());
            assert_eq!(
                input.string_units("/message/content/0/text"),
                Some(expected_units.clone())
            );
            assert!(expected_units.contains(&0xd800));
        }

        let native_stdout = fixture_file(
            &fixture_root,
            &format!("native-baseline/{}", native.stdout_file),
        );
        let native_stderr = fixture_file(
            &fixture_root,
            &format!("native-baseline/{}", native.stderr_file),
        );
        assert_eq!(native.stdout_bytes, captured.stdout_bytes);
        assert_eq!(native.stderr_bytes, captured.stderr_bytes);
        assert_eq!(sha256_hex(&native_stdout), native.stdout_sha256);
        assert_eq!(sha256_hex(&native_stderr), native.stderr_sha256);
        assert_eq!(native.stdout_sha256, captured.stdout_sha256);
        assert_eq!(native.stderr_sha256, captured.stderr_sha256);

        if let Some(skill_name) = case.skill_name.as_deref() {
            assert!(captured
                .request_user_text_blocks
                .iter()
                .any(|text| text.contains(skill_name)));
        }

        let (output, requests, cwd, session_raw) = run_cli_case(case, native, &fixture_root);
        assert_eq!(
            output.status.code(),
            Some(native.native_exit_code),
            "case {}; status: {:?}; stdout: {}; stderr: {}",
            case.id,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            sha256_hex(&output.stderr),
            native.stderr_sha256,
            "case {} stderr",
            case.id
        );
        assert_eq!(
            requests.len(),
            captured.request_count,
            "case {} request count",
            case.id
        );

        let (request_line, request_body) = request_parts(&requests[0]);
        let request_line_parts: Vec<_> = request_line.split_whitespace().collect();
        assert_eq!(
            request_line_parts.first(),
            Some(&"POST"),
            "case {}",
            case.id
        );
        assert_eq!(
            request_line_parts.get(1),
            Some(&native.loopback_requests[0].path.as_str())
        );
        assert!(
            String::from_utf8_lossy(&requests[0]).contains(API_KEY),
            "case {} missed the isolated dummy credential",
            case.id
        );
        let request: Value = serde_json::from_slice(&request_body).unwrap();
        assert_eq!(
            request["model"].as_str(),
            Some(captured.request_model.as_str())
        );
        assert_eq!(request["stream"].as_bool(), Some(true));
        assert_eq!(request["tools"].as_array().map_or(0, Vec::len), 0);
        let cwd_text = cwd.as_str();
        let actual_request_blocks = relevant_request_text_blocks(case, &request, cwd_text);
        let session_text = std::str::from_utf8(&session_raw).unwrap();
        let mut actual_session_units = session_user_content_units(session_text, cwd_text);
        if matches!(
            case.id.as_str(),
            "unicode-skill-name" | "punctuation-skill-name"
        ) {
            assert_eq!(
                actual_session_units.len(),
                2,
                "registered project Skill persists its command row and body row"
            );
            actual_session_units[1] =
                map_project_skill_base_path_units_to_native(&actual_session_units[1]);
        }
        assert_eq!(
            actual_session_units, captured.session_user_content_units,
            "case {} session user fields differ from captured Native UTF-16 units",
            case.id
        );

        if case.id == "unpaired-surrogate-skill-name" {
            assert!(session_raw.windows(6).any(|window| window == b"\\ud800"));
            let native_units = &captured.session_user_content_units[0];
            assert!(native_units.contains(&0xd800));
            assert!(actual_request_blocks
                .iter()
                .any(|text| text.contains("/skill_�_probe")));
            assert!(!actual_request_blocks
                .iter()
                .any(|text| text.encode_utf16().any(|unit| unit == 0xd800)));
        } else if case.id == "unknown-slash-command" {
            let native_reminder = &captured.request_user_text_blocks[0];
            let actual_reminder = actual_request_blocks
                .first()
                .expect("unknown-command context precedes the untouched user input");
            let (native_prefix, native_commands, native_suffix) =
                unknown_reminder_sections(native_reminder);
            let (actual_prefix, actual_commands, actual_suffix) =
                unknown_reminder_sections(actual_reminder);
            assert_eq!(actual_prefix, native_prefix);
            assert_eq!(actual_suffix, native_suffix);
            assert_runtime_command_preview(actual_commands, native_commands);
            assert_eq!(
                actual_request_blocks.get(1),
                captured.request_user_text_blocks.get(1),
                "the original slash command remains a separate, following user block"
            );
        } else {
            assert_eq!(
                actual_request_blocks, captured.request_user_text_blocks,
                "case {} fixed request user/Skill blocks differ from Native capture",
                case.id
            );
        }

        if case.kind == "argv" {
            assert_eq!(
                output.stdout, native_stdout,
                "case {} plain result bytes differ from captured Native output",
                case.id
            );
        } else {
            let native_frames = stream_frames(&native_stdout);
            let actual_frames = stream_frames(&output.stdout);
            assert_eq!(
                actual_frames
                    .iter()
                    .map(event_signature)
                    .collect::<Vec<_>>(),
                captured.event_types,
                "Native stream-json event sequence for {}",
                case.id
            );
            assert_eq!(
                native_frames
                    .iter()
                    .map(event_signature)
                    .collect::<Vec<_>>(),
                captured.event_types,
                "captured stream-json fixture is consistent for {}",
                case.id
            );
            let native_init = native_frames
                .iter()
                .find(|frame| frame["type"] == "system" && frame["subtype"] == "init")
                .unwrap();
            let actual_init = actual_frames
                .iter()
                .find(|frame| frame["type"] == "system" && frame["subtype"] == "init")
                .unwrap();
            assert_eq!(native_init["session_id"], case.session_id);
            assert_eq!(actual_init["session_id"], case.session_id);
            assert_eq!(actual_init["cwd"].as_str(), Some(cwd_text));
            for frame in &actual_frames {
                if let Some(uuid) = frame.get("uuid").and_then(Value::as_str) {
                    assert!(uuid::Uuid::parse_str(uuid).is_ok());
                }
            }
            for name in &captured.init_skill_names {
                assert!(native_init["skills"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(name)));
                assert!(actual_init["skills"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(name)));
            }
            for name in &captured.init_slash_skill_names {
                assert!(native_init["slash_commands"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(name)));
                assert!(actual_init["slash_commands"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(name)));
            }
            assert_eq!(
                actual_init["session_id"].as_str(),
                captured.init_session_id.as_deref()
            );
            let native_assistant = native_frames
                .iter()
                .find(|frame| frame["type"] == "assistant")
                .unwrap();
            let actual_assistant = actual_frames
                .iter()
                .find(|frame| frame["type"] == "assistant")
                .unwrap();
            assert_eq!(
                native_assistant["message"]["id"].as_str(),
                captured.assistant_message_id.as_deref()
            );
            assert_eq!(
                actual_assistant["message"]["id"],
                native_assistant["message"]["id"]
            );
            assert_eq!(
                actual_assistant["message"]["model"].as_str(),
                captured.assistant_model.as_deref()
            );
            assert_eq!(
                actual_assistant["message"]["content"][0]["text"].as_str(),
                Some(captured.assistant_text.as_str())
            );
            let native_result = native_frames
                .iter()
                .find(|frame| frame["type"] == "result")
                .unwrap();
            let actual_result = actual_frames
                .iter()
                .find(|frame| frame["type"] == "result")
                .unwrap();
            assert_eq!(
                native_result["type"].as_str(),
                captured.result_fields.type_name.as_deref()
            );
            assert_eq!(
                native_result["subtype"].as_str(),
                captured.result_fields.subtype.as_deref()
            );
            assert_eq!(
                native_result["is_error"].as_bool(),
                captured.result_fields.is_error
            );
            assert_eq!(
                native_result["stop_reason"].as_str(),
                captured.result_fields.stop_reason.as_deref()
            );
            assert_eq!(
                native_result["num_turns"].as_u64(),
                captured.result_fields.num_turns
            );
            assert_eq!(
                native_result["result"].as_str(),
                captured.result_fields.result.as_deref()
            );
            assert_eq!(
                native_result["session_id"].as_str(),
                captured.result_fields.session_id.as_deref()
            );
            for key in [
                "type",
                "subtype",
                "is_error",
                "stop_reason",
                "num_turns",
                "result",
                "session_id",
            ] {
                assert_eq!(actual_result[key], native_result[key], "result field {key}");
            }
            assert!(uuid::Uuid::parse_str(actual_result["uuid"].as_str().unwrap()).is_ok());
            assert!(actual_result["duration_ms"].as_u64().is_some());
            assert!(actual_result["usage"].is_object());
        }
    }
}

#[test]
fn cli_cold_resume_preserves_skill_source_and_normalizes_anthropic_wire() {
    let fixture_root = fixture_root();
    let inputs: CommandInputs =
        serde_json::from_slice(&fixture_file(&fixture_root, "cases.json")).unwrap();
    let case = inputs
        .cases
        .iter()
        .find(|case| case.id == "unpaired-surrogate-skill-name")
        .unwrap();
    let receipt: NativeRerunReceipt =
        serde_json::from_slice(&fixture_file(&fixture_root, "native-rerun-receipt.json")).unwrap();
    let native_case = receipt
        .cases
        .iter()
        .find(|native_case| native_case.id == case.id)
        .unwrap();
    let input = fixture_file(&fixture_root, native_case.input_file.as_deref().unwrap());
    let environment = CliFixtureEnvironment::new();

    let (first_output, first_requests) = environment.run_cli(&native_case.argv, &input, &case.id);
    assert_eq!(
        first_output.status.code(),
        Some(native_case.native_exit_code)
    );
    assert_eq!(first_requests.len(), 1);
    let transcript_path = environment.session_path(&case.session_id);
    let first_transcript =
        std::fs::read(&transcript_path).expect("first process saved its session");
    assert!(first_transcript
        .windows(6)
        .any(|window| window == b"\\ud800"));
    let first_units = session_user_content_units(
        std::str::from_utf8(&first_transcript).unwrap(),
        environment.cwd_path.to_str().unwrap(),
    );
    assert!(first_units.iter().any(|units| units.contains(&0xd800)));

    let mut resume_argv = vec!["lingxi-cli".to_owned()];
    let mut skip_value = false;
    for argument in native_case.argv.iter().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(argument.as_str(), "--session-id" | "--input-format") {
            skip_value = true;
            continue;
        }
        resume_argv.push(argument.clone());
    }
    let continuation_prompt = "continue after a cold resume";
    resume_argv.extend([
        "--resume".to_owned(),
        case.session_id.clone(),
        continuation_prompt.to_owned(),
    ]);

    // A fresh CLI process reloads the exact JSONL user units. The Anthropic
    // SDK then applies Native generation's final well-formed-string projection,
    // so the persisted row keeps U+D800 while the sent request contains U+FFFD.
    let (resume_output, resume_requests) = environment.run_cli(&resume_argv, &[], &case.id);
    assert_eq!(
        resume_output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    assert_eq!(resume_requests.len(), 1, "one resumed model request");
    let frames = stream_frames(&resume_output.stdout);
    let init = frames
        .iter()
        .find(|frame| frame["type"] == "system" && frame["subtype"] == "init")
        .expect("resumed stream initializes");
    assert_eq!(init["session_id"].as_str(), Some(case.session_id.as_str()));

    let (_, request_body) = request_parts(&resume_requests[0]);
    let request_wire = std::str::from_utf8(&request_body).expect("Anthropic body is UTF-8 JSON");
    assert!(
        !request_wire.contains("\\ud800"),
        "the final Anthropic body applies Native's well-formed-string projection"
    );
    let request: Value = serde_json::from_slice(&request_body).expect("normalized body is JSON");
    assert_eq!(
        relevant_request_text_blocks(case, &request, environment.cwd_path.to_str().unwrap()),
        vec!["/skill_\u{fffd}_probe"],
        "cold history is normalized at the final Anthropic request boundary"
    );
    assert_eq!(
        request_wire.matches(continuation_prompt).count(),
        1,
        "the continuation prompt appears once"
    );

    let resumed_transcript =
        std::fs::read(&transcript_path).expect("resumed session remains saved");
    assert!(resumed_transcript.len() > first_transcript.len());
    assert!(resumed_transcript
        .windows(6)
        .any(|window| window == b"\\ud800"));
    let resumed_units = session_user_content_units(
        std::str::from_utf8(&resumed_transcript).unwrap(),
        environment.cwd_path.to_str().unwrap(),
    );
    assert!(resumed_units.iter().any(|units| units.contains(&0xd800)));
}
