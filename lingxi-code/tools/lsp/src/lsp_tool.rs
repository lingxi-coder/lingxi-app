//! LSP builtin tool — `LSPTool` exposes 9 operations (goToDefinition /
//! findReferences / hover / documentSymbol / workspaceSymbol /
//! goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls)
//! over `lsp::LspClient` (M2-03; surface realigned to `LSPTool.ts:62-72`).
//!
//! Input position is 1-based (claude-code UI convention); we convert to
//! 0-based via `lsp::tool_operations::position_from_one_based`
//! before issuing the LSP request.
//!
//! Wire identifiers locked in spec §7 line 695.
//!
//! no-truncation: LSPTool returns structured definition / references / hover /
//! symbol / call-hierarchy payloads forwarded verbatim from lsp::LspClient (the
//! upstream LSP server is the trust boundary). Free-form text comes only
//! from hover contents, which are bounded by the LSP protocol itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lsp::registry::LspRegistry;
use lsp::tool_operations as ops;
use lsp::OpenFileTracker;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{LSP_COMPLETED, LSP_FAILED, LSP_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

// -- Wire identifier locks (spec §7 line 695) --------------------------------

/// Registry name for the LSP tool (the Rust struct keeps the
/// `LSPTool` identifier).
pub const LSP_TOOL_NAME: &str = "LSP";

/// Operation literal (`textDocument/definition`).
pub const LSP_OPERATION_GO_TO_DEFINITION: &str = "goToDefinition";
/// Operation literal (`textDocument/references`).
pub const LSP_OPERATION_FIND_REFERENCES: &str = "findReferences";
/// Operation literal (`textDocument/hover`).
pub const LSP_OPERATION_HOVER: &str = "hover";
/// Operation literal (`textDocument/documentSymbol`).
pub const LSP_OPERATION_DOCUMENT_SYMBOL: &str = "documentSymbol";
/// Operation literal (`workspace/symbol`).
pub const LSP_OPERATION_WORKSPACE_SYMBOL: &str = "workspaceSymbol";
/// Operation literal (`textDocument/implementation`).
pub const LSP_OPERATION_GO_TO_IMPLEMENTATION: &str = "goToImplementation";
/// Operation literal (`textDocument/prepareCallHierarchy`).
pub const LSP_OPERATION_PREPARE_CALL_HIERARCHY: &str = "prepareCallHierarchy";
/// Operation literal (`callHierarchy/incomingCalls`).
pub const LSP_OPERATION_INCOMING_CALLS: &str = "incomingCalls";
/// Operation literal (`callHierarchy/outgoingCalls`).
pub const LSP_OPERATION_OUTGOING_CALLS: &str = "outgoingCalls";

/// All nine operations in the locked surface order (`LSPTool.ts:62-72`).
pub const LSP_OPERATIONS_LOCKED: [&str; 9] = [
    LSP_OPERATION_GO_TO_DEFINITION,
    LSP_OPERATION_FIND_REFERENCES,
    LSP_OPERATION_HOVER,
    LSP_OPERATION_DOCUMENT_SYMBOL,
    LSP_OPERATION_WORKSPACE_SYMBOL,
    LSP_OPERATION_GO_TO_IMPLEMENTATION,
    LSP_OPERATION_PREPARE_CALL_HIERARCHY,
    LSP_OPERATION_INCOMING_CALLS,
    LSP_OPERATION_OUTGOING_CALLS,
];

/// Position-validation error literal (LingXi lock; ASCII `>=` form).
pub const LSP_POSITION_ERROR: &str = "LSP position must be 1-based (line >= 1, character >= 1)";

// -- Helpers -----------------------------------------------------------------

fn pii(s: &str) -> AnalyticsValue {
    use telemetry::pii::PiiTagged;
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}
fn verified_int(n: u64) -> AnalyticsValue {
    AnalyticsValue::Int(n as i64)
}
fn verified_str(s: &str) -> AnalyticsValue {
    use telemetry::pii::Verified;
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit(bus: &Arc<AnalyticsBus>, event: &'static str, fields: &[(&str, AnalyticsValue)]) {
    let mut md: LogEventMetadata = HashMap::new();
    for (k, v) in fields {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

fn allow_lsp() -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::Other {
            reason: "LSP read-only operation".into(),
        },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

// -- Path expansion (`utils/path.ts:expandPath`) -----------------------------

/// User home directory — the `os.homedir()` equivalent. `tool-lsp` does not
/// depend on the `dirs` crate, so we resolve via the conventional env vars
/// (`HOME` on posix, `USERPROFILE` on Windows).
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(PathBuf::new, PathBuf::from)
}

/// Current working directory — the `getCwd()` equivalent.
fn current_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The graceful "file too large" message from `LSPTool.ts:268`. The size is
/// reported in MB via `Math.ceil(size / 1_000_000)` — NOT in bytes.
fn file_too_large_message(size: u64) -> String {
    let mb = size.div_ceil(1_000_000);
    format!("File too large for LSP analysis ({mb}MB exceeds 10MB limit)")
}

/// Core of `validateInput` (`LSPTool.ts:166-208`), split out for direct
/// testing: confirm the (expanded) path exists and is a regular file, with
/// byte-exact TS messages. Returns `Ok(())` for UNC paths (skipped for the
/// NTLM-leak security reason in the TS source).
fn validate_file_path(file_path: &str) -> Result<(), ValidationError> {
    let absolute_path = expand_path(file_path);
    let display = absolute_path.to_string_lossy();
    if display.starts_with("\\\\") || display.starts_with("//") {
        return Ok(());
    }
    match std::fs::metadata(&absolute_path) {
        Ok(stats) => {
            if stats.is_file() {
                Ok(())
            } else {
                Err(ValidationError(format!("Path is not a file: {file_path}")))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(ValidationError(format!("File does not exist: {file_path}")))
        }
        Err(err) => Err(ValidationError(format!(
            "Cannot access file: {file_path}. {err}"
        ))),
    }
}

/// Expand a user-supplied path the way `utils/path.ts:expandPath` does:
/// leading `~` -> home, `~/x` -> home/x, absolute -> unchanged, relative ->
/// resolved against the current working directory. Mirrors the trim and
/// empty-path handling of the TS implementation; the Windows POSIX-path
/// conversion branch is not relevant on the target platform.
fn expand_path(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return current_cwd();
    }
    if trimmed == "~" {
        return home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    current_cwd().join(candidate)
}

// -- gitignore filtering (`LSPTool.ts:336-374`, `filterGitIgnoredLocations`) --

/// Percent-decode a `file://` path body, mirroring `decodeURIComponent`.
/// Returns `None` on malformed input so the caller can fall back to the raw
/// (un-decoded) path, exactly as the TS `try/catch` does.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(u8::try_from(hi * 16 + lo).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Extract a filesystem path from a `file://` URI, decoding percent-encoded
/// characters. Mirrors `LSPTool.ts:uriToFilePath`.
fn uri_to_file_path(uri: &str) -> String {
    let mut file_path = uri.strip_prefix("file://").unwrap_or(uri).to_string();
    // On Windows, file:///C:/path becomes /C:/path — strip the leading slash.
    let b = file_path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        file_path = file_path[1..].to_string();
    }
    percent_decode(&file_path).unwrap_or(file_path)
}

/// `toLocation(item).uri` for a `Location` (`uri`) or `LocationLink`
/// (`targetUri`). Returns `None` when the URI is absent/non-string (the TS
/// `!loc.uri` keep-case).
fn location_uri(item: &Value) -> Option<&str> {
    if item.get("targetUri").is_some() {
        return item.get("targetUri").and_then(Value::as_str);
    }
    item.get("uri").and_then(Value::as_str)
}

/// `SymbolInformation.location.uri`.
fn symbol_location_uri(item: &Value) -> Option<&str> {
    item.get("location")
        .and_then(|l| l.get("uri"))
        .and_then(Value::as_str)
}

/// Run `git check-ignore <paths...>` in `cwd`. Returns the stdout only when
/// git exits 0 (≥1 path ignored). Exit 1 (none ignored) and 128 (not a repo)
/// yield `None`. Matches `execFileNoThrowWithCwd('git', ['check-ignore', …])`.
fn run_git_check_ignore(paths: &[String], cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("check-ignore")
        .args(paths)
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.code() == Some(0) {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Compute the set of URIs whose backing file is gitignored, batching the
/// `git check-ignore` calls (50 paths per invocation) exactly like
/// `filterGitIgnoredLocations`.
fn ignored_uri_set(uris: &[&str], cwd: &Path) -> HashSet<String> {
    let mut ignored_uris: HashSet<String> = HashSet::new();
    // Unique URI -> filesystem path.
    let mut uri_to_path: Vec<(String, String)> = Vec::new();
    let mut seen_uri: HashSet<&str> = HashSet::new();
    for &uri in uris {
        if !uri.is_empty() && seen_uri.insert(uri) {
            uri_to_path.push((uri.to_string(), uri_to_file_path(uri)));
        }
    }
    // Unique paths (preserve first-seen order).
    let mut unique_paths: Vec<String> = Vec::new();
    let mut seen_path: HashSet<&str> = HashSet::new();
    for (_, p) in &uri_to_path {
        if seen_path.insert(p.as_str()) {
            unique_paths.push(p.clone());
        }
    }
    if unique_paths.is_empty() {
        return ignored_uris;
    }
    // Batch-check paths; collect the absolute paths git reports as ignored.
    let mut ignored_paths: HashSet<String> = HashSet::new();
    for batch in unique_paths.chunks(50) {
        if let Some(stdout) = run_git_check_ignore(batch, cwd) {
            for line in stdout.split('\n') {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    ignored_paths.insert(trimmed.to_string());
                }
            }
        }
    }
    if ignored_paths.is_empty() {
        return ignored_uris;
    }
    for (uri, p) in uri_to_path {
        if ignored_paths.contains(&p) {
            ignored_uris.insert(uri);
        }
    }
    ignored_uris
}

/// Filter gitignored files out of a location-bearing LSP result, mirroring
/// `LSPTool.ts:336-374`. Only array results of `findReferences`,
/// `goToDefinition`, `goToImplementation`, and `workspaceSymbol` are touched;
/// everything else is returned unchanged. Items whose URI is absent are kept.
fn filter_gitignored_results(operation: &str, raw: Value, cwd: &Path) -> Value {
    let is_location_op = matches!(
        operation,
        "findReferences" | "goToDefinition" | "goToImplementation" | "workspaceSymbol"
    );
    if !is_location_op {
        return raw;
    }
    let Value::Array(items) = &raw else {
        return raw;
    };
    let is_workspace_symbol = operation == "workspaceSymbol";
    let extract = |item: &Value| -> Option<String> {
        if is_workspace_symbol {
            symbol_location_uri(item)
        } else {
            location_uri(item)
        }
        .map(ToString::to_string)
    };
    let uris: Vec<String> = items.iter().filter_map(&extract).collect();
    let uri_refs: Vec<&str> = uris.iter().map(String::as_str).collect();
    let ignored = ignored_uri_set(&uri_refs, cwd);
    if ignored.is_empty() {
        return raw;
    }
    let filtered: Vec<Value> = items
        .iter()
        .filter(|item| extract(item).is_none_or(|uri| !ignored.contains(&uri)))
        .cloned()
        .collect();
    Value::Array(filtered)
}

// -- Tool struct -------------------------------------------------------------

/// Builtin tool — dispatches 4 LSP operations.
pub struct LSPTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl LSPTool {
    /// Construct a new [`LSPTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
    fn lsp_registry(&self) -> Option<&Arc<LspRegistry>> {
        self.ctx.lsp_registry.as_ref()
    }
}

static LSP_TOOL_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "operation":   { "type": "string", "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol", "goToImplementation", "prepareCallHierarchy", "incomingCalls", "outgoingCalls"] },
            "server_name": { "type": "string", "minLength": 1 },
            "file_path":   { "type": "string", "minLength": 1 },
            "line":        { "type": "integer", "minimum": 1 },
            "character":   { "type": "integer", "minimum": 1 }
        },
        "required": ["operation", "server_name", "file_path", "line", "character"]
    })
});

