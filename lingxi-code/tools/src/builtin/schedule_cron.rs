//! `ScheduleCronTool` — parse 5-field cron + persist `~/.claude/cron/<task_id>.json`.
//!
//! Wire identifiers locked in spec §7:
//! - 5-field cron expressions only (6-field with seconds is rejected).
//! - Persistence path `~/.claude/cron/<task_id>.json` where
//!   `task_id = [d][0-9a-z]{8}` (9-char M1 format).
//!
//! M4-08 does NOT register the job into a scheduler — it parses, computes
//! the next fire time, and persists the job descriptor. Production hosts
//! pick the file up via `cron::scheduler::CronScheduler`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    SCHEDULE_CRON_COMPLETED, SCHEDULE_CRON_FAILED, SCHEDULE_CRON_STARTED,
};
use telemetry::AnalyticsBus;

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

// -- Inlined 5-field cron parser (mirrors cron::schedule) -------------
//
// We mirror the M1.17 cron::schedule::{parse_cron, CronExpression,
// CronField, decompose} surface inline because the natural dependency
// `lingxi-tools → lingxi-cron` would form a workspace cycle through
// `lingxi-cron → lingxi-tasks → lingxi-agent → lingxi-memory →
//  lingxi-sidequery → lingxi-tools`. The parser and matcher are byte-for-byte
// equivalent; downstream parity is verified by the (out-of-cycle) M4-09
// fixture driver against the canonical lingxi_cron table.

#[derive(Debug, Clone)]
enum CronField {
    Any,
    Exact(u32),
    Step(u32),
    Range(u32, u32),
    List(Vec<u32>),
}

#[derive(Debug, Clone)]
struct CronExpression {
    minute: CronField,
    hour: CronField,
    dom: CronField,
    month: CronField,
    dow: CronField,
}

fn parse_cron_field(s: &str) -> Result<CronField, String> {
    if s == "*" {
        return Ok(CronField::Any);
    }
    if let Some(rest) = s.strip_prefix("*/") {
        let n = rest.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::Step(n));
    }
    if let Some((a, b)) = s.split_once('-') {
        let a = a.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        let b = b.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::Range(a, b));
    }
    if s.contains(',') {
        let list: Vec<u32> = s
            .split(',')
            .map(str::parse::<u32>)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::List(list));
    }
    s.parse::<u32>()
        .map(CronField::Exact)
        .map_err(|_| format!("bad field: {s}"))
}

fn parse_cron(s: &str) -> Result<CronExpression, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!("expected 5 fields, got {}", parts.len()));
    }
    Ok(CronExpression {
        minute: parse_cron_field(parts[0])?,
        hour: parse_cron_field(parts[1])?,
        dom: parse_cron_field(parts[2])?,
        month: parse_cron_field(parts[3])?,
        dow: parse_cron_field(parts[4])?,
    })
}

fn field_match(field: &CronField, value: u32) -> bool {
    match field {
        CronField::Any => true,
        CronField::Exact(v) => *v == value,
        CronField::Step(n) => *n > 0 && value % n == 0,
        CronField::Range(a, b) => value >= *a && value <= *b,
        CronField::List(list) => list.contains(&value),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    // Mirrors cron::schedule::decompose (crude UTC; sufficient for matches()).
    let minute = (secs / 60 % 60) as u32;
    let hour = (secs / 3600 % 24) as u32;
    let day = (secs / 86_400 % 30 + 1) as u32;
    let month = ((secs / 2_628_000) % 12 + 1) as u32;
    let year = 1970 + (secs / 31_536_000) as u32;
    let dow = ((secs / 86_400 + 4) % 7) as u32;
    (year, month, day, hour, minute, dow)
}

impl CronExpression {
    fn matches(&self, time: SystemTime) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (year, month, day, hour, minute, dow) = decompose(secs);
        field_match(&self.minute, minute)
            && field_match(&self.hour, hour)
            && field_match(&self.dom, day)
            && field_match(&self.month, month)
            && field_match(&self.dow, dow)
            && year > 1970
    }
}

