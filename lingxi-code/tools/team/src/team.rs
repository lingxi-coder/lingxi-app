//! Team-management builtin tools (`TeamCreate`, `TeamDelete`).
//!
//! LingXi-internal feature with no upstream `claude-code/src/tools/`
//! counterpart. Both tools operate on the M3-02 team-mem root
//! `~/.claude/team-mem/<team_name>/`. The `config.json` body is a LingXi
//! schema-versioned descriptor; the directory itself becomes the
//! watcher root when `settings.team_memory.enabled == true` (M3-02).
//!
//! Wire identifiers locked in spec §7 lines 501-507 and reproduced byte-for-byte
//! in `parity/fixtures/team_tools.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    TEAM_CREATE_COMPLETED, TEAM_CREATE_FAILED, TEAM_CREATE_STARTED, TEAM_DELETE_COMPLETED,
    TEAM_DELETE_FAILED, TEAM_DELETE_STARTED,
};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

// -- Wire identifier locks (spec §7 lines 501-507) ---------------------------

/// M3-02 lock: the `~/.claude/team-mem/` subdirectory name.
///
/// **Deviation from plan Task 0 step 2:** the plan called for importing
/// `memory::memdir::paths::TEAM_MEM_SUBDIR`, but `lingxi-memory`
/// already depends transitively on `lingxi-tools` (via `lingxi-sidequery`),
/// so a direct path-dep would form a cycle. We mirror the M3-02 literal
/// here; the parity fixture asserts the two strings stay in sync.
pub const TEAM_MEM_SUBDIR: &str = "team-mem";

/// Tool name for the `TeamCreate` builtin.
pub const TEAM_CREATE_TOOL_NAME: &str = "TeamCreate";

/// Tool name for the `TeamDelete` builtin.
pub const TEAM_DELETE_TOOL_NAME: &str = "TeamDelete";

/// Default team name (spec §7 line 507 lock).
pub const DEFAULT_TEAM_NAME: &str = "default";

/// Team config filename (spec §7 line 506 lock).
pub const TEAM_CONFIG_FILENAME: &str = "config.json";

/// Maximum team name length (LingXi-internal lock; see plan critical-fidelity note).
pub const MAX_TEAM_NAME_LEN: usize = 64;

/// Allowed-character-class description (user-visible; appears in error strings).
pub const TEAM_NAME_PATTERN_DESC: &str = "[a-zA-Z0-9_-]+";

// -- Tool structs (impl Tool added in Tasks 3 and 5) -------------------------

/// Builtin tool: create a team directory + default `config.json`.
pub struct TeamCreateTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

/// Builtin tool: delete a team directory (with safety opt-in).
pub struct TeamDeleteTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl TeamCreateTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

impl TeamDeleteTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

// -- Path helpers ------------------------------------------------------------

/// Resolve the on-disk team directory: `<home>/.claude/team-mem/<team_name>/`.
///
/// The use of [`TEAM_MEM_SUBDIR`] (symbol, not literal) makes the M3-02 lock
/// observable at compile time.
#[must_use]
pub fn resolve_team_dir(home: &Path, team_name: &str) -> PathBuf {
    config_home_dir(home).join(TEAM_MEM_SUBDIR).join(team_name)
}

/// User config-home: `$CLAUDE_CONFIG_DIR` (set+non-empty) else `<home>/.claude`
/// (claude-code `tr()` / `getClaudeConfigHomeDir`).
#[must_use]
fn config_home_dir(home: &Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.join(".claude"),
    }
}

/// Resolve the active HOME directory.
///
/// Reads `$HOME` (the integration / parity tests redirect this env var to a
/// `tempfile::TempDir`). Returns [`ToolError::Internal`] when the env var is
/// missing — admin tools cannot meaningfully operate without a HOME.
pub(crate) fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("Team: HOME directory not available".into()))
}

/// Validate a team name per spec §7 + plan locks.
///
/// Rules:
/// - non-empty
/// - length <= [`MAX_TEAM_NAME_LEN`] bytes
/// - no `/`, `\`, `..`, or `\0`
/// - all chars in `[a-zA-Z0-9_-]`
///
/// Each failure produces a byte-locked error string (see
/// `parity/fixtures/team_tools.json`).
pub(crate) fn validate_team_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidInput("Team: team_name is empty".into()));
    }
    if name.len() > MAX_TEAM_NAME_LEN {
        return Err(ToolError::InvalidInput(format!(
            "Team: team_name '{name}' exceeds max length {MAX_TEAM_NAME_LEN}"
        )));
    }
    // Slash / traversal check FIRST so its error string is more specific
    // than the generic "invalid characters" message.
    if name.contains('/') || name.contains('\\') || name == ".." || name.contains("..") {
        return Err(ToolError::InvalidInput(format!(
            "Team: team_name '{name}' contains slashes or path traversal"
        )));
    }
    for ch in name.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if !ok {
            return Err(ToolError::InvalidInput(format!(
                "Team: team_name '{name}' contains invalid characters (allowed: {TEAM_NAME_PATTERN_DESC})"
            )));
        }
    }
    Ok(())
}

