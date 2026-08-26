//! Hidden `plugin eval` MCP mock runtime.
//!
//! This module is intentionally self-contained so `plugin_eval.rs` can wire it
//! in later without forcing refactors in the existing `mcp` command family.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const SUCCESS: i32 = 0;
const RUNTIME_ERROR: i32 = 1;
const MCP_PROTOCOL_VERSION: &str = mcp::initialize_params::LATEST_PROTOCOL_VERSION;

/// Internal environment variable carrying the mock server spec path.
pub const SPEC_ENV: &str = "LINGXI_INTERNAL_PLUGIN_EVAL_MOCK_SPEC";
/// Internal environment variable carrying the append-only call log path.
pub const CALLS_ENV: &str = "LINGXI_INTERNAL_PLUGIN_EVAL_MOCK_CALLS";

/// Internal stdio MCP mock server configuration used by `plugin eval`.
#[derive(Debug, Clone)]
pub struct Cli {
    /// JSON spec describing the mocked server surface and fixed responders.
    pub spec: PathBuf,

    /// JSONL append-only call log written by the mock runtime.
    pub calls: PathBuf,
}

/// Run the internal mock server when both private launch variables are set.
pub async fn run_from_env() -> Option<i32> {
    let spec = std::env::var_os(SPEC_ENV)?;
    let calls = std::env::var_os(CALLS_ENV)?;
    Some(
        run(&Cli {
            spec: PathBuf::from(spec),
            calls: PathBuf::from(calls),
        })
        .await,
    )
}

/// Root JSON spec for one mocked MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MockServerSpec {
    /// Display name used in serverInfo and call logs.
    #[serde(default = "default_server_name")]
    pub server: String,

    /// Tools advertised by this server.
    #[serde(default)]
    pub tools: Vec<MockToolSpec>,
}

/// One tool entry returned from `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MockToolSpec {
    /// MCP tool name.
    pub name: String,

    /// Optional user-facing description.
    #[serde(default)]
    pub description: String,

    /// JSON Schema object returned in `tools/list`.
    #[serde(rename = "inputSchema", default = "default_input_schema")]
    pub input_schema: Value,

    /// Optional fixed responder for `tools/call`.
    #[serde(default)]
    pub responder: Option<FixedResponderSpec>,
}

/// Fixed responder definition for one tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FixedResponderSpec {
    /// Optional discriminator for future responder kinds.
    #[serde(rename = "type", default = "default_responder_type")]
    pub responder_type: String,

    /// Shorthand for a single text content block.
    #[serde(default)]
    pub body: Option<String>,

    /// Base directory used by `{{file:...}}` substitutions.
    #[serde(default)]
    pub base_dir: Option<PathBuf>,

    /// Full MCP `content` payload as one object or an array of objects.
    #[serde(default)]
    pub content: Option<Value>,

    /// Return `isError: true`.
    #[serde(default)]
    pub error: bool,

    /// Exact expected tool arguments; mismatches are returned as errors.
    #[serde(default)]
    pub expect: Option<Value>,
}

/// Append-only call record written to `--calls`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MockCallRecord {
    /// Mock server name.
    pub server: String,

    /// Tool name.
    pub tool: String,

    /// Tool arguments received from the client.
    pub arguments: Value,

    /// Error summary for failed/mock-error calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Structured abort reason for parent-side aggregation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<String>,

    /// Whether the call targeted an advertised but unimplemented tool.
    #[serde(default)]
    pub unmocked: bool,
}

pub async fn run(cli: &Cli) -> i32 {
    let spec = match load_spec(&cli.spec) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("{error}");
            return RUNTIME_ERROR;
        }
    };

    let log = match CallLog::open(&cli.calls) {
        Ok(log) => log,
        Err(error) => {
            eprintln!("{error}");
            return RUNTIME_ERROR;
        }
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    if let Err(error) = serve_session(reader, stdout, spec, log).await {
        eprintln!("{error}");
        return RUNTIME_ERROR;
    }
    SUCCESS
}

fn load_spec(path: &Path) -> Result<MockServerSpec, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read mock spec {}: {error}", path.display()))?;
    let mut spec = serde_json::from_str::<MockServerSpec>(&raw)
        .map_err(|error| format!("invalid mock spec {}: {error}", path.display()))?;
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for tool in &mut spec.tools {
        if let Some(responder) = &mut tool.responder {
            if let Some(dir) = &responder.base_dir {
                if dir.is_relative() {
                    responder.base_dir = Some(base_dir.join(dir));
                }
            }
        }
    }
    validate_spec(&spec)
        .map_err(|error| format!("invalid mock spec {}: {error}", path.display()))?;
    Ok(spec)
}