/// Tool name byte-lock.
pub const SCHEDULE_CRON_TOOL_NAME: &str = "ScheduleCron";
/// Subdirectory under `~/.claude/`.
pub const CRON_SUBDIR: &str = "cron";
/// File extension.
pub const CRON_FILE_SUFFIX: &str = ".json";
/// 9-char task-id prefix character (`d` for daemon/cron).
pub const CRON_TASK_ID_PREFIX: char = 'd';
/// 6-field rejection message.
pub const SIX_FIELD_REJECTION: &str =
    "ScheduleCron: 6-field cron (with seconds) is not supported; use 5-field cron (minute hour day month weekday)";
/// Search horizon for `next_fire_unix_secs`: 1 year of minutes (sane upper
/// bound so an unsatisfiable expression doesn't loop forever).
pub const NEXT_FIRE_HORIZON_MINUTES: u64 = 60 * 24 * 366;

fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("ScheduleCron: HOME directory not available".into()))
}

#[must_use]
pub(crate) fn cron_path(home: &Path, task_id: &str) -> PathBuf {
    home.join(".claude")
        .join(CRON_SUBDIR)
        .join(format!("{task_id}{CRON_FILE_SUFFIX}"))
}

#[must_use]
pub(crate) fn generate_cron_task_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let mut s = String::with_capacity(9);
    s.push(CRON_TASK_ID_PREFIX);
    for _ in 0..8 {
        let idx = rng.random_range(0..ALPHABET.len());
        s.push(ALPHABET[idx] as char);
    }
    s
}

/// Compute the next fire time at or after `from` matching `expr`, capped by
/// [`NEXT_FIRE_HORIZON_MINUTES`]. Returns `None` if no minute in the horizon
/// satisfies the expression (rare; only happens for malformed expressions).
fn next_fire_after(expr: &CronExpression, from: SystemTime) -> Option<SystemTime> {
    // Round up to the next minute boundary.
    let from_secs = from.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let mut candidate_secs = (from_secs / 60 + 1) * 60;
    for _ in 0..NEXT_FIRE_HORIZON_MINUTES {
        let candidate = SystemTime::UNIX_EPOCH + Duration::from_secs(candidate_secs);
        if expr.matches(candidate) {
            return Some(candidate);
        }
        candidate_secs += 60;
    }
    None
}

/// Reject 6-field expressions (Quartz-style with seconds). The cron crate's
/// own parser would surface `FieldCount(6)`, but we want a custom message.
fn reject_six_field(expr: &str) -> Result<(), ToolError> {
    let parts: usize = expr.split_whitespace().count();
    if parts == 6 {
        return Err(ToolError::InvalidInput(SIX_FIELD_REJECTION.into()));
    }
    Ok(())
}

/// `ScheduleCronTool` — parse + persist a cron job descriptor.
pub struct ScheduleCronTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl ScheduleCronTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "expression": { "type": "string", "minLength": 1 },
            "command":    { "type": "string", "minLength": 1 }
        },
        "required": ["expression", "command"]
    })
});

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(SCHEDULE_CRON_FAILED, md).await;
}