// -- Shared telemetry helpers ------------------------------------------------

fn fresh_invocation_id() -> String {
    tool_api::util::ids::ulid_or_uuid()
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn pii_team_name(team_name: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(team_name.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_team_started(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    team_name: &str,
    extras: &[(&'static str, AnalyticsValue)],
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("invocation_id".into(), verified_str(invocation_id));
    md.insert("_PROTO_team_name".into(), pii_team_name(team_name));
    for (k, v) in extras {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

async fn emit_team_completed(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    team_name: &str,
    duration_ms: u64,
    extras: &[(&'static str, AnalyticsValue)],
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("invocation_id".into(), verified_str(invocation_id));
    md.insert("_PROTO_team_name".into(), pii_team_name(team_name));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    for (k, v) in extras {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

async fn emit_team_failed(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    team_name: &str,
    error_kind: &str,
    duration_ms: u64,
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("invocation_id".into(), verified_str(invocation_id));
    md.insert("_PROTO_team_name".into(), pii_team_name(team_name));
    md.insert("error_kind".into(), verified_str(error_kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(event, md).await;
}

// -- TeamCreateTool ----------------------------------------------------------

static TEAM_CREATE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "team_name": { "type": "string", "minLength": 1, "maxLength": 64 }
        },
        "required": ["team_name"]
    })
});

#[async_trait]
impl Tool for TeamCreateTool {
    fn name(&self) -> &str {
        TEAM_CREATE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TEAM_CREATE_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "TeamCreate provisions a team-mem directory (admin action)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Create a team-memory directory under ~/.claude/team-mem/<team_name>/.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TeamCreate to provision a new team-memory directory.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // Parse team_name.
        let team_name = match input.get("team_name").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_team_failed(
                    &bus,
                    TEAM_CREATE_FAILED,
                    &invocation_id,
                    "",
                    "missing_team_name",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TeamCreate: missing or non-string team_name".into(),
                ));
            }
        };

        emit_team_started(&bus, TEAM_CREATE_STARTED, &invocation_id, &team_name, &[]).await;

        // Validate name.
        if let Err(e) = validate_team_name(&team_name) {
            emit_team_failed(
                &bus,
                TEAM_CREATE_FAILED,
                &invocation_id,
                &team_name,
                "invalid_team_name",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(e);
        }

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_team_failed(
                    &bus,
                    TEAM_CREATE_FAILED,
                    &invocation_id,
                    &team_name,
                    "no_home",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(e);
            }
        };
        let team_dir = resolve_team_dir(&home, &team_name);
        let config_path = team_dir.join(TEAM_CONFIG_FILENAME);

        // Existence check.
        if tokio::fs::try_exists(&team_dir).await.unwrap_or(false) {
            emit_team_failed(
                &bus,
                TEAM_CREATE_FAILED,
                &invocation_id,
                &team_name,
                "already_exists",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "TeamCreate: team '{team_name}' already exists at {}",
                team_dir.display()
            )));
        }

        // Create dir.
        if let Err(e) = tokio::fs::create_dir_all(&team_dir).await {
            emit_team_failed(
                &bus,
                TEAM_CREATE_FAILED,
                &invocation_id,
                &team_name,
                "io_create_dir",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::Io(format!(
                "Team: io error at {}: {e}",
                team_dir.display()
            )));
        }

        // Build + write config.
        let config = json!({
            "team_name": team_name,
            "schema_version": 1,
            "created_at_unix_secs": now_unix_secs(),
        });
        let body = match serde_json::to_vec_pretty(&config) {
            Ok(b) => b,
            Err(e) => {
                emit_team_failed(
                    &bus,
                    TEAM_CREATE_FAILED,
                    &invocation_id,
                    &team_name,
                    "serde_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(format!("Team: serde error: {e}")));
            }
        };
        if let Err(e) = tokio::fs::write(&config_path, &body).await {
            emit_team_failed(
                &bus,
                TEAM_CREATE_FAILED,
                &invocation_id,
                &team_name,
                "io_write_config",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::Io(format!(
                "Team: io error at {}: {e}",
                config_path.display()
            )));
        }

        emit_team_completed(
            &bus,
            TEAM_CREATE_COMPLETED,
            &invocation_id,
            &team_name,
            started.elapsed().as_millis() as u64,
            &[("bytes_written", AnalyticsValue::Int(body.len() as i64))],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({
                "team_name": team_name,
                "team_dir": team_dir.display().to_string(),
                "config_path": config_path.display().to_string(),
                "created": true,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// -- TeamDeleteTool ----------------------------------------------------------

static TEAM_DELETE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "team_name": { "type": "string", "minLength": 1, "maxLength": 64 },
            "force":     { "type": "boolean", "default": false }
        },
        "required": ["team_name"]
    })
});

#[async_trait]
impl Tool for TeamDeleteTool {
    fn name(&self) -> &str {
        TEAM_DELETE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TEAM_DELETE_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "TeamDelete removes a team-mem directory (admin action; safety gate enforced inline)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Delete a team-memory directory (safety opt-in via force=true).".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TeamDelete to remove a team-memory directory; pass force=true if non-empty.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        let team_name = match input.get("team_name").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_team_failed(
                    &bus,
                    TEAM_DELETE_FAILED,
                    &invocation_id,
                    "",
                    "missing_team_name",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TeamDelete: missing or non-string team_name".into(),
                ));
            }
        };
        let force = input.get("force").and_then(Value::as_bool).unwrap_or(false);

        emit_team_started(
            &bus,
            TEAM_DELETE_STARTED,
            &invocation_id,
            &team_name,
            &[("force", AnalyticsValue::Bool(force))],
        )
        .await;

        if let Err(e) = validate_team_name(&team_name) {
            emit_team_failed(
                &bus,
                TEAM_DELETE_FAILED,
                &invocation_id,
                &team_name,
                "invalid_team_name",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(e);
        }

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_team_failed(
                    &bus,
                    TEAM_DELETE_FAILED,
                    &invocation_id,
                    &team_name,
                    "no_home",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(e);
            }
        };
        let team_dir = resolve_team_dir(&home, &team_name);

        if !tokio::fs::try_exists(&team_dir).await.unwrap_or(false) {
            emit_team_failed(
                &bus,
                TEAM_DELETE_FAILED,
                &invocation_id,
                &team_name,
                "not_found",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "TeamDelete: team '{team_name}' does not exist at {}",
                team_dir.display()
            )));
        }

        // Count entries (one-level read_dir).
        let mut entries = match tokio::fs::read_dir(&team_dir).await {
            Ok(e) => e,
            Err(e) => {
                emit_team_failed(
                    &bus,
                    TEAM_DELETE_FAILED,
                    &invocation_id,
                    &team_name,
                    "io_read_dir",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Io(format!(
                    "Team: io error at {}: {e}",
                    team_dir.display()
                )));
            }
        };
        let mut file_count: u64 = 0;
        loop {
            match entries.next_entry().await {
                Ok(Some(_)) => file_count += 1,
                Ok(None) => break,
                Err(e) => {
                    emit_team_failed(
                        &bus,
                        TEAM_DELETE_FAILED,
                        &invocation_id,
                        &team_name,
                        "io_read_dir",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::Io(format!(
                        "Team: io error at {}: {e}",
                        team_dir.display()
                    )));
                }
            }
        }

        if file_count > 0 && !force {
            emit_team_failed(
                &bus,
                TEAM_DELETE_FAILED,
                &invocation_id,
                &team_name,
                "non_empty",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "TeamDelete: team '{team_name}' directory is non-empty ({file_count} files); pass force=true to delete anyway"
            )));
        }

        if let Err(e) = tokio::fs::remove_dir_all(&team_dir).await {
            emit_team_failed(
                &bus,
                TEAM_DELETE_FAILED,
                &invocation_id,
                &team_name,
                "io_remove_dir",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::Io(format!(
                "Team: io error at {}: {e}",
                team_dir.display()
            )));
        }

        emit_team_completed(
            &bus,
            TEAM_DELETE_COMPLETED,
            &invocation_id,
            &team_name,
            started.elapsed().as_millis() as u64,
            &[(
                "file_count_at_delete",
                AnalyticsValue::Int(file_count as i64),
            )],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({
                "team_name": team_name,
                "team_dir": team_dir.display().to_string(),
                "deleted": true,
                "file_count_at_delete": file_count,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn assert_valid(name: &str) {
        validate_team_name(name).unwrap_or_else(|_| panic!("expected '{name}' valid"));
    }

    fn assert_rejects(name: &str, fragment: &str) {
        let err = validate_team_name(name)
            .unwrap_err_or_panic_with(|| format!("expected '{name}' rejected"));
        let msg = match err {
            ToolError::InvalidInput(s) => s,
            other => panic!("unexpected error variant: {other:?}"),
        };
        assert!(
            msg.contains(fragment),
            "error '{msg}' missing fragment '{fragment}'"
        );
    }

    // Local extension to ToolError to provide an unwrap_err with custom panic.
    trait UnwrapErrOrPanic<T> {
        fn unwrap_err_or_panic_with<F: FnOnce() -> String>(self, msg: F) -> ToolError;
    }
    impl<T: std::fmt::Debug> UnwrapErrOrPanic<T> for Result<T, ToolError> {
        fn unwrap_err_or_panic_with<F: FnOnce() -> String>(self, msg: F) -> ToolError {
            match self {
                Ok(_) => panic!("{}", msg()),
                Err(e) => e,
            }
        }
    }

    #[test]
    fn accepts_simple_lowercase_name() {
        assert_valid("default");
        assert_valid("alpha");
        assert_valid("team1");
    }

    #[test]
    fn accepts_underscore_dash_digits_mixed_case() {
        assert_valid("Alpha_Beta-1");
        assert_valid("A_b-2_C");
        assert_valid("X");
    }

    #[test]
    fn rejects_empty() {
        assert_rejects("", "team_name is empty");
    }

    #[test]
    fn rejects_too_long() {
        let long: String = "a".repeat(MAX_TEAM_NAME_LEN + 1);
        assert_rejects(&long, "exceeds max length 64");
    }

    #[test]
    fn rejects_slash_or_traversal() {
        assert_rejects("a/b", "slashes or path traversal");
        assert_rejects("a\\b", "slashes or path traversal");
        assert_rejects("..", "slashes or path traversal");
        assert_rejects("a/../b", "slashes or path traversal");
    }

    #[test]
    fn rejects_invalid_chars() {
        assert_rejects("hello world", "invalid characters");
        assert_rejects("hello.world", "invalid characters");
        assert_rejects("hello!", "invalid characters");
        assert_rejects("héllo", "invalid characters"); // non-ASCII
        assert_rejects("a\0b", "invalid characters");
    }

    // Byte-locked error string assertions (Task 4 equivalent).

    fn err_string(name: &str) -> String {
        match validate_team_name(name) {
            Err(ToolError::InvalidInput(s)) => s,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn error_string_empty_team_name_is_byte_locked() {
        assert_eq!(err_string(""), "Team: team_name is empty");
    }

    #[test]
    fn error_string_too_long_team_name_is_byte_locked() {
        let long = "a".repeat(65);
        assert_eq!(
            err_string(&long),
            format!("Team: team_name '{long}' exceeds max length 64")
        );
    }

    #[test]
    fn error_string_invalid_char_template_is_byte_locked() {
        assert_eq!(
            err_string("bad name"),
            "Team: team_name 'bad name' contains invalid characters (allowed: [a-zA-Z0-9_-]+)"
        );
    }

    #[test]
    fn error_string_slash_template_is_byte_locked() {
        assert_eq!(
            err_string("a/b"),
            "Team: team_name 'a/b' contains slashes or path traversal"
        );
    }
}

#[cfg(test)]
mod tool_metadata_tests {
    use super::*;
    use serde_json::json;
    use telemetry::AnalyticsBus;
    use tool_api::test_support::ctx_for_file_tools;

    fn make_create() -> TeamCreateTool {
        let bus = Arc::new(AnalyticsBus::new());
        let ctx = ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::env::temp_dir()],
        );
        TeamCreateTool::new(ctx)
    }

    fn make_delete() -> TeamDeleteTool {
        let bus = Arc::new(AnalyticsBus::new());
        let ctx = ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::env::temp_dir()],
        );
        TeamDeleteTool::new(ctx)
    }

    #[test]
    fn create_metadata_is_concurrency_safe_and_not_destructive() {
        let t = make_create();
        assert_eq!(t.name(), "TeamCreate");
        assert!(t.is_concurrency_safe(&json!({})));
        assert!(!t.is_read_only(&json!({})));
        assert!(!t.is_destructive(&json!({})));
        assert!(!t.is_open_world(&json!({})));
    }

    #[test]
    fn delete_metadata_is_destructive_not_read_only() {
        let t = make_delete();
        assert_eq!(t.name(), "TeamDelete");
        assert!(t.is_destructive(&json!({})));
        assert!(!t.is_read_only(&json!({})));
        assert!(t.is_concurrency_safe(&json!({})));
        assert!(!t.is_open_world(&json!({})));
    }
}