fn validate_spec(spec: &MockServerSpec) -> Result<(), String> {
    if spec.server.trim().is_empty() {
        return Err("server name must not be empty".to_string());
    }

    for tool in &spec.tools {
        if tool.name.trim().is_empty() {
            return Err("tool name must not be empty".to_string());
        }
        if !tool.input_schema.is_object() {
            return Err(format!("tool {} inputSchema must be an object", tool.name));
        }
        if let Some(responder) = &tool.responder {
            if responder.responder_type != "fixed" {
                return Err(format!(
                    "tool {} responder type {} is unsupported",
                    tool.name, responder.responder_type
                ));
            }
            if let Some(expect) = &responder.expect {
                if !expect.is_object() {
                    return Err(format!("tool {} expect must be an object", tool.name));
                }
            }
            if let Some(content) = &responder.content {
                match content {
                    Value::Array(items) => {
                        if items.iter().any(|item| !item.is_object()) {
                            return Err(format!(
                                "tool {} content arrays must contain objects",
                                tool.name
                            ));
                        }
                    }
                    Value::Object(_) => {}
                    _ => {
                        return Err(format!(
                            "tool {} content must be an object or array",
                            tool.name
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

async fn serve_session<R, W>(
    mut reader: R,
    mut writer: W,
    spec: MockServerSpec,
    mut log: CallLog,
) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut initialize_seen = false;
    let mut client_initialized = false;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("read stdin: {error}"))?;
        if bytes == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = jsonrpc_error(
                    Value::Null,
                    -32700,
                    "Parse error",
                    Some(json!({ "detail": error.to_string() })),
                );
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
                continue;
            }
        };

        let Some(object) = message.as_object() else {
            let response = jsonrpc_error(Value::Null, -32600, "Invalid Request", None);
            write_protocol_frame(&mut writer, &response)
                .await
                .map_err(|error| error.to_string())?;
            continue;
        };

        let request_id = object.get("id").cloned();
        let method = object.get("method").and_then(Value::as_str);
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_none() {
            if let Some(id) = request_id {
                let response = jsonrpc_error(id, -32600, "Invalid Request", None);
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            continue;
        }

        let method = method.unwrap_or_default();
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "notifications/initialized" => {
                if initialize_seen {
                    client_initialized = true;
                }
            }
            _ if request_id.is_none() => {}
            "initialize" => {
                let id = request_id.unwrap_or(Value::Null);
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let response = if initialize_seen {
                    jsonrpc_error(id, -32600, "Server is already initialized", None)
                } else if requested != Some(MCP_PROTOCOL_VERSION) {
                    jsonrpc_error(
                        id,
                        -32602,
                        "Unsupported protocol version",
                        Some(json!({
                            "supported": [MCP_PROTOCOL_VERSION],
                            "requested": requested,
                        })),
                    )
                } else {
                    initialize_seen = true;
                    initialize_response(id, &spec.server)
                };
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            "ping" => {
                let response = jsonrpc_result(request_id.unwrap_or(Value::Null), json!({}));
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            _ if !client_initialized => {
                let response = jsonrpc_error(
                    request_id.unwrap_or(Value::Null),
                    -32002,
                    "Server is not initialized",
                    None,
                );
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            "tools/list" => {
                let tools = spec
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "inputSchema": tool.input_schema,
                        })
                    })
                    .collect::<Vec<_>>();
                let response =
                    jsonrpc_result(request_id.unwrap_or(Value::Null), json!({ "tools": tools }));
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            "tools/call" => {
                let id = request_id.unwrap_or(Value::Null);
                let response = handle_tool_call(&spec, &mut log, id, params)?;
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            _ => {
                let response = jsonrpc_error(
                    request_id.unwrap_or(Value::Null),
                    -32601,
                    "Method not found",
                    Some(json!({ "method": method })),
                );
                write_protocol_frame(&mut writer, &response)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
    }

    Ok(())
}

fn handle_tool_call(
    spec: &MockServerSpec,
    log: &mut CallLog,
    request_id: Value,
    params: Value,
) -> Result<Value, String> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Ok(jsonrpc_error(request_id, -32602, "Missing tool name", None));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Ok(jsonrpc_error(
            request_id,
            -32602,
            "Tool arguments must be an object",
            None,
        ));
    }

    let Some(tool) = spec.tools.iter().find(|tool| tool.name == name) else {
        return Ok(jsonrpc_error(
            request_id,
            -32602,
            &format!("Unknown tool: {name}"),
            None,
        ));
    };

    let (content, is_error, error, abort_reason, unmocked) = match &tool.responder {
        None => {
            let error = format!("Unmocked tool call: {}.{}", spec.server, tool.name);
            eprintln!("{error}");
            (text_content(&error), true, Some(error), None, true)
        }
        Some(responder) => {
            if let Some(expect) = &responder.expect {
                if let Err(reason) = expect_arguments(expect, &arguments) {
                    let error = format!(
                        "Mock expectation mismatch for {}.{}: {reason}",
                        spec.server, tool.name,
                    );
                    eprintln!("{error}");
                    (
                        text_content(&error),
                        true,
                        Some(error),
                        Some("expect_mismatch".to_string()),
                        false,
                    )
                } else {
                    let content = responder_content(responder, &arguments)?;
                    let error = responder.error.then(|| {
                        format!(
                            "Mock responder returned error for {}.{}",
                            spec.server, tool.name
                        )
                    });
                    if let Some(message) = &error {
                        eprintln!("{message}");
                    }
                    (content, responder.error, error, None, false)
                }
            } else {
                let content = responder_content(responder, &arguments)?;
                let error = responder.error.then(|| {
                    format!(
                        "Mock responder returned error for {}.{}",
                        spec.server, tool.name
                    )
                });
                if let Some(message) = &error {
                    eprintln!("{message}");
                }
                (content, responder.error, error, None, false)
            }
        }
    };

    log.append(&MockCallRecord {
        server: spec.server.clone(),
        tool: tool.name.clone(),
        arguments,
        error: error.clone(),
        abort_reason,
        unmocked,
    })?;

    Ok(jsonrpc_result(
        request_id,
        json!({
            "content": content,
            "isError": is_error,
        }),
    ))
}

fn expect_arguments(expect: &Value, arguments: &Value) -> Result<(), String> {
    let expected = expect
        .as_object()
        .ok_or_else(|| "expect must be an object".to_string())?;
    for (path, predicate) in expected {
        let actual = value_at_dotted_path(arguments, path)
            .ok_or_else(|| format!("missing input path {path:?}"))?;
        if !expect_predicate_matches(predicate, actual)? {
            return Err(format!(
                "input path {path:?} did not satisfy {}",
                serde_json::to_string(predicate).unwrap_or_else(|_| "the expectation".to_string())
            ));
        }
    }
    Ok(())
}

fn value_at_dotted_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(object) => object.get(segment)?,
            Value::Array(array) => array.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn expect_predicate_matches(predicate: &Value, actual: &Value) -> Result<bool, String> {
    match predicate {
        Value::String(expected) => match expected.as_str() {
            "string" => Ok(actual.is_string()),
            "number" => Ok(actual.is_number()),
            "boolean" => Ok(actual.is_boolean()),
            "array" => Ok(actual.is_array()),
            "object" => Ok(actual.is_object()),
            _ if regex_literal_parts(expected).is_some() => {
                let Some(actual) = scalar_text(actual) else {
                    return Ok(false);
                };
                regex_expectation_matches(expected, &actual)
            }
            _ => Ok(scalar_text(actual).as_deref() == Some(expected.as_str())),
        },
        Value::Array(allowed) => {
            let Some(actual) = scalar_text(actual) else {
                return Ok(false);
            };
            Ok(allowed
                .iter()
                .filter_map(scalar_text)
                .any(|candidate| candidate == actual))
        }
        Value::Number(_) | Value::Bool(_) => Ok(scalar_text(predicate) == scalar_text(actual)),
        _ => Ok(predicate == actual),
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn regex_literal_parts(value: &str) -> Option<(&str, &str)> {
    if !value.starts_with('/') || value.len() < 2 {
        return None;
    }
    let end = value.rfind('/')?;
    (end > 0).then(|| (&value[1..end], &value[end + 1..]))
}

fn regex_expectation_matches(literal: &str, actual: &str) -> Result<bool, String> {
    const MAX_REGEX_INPUT_BYTES: usize = 16 * 1024;
    if actual.len() > MAX_REGEX_INPUT_BYTES {
        return Ok(false);
    }
    let (pattern, flags) = regex_literal_parts(literal)
        .ok_or_else(|| format!("invalid regex expectation {literal:?}"))?;
    if flags.bytes().any(|flag| !matches!(flag, b'i' | b's')) {
        return Err(format!("unsupported regex flags in {literal:?}"));
    }
    if pattern.contains('(')
        || pattern.contains(')')
        || pattern.contains('|')
        || pattern.contains("(?")
        || pattern
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'\\' && pair[1].is_ascii_digit())
    {
        return Err(format!("unsupported regex construct in {literal:?}"));
    }
    let mut compiled = String::new();
    if flags.contains('i') {
        compiled.push_str("(?i)");
    }
    if flags.contains('s') {
        compiled.push_str("(?s)");
    }
    compiled.push_str(pattern);
    regex::Regex::new(&compiled)
        .map(|regex| regex.is_match(actual))
        .map_err(|error| format!("invalid regex expectation {literal:?}: {error}"))
}

fn responder_content(
    responder: &FixedResponderSpec,
    arguments: &Value,
) -> Result<Vec<Value>, String> {
    if let Some(content) = &responder.content {
        return match content {
            Value::Array(items) => Ok(items.clone()),
            Value::Object(_) => Ok(vec![content.clone()]),
            _ => Err("responder content must be an object or array".to_string()),
        };
    }
    if let Some(body) = &responder.body {
        return Ok(text_content(&render_body_template(
            body,
            responder.base_dir.as_deref(),
            arguments,
        )?));
    }
    Ok(text_content(""))
}

fn render_body_template(
    template: &str,
    base_dir: Option<&Path>,
    arguments: &Value,
) -> Result<String, String> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err("unterminated template placeholder".to_string());
        };
        let expression = after_start[..end].trim();
        let replacement = if let Some(path_template) = expression.strip_prefix("file:") {
            let resolved_path = resolve_file_template(path_template, arguments)?;
            let base_dir =
                base_dir.ok_or_else(|| "file template requires responder.baseDir".to_string())?;
            let full_path = base_dir.join(&resolved_path);
            ensure_within_base_dir(base_dir, &full_path)?;
            fs::read_to_string(&full_path).map_err(|error| {
                format!("failed to read fixture {}: {error}", full_path.display())
            })?
        } else {
            lookup_input_template(expression, arguments)?
        };
        rendered.push_str(&replacement);
        rest = &after_start[end + 2..];
    }

    rendered.push_str(rest);
    Ok(rendered)
}

fn lookup_input_template(expression: &str, arguments: &Value) -> Result<String, String> {
    let path = expression
        .strip_prefix("input.")
        .ok_or_else(|| format!("unsupported template expression: {expression}"))?;
    let mut current = arguments;
    for segment in path.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| format!("missing template input: {expression}"))?;
    }
    match current {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok("null".to_string()),
        _ => Err(format!("template input must be scalar: {expression}")),
    }
}