#[async_trait]
impl Tool for ScheduleCronTool {
    fn name(&self) -> &str {
        SCHEDULE_CRON_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4_096
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
                reason: "ScheduleCron persists a cron descriptor under ~/.claude/cron/".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Schedule a cron job. Accepts 5-field cron expressions only.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "ScheduleCron: persist a cron descriptor under ~/.claude/cron/.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let expr = input
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ValidationError("ScheduleCron: missing or non-string expression".into())
            })?;
        reject_six_field(expr).map_err(|e| ValidationError(format!("{e}")))?;
        parse_cron(expr).map_err(|e| {
            ValidationError(format!(
                "ScheduleCron: invalid cron expression {expr:?}: {e}"
            ))
        })?;
        input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("ScheduleCron: missing or non-string command".into()))?;
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let expr = match input.get("expression").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_expression",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "ScheduleCron: missing or non-string expression".into(),
                ));
            }
        };
        if let Err(e) = reject_six_field(&expr) {
            emit_failed(
                &bus,
                "six_field_unsupported",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(e);
        }
        let command = match input.get("command").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_command",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "ScheduleCron: missing or non-string command".into(),
                ));
            }
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_expression".into(), pii_str(&expr));
        md.insert("_PROTO_command".into(), pii_str(&command));
        bus.log_event(SCHEDULE_CRON_STARTED, md).await;

        let parsed = match parse_cron(&expr) {
            Ok(c) => c,
            Err(e) => {
                emit_failed(&bus, "parse_error", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(format!(
                    "ScheduleCron: invalid cron expression {expr:?}: {e}"
                )));
            }
        };
        let now = self.ctx.clock.now();
        let next = next_fire_after(&parsed, now);
        let next_unix = next
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let task_id = generate_cron_task_id();
        let path = cron_path(&home, &task_id);
        let descriptor = json!({
            "task_id": task_id,
            "expression": expr,
            "command": command,
            "created_at_unix_secs": now
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "next_fire_unix_secs": next_unix,
        });
        if let Some(dir) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                emit_failed(&bus, "io_create_dir", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "ScheduleCron: io error at {}: {e}",
                    dir.display()
                )));
            }
        }
        let body = match serde_json::to_vec_pretty(&descriptor) {
            Ok(b) => b,
            Err(e) => {
                emit_failed(&bus, "serde_error", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Internal(format!(
                    "ScheduleCron: serde error: {e}"
                )));
            }
        };
        if let Err(e) = tokio::fs::write(&path, &body).await {
            emit_failed(&bus, "io_write", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::Io(format!(
                "ScheduleCron: io error at {}: {e}",
                path.display()
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "next_fire_unix_secs".into(),
            match next_unix {
                Some(n) => AnalyticsValue::Int(n as i64),
                None => AnalyticsValue::None,
            },
        );
        md.insert(
            "bytes_written".into(),
            AnalyticsValue::Int(body.len() as i64),
        );
        bus.log_event(SCHEDULE_CRON_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "task_id": task_id,
                "path": path.display().to_string(),
                "expression": expr,
                "command": command,
                "next_fire_unix_secs": next_unix,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, shell_test_ctx, HOME_LOCK};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SCHEDULE_CRON_TOOL_NAME, "ScheduleCron");
        assert_eq!(CRON_SUBDIR, "cron");
        assert_eq!(CRON_TASK_ID_PREFIX, 'd');
    }

    #[test]
    fn task_id_format() {
        let id = generate_cron_task_id();
        assert_eq!(id.len(), 9);
        assert_eq!(id.chars().next().unwrap(), 'd');
    }

    #[test]
    fn rejects_six_field_with_seconds() {
        let r = reject_six_field("0 */5 9-17 * * 1-5");
        let err = r.unwrap_err();
        assert!(format!("{err}").contains("6-field cron"));
    }

    #[tokio::test]
    async fn persists_descriptor() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ScheduleCronTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(
                json!({"expression": "*/5 9-17 * * 1-5", "command": "echo hi"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let path = out.data["path"].as_str().unwrap();
        assert!(
            path.starts_with(&format!("{}/.claude/cron/d", tmp.path().display())),
            "{path}"
        );
        assert!(path.to_lowercase().ends_with(".json"));
        let written = tokio::fs::read_to_string(path).await.unwrap();
        assert!(written.contains("\"expression\": \"*/5 9-17 * * 1-5\""));
        assert!(written.contains("\"command\": \"echo hi\""));
    }

    #[tokio::test]
    async fn rejects_6_field_expression() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ScheduleCronTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({"expression": "0 */5 9-17 * * 1-5", "command": "x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("must reject");
        assert!(format!("{err}").contains("6-field cron"));
    }

    #[tokio::test]
    async fn rejects_garbage_expression() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ScheduleCronTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({"expression": "not a cron", "command": "x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("garbage");
        assert!(format!("{err}").contains("invalid cron expression"));
    }
}