#[async_trait]
impl Tool for LSPTool {
    fn name(&self) -> &str {
        LSP_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &LSP_TOOL_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn is_lsp(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        30_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    fn get_path(&self, input: &Value) -> Option<PathBuf> {
        // `LSPTool.ts:152-154` `getPath({ filePath }) => expandPath(filePath)`.
        let file_path = input.get("file_path").and_then(Value::as_str)?;
        Some(expand_path(file_path))
    }

    /// Port of `LSPTool.ts:155-209` `validateInput`: confirm the (expanded)
    /// path exists and is a regular file. Byte-exact TS messages.
    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        validate_file_path(file_path)
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_lsp()
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "LSP-server operations (goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls). 1-based positions."
            .into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use LSP to query LSP servers at a 1-based (line, character) position.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let operation = input
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string operation".into())
            })?
            .to_string();
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string server_name".into())
            })?
            .to_string();
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string file_path".into())
            })?
            .to_string();
        let line = input
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ToolError::InvalidInput("LSPTool: missing or non-integer line".into()))?
            as u32;
        let character = input
            .get("character")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-integer character".into())
            })? as u32;

        // 1-based position guard — emits no STARTED to keep paired events.
        if line < 1 || character < 1 {
            return Err(ToolError::InvalidInput(LSP_POSITION_ERROR.into()));
        }

        // Unknown-operation guard — also pre-STARTED.
        if !LSP_OPERATIONS_LOCKED.contains(&operation.as_str()) {
            return Err(ToolError::InvalidInput(format!(
                "LSPTool: unknown operation {operation:?}; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
            )));
        }

        let bus = &self.ctx.bus;
        emit(
            bus,
            LSP_STARTED,
            &[
                ("_PROTO_operation", pii(&operation)),
                ("_PROTO_server_name", pii(&server_name)),
                ("_PROTO_file_path", pii(&file_path)),
                ("line", verified_int(line as u64)),
                ("character", verified_int(character as u64)),
            ],
        )
        .await;

        let registry = match self.lsp_registry() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "LSPTool: LSP registry not configured on this host".into(),
                ));
            }
        };

        let client = match registry.get_client(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_running")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "LSP server '{server_name}' is not running"
                )));
            }
        };

        let config = match registry.get_config(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_running")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "LSP server '{server_name}' is not running"
                )));
            }
        };

        let tracker = OpenFileTracker::new();
        // `LSPTool.ts:225` `expandPath(input.filePath)` — tilde / relative path
        // resolution before the file is opened.
        let expanded = expand_path(&file_path);
        let path = expanded.as_path();
        // `getCwd()` for the gitignore filter (`LSPTool.ts:226`).
        let cwd = current_cwd();

        // `LSPTool.ts:427-` `getMethodAndParams` — per-operation dispatch. Position-
        // based ops use `line`/`character`; `documentSymbol` is file-level;
        // `workspaceSymbol` ignores the file and queries with an empty string
        // ("returns all symbols", `LSPTool.ts:475`); the call-hierarchy ops do the
        // two-step prepare-then-calls round-trip inside their `ops::` impl.
        let res = match operation.as_str() {
            "goToDefinition" => {
                ops::go_to_definition(&client, &tracker, &config, path, line, character).await
            }
            "findReferences" => {
                ops::find_references(&client, &tracker, &config, path, line, character, true).await
            }
            "hover" => ops::hover(&client, &tracker, &config, path, line, character).await,
            "documentSymbol" => ops::document_symbol(&client, &tracker, &config, path).await,
            "workspaceSymbol" => ops::workspace_symbol(&client, None).await,
            "goToImplementation" => {
                ops::go_to_implementation(&client, &tracker, &config, path, line, character).await
            }
            "prepareCallHierarchy" => {
                ops::prepare_call_hierarchy(&client, &tracker, &config, path, line, character).await
            }
            "incomingCalls" => {
                ops::incoming_calls(&client, &tracker, &config, path, line, character).await
            }
            "outgoingCalls" => {
                ops::outgoing_calls(&client, &tracker, &config, path, line, character).await
            }
            _ => unreachable!("validated above"),
        };

        match res {
            Ok(r) => {
                emit(
                    bus,
                    LSP_COMPLETED,
                    &[
                        ("_PROTO_operation", pii(&operation)),
                        ("_PROTO_server_name", pii(&server_name)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                    ],
                )
                .await;
                // `LSPTool.ts:336-374` — drop gitignored files from
                // location-bearing array results before returning.
                let result = filter_gitignored_results(&operation, r.raw, &cwd);
                Ok(ToolCallResult {
                    data: json!({
                        "operation": operation,
                        "server_name": server_name,
                        "file_path": file_path,
                        "result": result,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            // `LSPTool.ts:265-272` — a file over the 10 MB cap is NOT an error;
            // it returns a graceful success whose `result` states the size in
            // MB (`Math.ceil(size / 1_000_000)`), not a hard byte error.
            Err(ops::LspOperationError::FileTooLarge { size, .. }) => {
                emit(
                    bus,
                    LSP_COMPLETED,
                    &[
                        ("_PROTO_operation", pii(&operation)),
                        ("_PROTO_server_name", pii(&server_name)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                    ],
                )
                .await;
                let result = file_too_large_message(size);
                Ok(ToolCallResult {
                    data: json!({
                        "operation": operation,
                        "server_name": server_name,
                        "file_path": file_path,
                        "result": result,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_operation", pii(&operation)),
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("operation")),
                    ],
                )
                .await;
                Err(ToolError::Io(format!(
                    "LSPTool: {server_name} {operation}: {e}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_name_locked() {
        assert_eq!(LSP_TOOL_NAME, "LSP");
    }

    #[test]
    fn lsp_operations_locked_array_matches_constants() {
        // Locked to the TS surface order (`LSPTool.ts:62-72`).
        assert_eq!(
            LSP_OPERATIONS_LOCKED,
            [
                "goToDefinition",
                "findReferences",
                "hover",
                "documentSymbol",
                "workspaceSymbol",
                "goToImplementation",
                "prepareCallHierarchy",
                "incomingCalls",
                "outgoingCalls"
            ]
        );
        assert_eq!(LSP_OPERATION_GO_TO_DEFINITION, "goToDefinition");
        assert_eq!(LSP_OPERATION_FIND_REFERENCES, "findReferences");
        assert_eq!(LSP_OPERATION_HOVER, "hover");
        assert_eq!(LSP_OPERATION_DOCUMENT_SYMBOL, "documentSymbol");
        assert_eq!(LSP_OPERATION_WORKSPACE_SYMBOL, "workspaceSymbol");
        assert_eq!(LSP_OPERATION_GO_TO_IMPLEMENTATION, "goToImplementation");
        assert_eq!(LSP_OPERATION_PREPARE_CALL_HIERARCHY, "prepareCallHierarchy");
        assert_eq!(LSP_OPERATION_INCOMING_CALLS, "incomingCalls");
        assert_eq!(LSP_OPERATION_OUTGOING_CALLS, "outgoingCalls");
    }

    #[test]
    fn lsp_position_error_byte_locked() {
        assert_eq!(
            LSP_POSITION_ERROR,
            "LSP position must be 1-based (line >= 1, character >= 1)"
        );
    }

    #[test]
    fn lsp_server_not_running_template() {
        let s = format!("LSP server '{}' is not running", "rust-analyzer");
        assert_eq!(s, "LSP server 'rust-analyzer' is not running");
    }

    // -- LSP.1: too-large file → graceful MB success message -----------------

    #[test]
    fn file_too_large_message_reports_mb_byte_exact() {
        // 20 MB exactly → 20MB; matches `Math.ceil(20_000_000 / 1_000_000)`.
        assert_eq!(
            file_too_large_message(20_000_000),
            "File too large for LSP analysis (20MB exceeds 10MB limit)"
        );
        // One byte over 10 MB → ceil(10.000001) = 11MB (NOT bytes).
        assert_eq!(
            file_too_large_message(10_000_001),
            "File too large for LSP analysis (11MB exceeds 10MB limit)"
        );
        // 15.5 MB → ceil = 16MB.
        assert_eq!(
            file_too_large_message(15_500_000),
            "File too large for LSP analysis (16MB exceeds 10MB limit)"
        );
    }

    // -- LSP.2: validate_input existence / not-a-file ------------------------

    #[test]
    fn validate_input_missing_file_message_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.rs");
        let p = missing.to_string_lossy().into_owned();
        let err = validate_file_path(&p).unwrap_err();
        assert_eq!(err.0, format!("File does not exist: {p}"));
    }

    #[test]
    fn validate_input_directory_is_not_a_file_message_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_string_lossy().into_owned();
        let err = validate_file_path(&p).unwrap_err();
        assert_eq!(err.0, format!("Path is not a file: {p}"));
    }

    #[test]
    fn validate_input_regular_file_ok() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ok.rs");
        std::fs::write(&file, b"fn main() {}").unwrap();
        let p = file.to_string_lossy().into_owned();
        assert!(validate_file_path(&p).is_ok());
    }

    // -- LSP.3: tilde / relative path expansion ------------------------------

    #[test]
    fn expand_path_tilde_forms() {
        assert_eq!(expand_path("~"), home_dir());
        assert_eq!(expand_path("~/foo/bar"), home_dir().join("foo/bar"));
    }

    #[test]
    fn expand_path_absolute_unchanged() {
        assert_eq!(
            expand_path("/abs/path/file.rs"),
            PathBuf::from("/abs/path/file.rs")
        );
    }

    #[test]
    fn expand_path_relative_resolves_against_cwd() {
        let got = expand_path("rel/dir/file.rs");
        assert!(got.is_absolute());
        assert_eq!(got, current_cwd().join("rel/dir/file.rs"));
    }

    #[test]
    fn uri_to_file_path_decodes_percent_encoding() {
        assert_eq!(
            uri_to_file_path("file:///tmp/a%20b/c.rs"),
            "/tmp/a b/c.rs"
        );
        // Malformed percent escape → fall back to the un-decoded body.
        assert_eq!(uri_to_file_path("file:///tmp/x%2"), "/tmp/x%2");
    }

    // -- LSP.4: gitignore filtering ------------------------------------------

    fn file_uri(p: &std::path::Path) -> String {
        format!("file://{}", p.to_string_lossy())
    }

    #[test]
    fn filter_gitignored_drops_ignored_locations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Real git repo so `git check-ignore` resolves.
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
        };
        git(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), b"ignored/\n").unwrap();
        std::fs::create_dir(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/secret.rs"), b"x").unwrap();
        std::fs::write(root.join("keep.rs"), b"y").unwrap();

        let kept = file_uri(&root.join("keep.rs"));
        let dropped = file_uri(&root.join("ignored/secret.rs"));
        let raw = json!([
            { "uri": kept, "range": {} },
            { "uri": dropped, "range": {} },
        ]);

        let out = filter_gitignored_results("findReferences", raw, root);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1, "ignored location should be filtered out");
        assert_eq!(arr[0]["uri"].as_str().unwrap(), kept);
    }

    #[test]
    fn filter_gitignored_workspace_symbol_uses_location_uri() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        std::fs::write(root.join(".gitignore"), b"target/\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/gen.rs"), b"x").unwrap();
        std::fs::write(root.join("src.rs"), b"y").unwrap();

        let kept = file_uri(&root.join("src.rs"));
        let dropped = file_uri(&root.join("target/gen.rs"));
        let raw = json!([
            { "name": "A", "location": { "uri": kept, "range": {} } },
            { "name": "B", "location": { "uri": dropped, "range": {} } },
        ]);

        let out = filter_gitignored_results("workspaceSymbol", raw, root);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"].as_str().unwrap(), "A");
    }

    #[test]
    fn filter_gitignored_ignores_non_location_ops_and_non_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // hover (not a location op) — returned verbatim.
        let hover = json!({ "contents": "doc" });
        assert_eq!(
            filter_gitignored_results("hover", hover.clone(), root),
            hover
        );
        // goToDefinition single (non-array) Location — TS only filters arrays.
        let single = json!({ "uri": "file:///whatever.rs", "range": {} });
        assert_eq!(
            filter_gitignored_results("goToDefinition", single.clone(), root),
            single
        );
    }

    #[test]
    fn filter_gitignored_keeps_all_when_nothing_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        std::fs::write(root.join("a.rs"), b"x").unwrap();
        std::fs::write(root.join("b.rs"), b"y").unwrap();
        let raw = json!([
            { "uri": file_uri(&root.join("a.rs")), "range": {} },
            { "uri": file_uri(&root.join("b.rs")), "range": {} },
        ]);
        let out = filter_gitignored_results("goToImplementation", raw.clone(), root);
        assert_eq!(out, raw);
    }

    #[test]
    fn filter_gitignored_location_link_uses_target_uri() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        std::fs::write(root.join(".gitignore"), b"dist/\n").unwrap();
        std::fs::create_dir(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/out.rs"), b"x").unwrap();
        std::fs::write(root.join("in.rs"), b"y").unwrap();

        let kept = file_uri(&root.join("in.rs"));
        let dropped = file_uri(&root.join("dist/out.rs"));
        // LocationLink shape: `targetUri` rather than `uri`.
        let raw = json!([
            { "targetUri": kept, "targetRange": {} },
            { "targetUri": dropped, "targetRange": {} },
        ]);
        let out = filter_gitignored_results("goToDefinition", raw, root);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["targetUri"].as_str().unwrap(), kept);
    }

    #[test]
    fn lsp_unknown_operation_template() {
        // `completion` is no longer a tool operation (TS LSPTool has no
        // completion op) — it is now rejected as unknown.
        let op = "completion";
        let s = format!(
            "LSPTool: unknown operation {op:?}; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
        );
        assert_eq!(
            s,
            "LSPTool: unknown operation \"completion\"; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
        );
    }
}