fn resolve_file_template(path_template: &str, arguments: &Value) -> Result<PathBuf, String> {
    let mut rendered = String::with_capacity(path_template.len());
    let mut rest = path_template;

    while let Some(start) = rest.find("{input.") {
        rendered.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err("unterminated file template placeholder".to_string());
        };
        let expression = &after_start[..end];
        let value = lookup_input_template(expression, arguments)?;
        if !is_safe_segment(&value) {
            return Err(format!("unsafe file template segment: {value}"));
        }
        rendered.push_str(&value);
        rest = &after_start[end + 1..];
    }

    rendered.push_str(rest);
    Ok(PathBuf::from(rendered))
}

fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn ensure_within_base_dir(base_dir: &Path, candidate: &Path) -> Result<(), String> {
    let full = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to resolve fixture path {}: {error}",
            candidate.display()
        )
    })?;
    let root = base_dir
        .canonicalize()
        .map_err(|error| format!("failed to resolve base dir {}: {error}", base_dir.display()))?;
    if full.starts_with(&root) {
        Ok(())
    } else {
        Err(format!(
            "fixture path escapes base dir: {}",
            candidate.display()
        ))
    }
}

fn text_content(text: &str) -> Vec<Value> {
    vec![json!({ "type": "text", "text": text })]
}

fn initialize_response(id: Value, server: &str) -> Value {
    jsonrpc_result(
        id,
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": server,
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn jsonrpc_error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

async fn write_protocol_frame<W>(writer: &mut W, frame: &Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

fn default_server_name() -> String {
    "mock".to_string()
}

fn default_responder_type() -> String {
    "fixed".to_string()
}

fn default_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": true,
    })
}

struct CallLog {
    file: std::fs::File,
}

impl CallLog {
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create call log directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let file = open_append_only(path)
            .map_err(|error| format!("failed to open call log {}: {error}", path.display()))?;
        set_private_permissions(path)?;
        Ok(Self { file })
    }

    fn append(&mut self, record: &MockCallRecord) -> Result<(), String> {
        serde_json::to_writer(&mut self.file, record)
            .map_err(|error| format!("failed to encode mock call record: {error}"))?;
        self.file
            .write_all(b"\n")
            .map_err(|error| format!("failed to append mock call record: {error}"))?;
        self.file
            .flush()
            .map_err(|error| format!("failed to flush mock call log: {error}"))
    }
}

fn open_append_only(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new().create(true).append(true).open(path)
    }
}

fn set_private_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "failed to set call log permissions {}: {error}",
                path.display()
            )
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt};

    fn protocol_input(extra: Value) -> String {
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        format!(
            "{}\n{}\n{}\n",
            initialize,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            extra
        )
    }

    #[tokio::test]
    async fn initialize_accepts_production_client_protocol_version() {
        let production_protocol = mcp::InitializeParams::default().protocol_version;
        let input = format!(
            "{}\n{}\n",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": production_protocol,
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
        );

        let (frames, calls) = run_protocol(basic_spec(None), input).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["result"]["protocolVersion"], production_protocol);
        assert!(calls.is_empty());
    }

    async fn run_protocol(
        spec: MockServerSpec,
        input: String,
    ) -> (Vec<Value>, Vec<MockCallRecord>) {
        let temp = tempfile::tempdir().unwrap();
        let calls_path = temp.path().join("calls.jsonl");
        let log = CallLog::open(&calls_path).unwrap();

        let (client, server) = tokio::io::duplex(8192);
        let (mut client_reader, mut client_writer) = tokio::io::split(client);
        let (server_reader, server_writer) = tokio::io::split(server);
        let handle = tokio::spawn(async move {
            serve_session(BufReader::new(server_reader), server_writer, spec, log)
                .await
                .unwrap();
        });

        client_writer.write_all(input.as_bytes()).await.unwrap();
        AsyncWriteExt::shutdown(&mut client_writer).await.unwrap();

        let mut stdout = String::new();
        client_reader.read_to_string(&mut stdout).await.unwrap();
        handle.await.unwrap();

        let frames = stdout
            .lines()
            .map(|line| Value::from_str(line).unwrap())
            .collect::<Vec<_>>();
        let calls_raw = fs::read_to_string(&calls_path).unwrap();
        let calls = calls_raw
            .lines()
            .map(|line| serde_json::from_str::<MockCallRecord>(line).unwrap())
            .collect::<Vec<_>>();
        (frames, calls)
    }

    fn basic_spec(responder: Option<FixedResponderSpec>) -> MockServerSpec {
        MockServerSpec {
            server: "demo".to_string(),
            tools: vec![MockToolSpec {
                name: "lookup".to_string(),
                description: "Lookup data".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "summary": { "type": "string" }, "channel": { "type": "string" } },
                    "required": ["summary", "channel"],
                    "additionalProperties": false
                }),
                responder,
            }],
        }
    }

    #[tokio::test]
    async fn tools_list_returns_declared_metadata() {
        let input = protocol_input(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }));
        let (frames, calls) = run_protocol(basic_spec(None), input).await;
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(frames[1]["result"]["tools"][0]["name"], "lookup");
        assert_eq!(
            frames[1]["result"]["tools"][0]["description"],
            "Lookup data"
        );
        assert_eq!(
            frames[1]["result"]["tools"][0]["inputSchema"]["required"],
            json!(["summary", "channel"])
        );
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn fixed_success_returns_content_and_logs_call() {
        let temp = tempfile::tempdir().unwrap();
        let fixtures = temp.path().join("fixtures");
        fs::create_dir_all(&fixtures).unwrap();
        fs::write(fixtures.join("alerts.json"), "{\"ok\":true}\n").unwrap();
        let input = protocol_input(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup",
                "arguments": { "summary": "hello", "channel": "alerts" }
            }
        }));
        let (frames, calls) = run_protocol(
            basic_spec(Some(FixedResponderSpec {
                responder_type: "fixed".to_string(),
                body: Some(
                    "summary={{input.summary}} fixture={{file:fixtures/{input.channel}.json}}"
                        .to_string(),
                ),
                base_dir: Some(temp.path().to_path_buf()),
                content: None,
                error: false,
                expect: None,
            })),
            input,
        )
        .await;
        assert_eq!(frames[1]["result"]["isError"], false);
        assert_eq!(
            frames[1]["result"]["content"][0]["text"],
            "summary=hello fixture={\"ok\":true}\n"
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "demo");
        assert_eq!(calls[0].tool, "lookup");
        assert_eq!(
            calls[0].arguments,
            json!({ "summary": "hello", "channel": "alerts" })
        );
        assert_eq!(calls[0].error, None);
        assert_eq!(calls[0].abort_reason, None);
        assert!(!calls[0].unmocked);
    }

    #[tokio::test]
    async fn fixed_error_returns_is_error_and_logs_error() {
        let input = protocol_input(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup",
                "arguments": { "summary": "hello", "channel": "alerts" }
            }
        }));
        let (frames, calls) = run_protocol(
            basic_spec(Some(FixedResponderSpec {
                responder_type: "fixed".to_string(),
                body: Some("bad".to_string()),
                base_dir: None,
                content: None,
                error: true,
                expect: None,
            })),
            input,
        )
        .await;
        assert_eq!(frames[1]["result"]["isError"], true);
        assert_eq!(frames[1]["result"]["content"][0]["text"], "bad");
        assert_eq!(
            calls[0].error.as_deref(),
            Some("Mock responder returned error for demo.lookup")
        );
        assert!(!calls[0].unmocked);
    }

    #[test]
    fn expect_supports_dotted_paths_types_regex_and_literal_lists() {
        let arguments = json!({
            "summary": "HELLO-42",
            "labels": ["urgent", "backend"],
            "count": 2,
            "enabled": true,
            "literalRegex": "/not-a-pattern/"
        });
        expect_arguments(
            &json!({
                "summary": "/^hello-[0-9]+$/i",
                "labels": "array",
                "labels.0": ["normal", "urgent"],
                "count": 2,
                "enabled": true,
                "literalRegex": ["/not-a-pattern/"]
            }),
            &arguments,
        )
        .unwrap();
        assert!(expect_arguments(&json!({"summary": "/(hello|bye)/"}), &arguments).is_err());
        assert!(expect_arguments(&json!({"labels.9": "string"}), &arguments).is_err());
    }

    #[tokio::test]
    async fn expect_mismatch_returns_error_and_logs_mismatch() {
        let input = protocol_input(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup",
                "arguments": { "summary": "unexpected", "channel": "alerts" }
            }
        }));
        let (frames, calls) = run_protocol(
            basic_spec(Some(FixedResponderSpec {
                responder_type: "fixed".to_string(),
                body: Some("ok".to_string()),
                base_dir: None,
                content: None,
                error: false,
                expect: Some(json!({ "summary": "expected", "channel": "alerts" })),
            })),
            input,
        )
        .await;
        assert_eq!(frames[1]["result"]["isError"], true);
        let message = frames[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(message.starts_with("Mock expectation mismatch for demo.lookup:"));
        assert_eq!(calls[0].error.as_deref(), Some(message));
        assert_eq!(calls[0].abort_reason.as_deref(), Some("expect_mismatch"));
        assert!(!calls[0].unmocked);
    }

    #[tokio::test]
    async fn unmocked_tool_returns_error_and_logs_unmocked() {
        let input = protocol_input(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup",
                "arguments": { "summary": "hello", "channel": "alerts" }
            }
        }));
        let (frames, calls) = run_protocol(basic_spec(None), input).await;
        assert_eq!(frames[1]["result"]["isError"], true);
        assert_eq!(
            frames[1]["result"]["content"][0]["text"],
            "Unmocked tool call: demo.lookup"
        );
        assert_eq!(
            calls[0].error.as_deref(),
            Some("Unmocked tool call: demo.lookup")
        );
        assert!(calls[0].unmocked);
    }

    #[test]
    fn validate_spec_rejects_agent_responder_type() {
        let spec = basic_spec(Some(FixedResponderSpec {
            responder_type: "agent".to_string(),
            body: Some("ignored".to_string()),
            base_dir: None,
            content: None,
            error: false,
            expect: None,
        }));
        assert_eq!(
            validate_spec(&spec).unwrap_err(),
            "tool lookup responder type agent is unsupported"
        );
    }
}
