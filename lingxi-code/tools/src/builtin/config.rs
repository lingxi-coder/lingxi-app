//! `ConfigTool` — read/write 4 settings fields against `~/.claude/settings.json`.
//!
//! Wire identifiers locked in spec §7:
//! - Backing store: `~/.claude/settings.json`.
//! - Allowed fields (exactly 4): `model`, `outputStyle`, `theme`, `verbose`.
//! - On input, both `outputStyle` and `output_style` are accepted; output
//!   always emits camelCase (upstream parity).
//!
//! M4-08 stops at the local-file read/write; it does NOT consult the broader
//! `settings::SettingsJson` (whose schema currently locks
//! `outputStyle: Option<BTreeMap>` rather than a string, conflicting with the
//! plan's 4-field byte-lock). Operating directly on the JSON keeps ConfigTool
//! aligned with the spec's wire allowlist while leaving the M3-01 schema
//! untouched.
//!
//! no-truncation: ConfigTool returns the 4-field allowlist value (each
//! bounded by the settings.json schema). No free-form user content flows
//! through the response.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{CONFIG_COMPLETED, CONFIG_FAILED, CONFIG_STARTED};
use telemetry::AnalyticsBus;

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const CONFIG_TOOL_NAME: &str = "Config";
/// Backing-store filename.
pub const CONFIG_FILE_NAME: &str = "settings.json";
/// `~/.claude/` subdirectory.
pub const CONFIG_SUBDIR: &str = ".claude";

/// Allowed config fields (spec §7 lock — exactly 4).
pub const CONFIG_FIELD_MODEL: &str = "model";
/// Allowed config field: output style (camelCase wire key).
pub const CONFIG_FIELD_OUTPUT_STYLE: &str = "outputStyle";
/// Allowed config field: theme.
pub const CONFIG_FIELD_THEME: &str = "theme";
/// Allowed config field: verbose toggle.
pub const CONFIG_FIELD_VERBOSE: &str = "verbose";
/// Order-locked allowlist (exactly 4 fields per spec §7).
pub const CONFIG_FIELDS_ALLOWED: [&str; 4] = [
    CONFIG_FIELD_MODEL,
    CONFIG_FIELD_OUTPUT_STYLE,
    CONFIG_FIELD_THEME,
    CONFIG_FIELD_VERBOSE,
];

/// Normalize an inbound field name: accept legacy `output_style` snake_case
/// alias for the `outputStyle` camelCase wire key.
fn normalize_field(name: &str) -> &str {
    match name {
        "output_style" => CONFIG_FIELD_OUTPUT_STYLE,
        other => other,
    }
}

fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("Config: HOME directory not available".into()))
}

#[must_use]
pub(crate) fn config_path(home: &Path) -> PathBuf {
    home.join(CONFIG_SUBDIR).join(CONFIG_FILE_NAME)
}

/// `ConfigTool` — read/write 4 settings fields.
pub struct ConfigTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl ConfigTool {
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
            "op":    { "type": "string", "enum": ["get", "set"] },
            "field": { "type": "string", "enum": ["model", "outputStyle", "output_style", "theme", "verbose"] },
            "value": {}
        },
        "required": ["op", "field"]
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
    bus.log_event(CONFIG_FAILED, md).await;
}

async fn read_settings_obj(path: &Path) -> Result<Map<String, Value>, ToolError> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(Map::new());
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| ToolError::Io(format!("Config: io error at {}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Ok(Map::new());
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
        ToolError::Io(format!(
            "Config: settings.json at {} is invalid JSON: {e}",
            path.display()
        ))
    })?;
    match v {
        Value::Object(m) => Ok(m),
        other => Err(ToolError::Io(format!(
            "Config: settings.json at {} is not a JSON object (got {})",
            path.display(),
            match &other {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => unreachable!(),
            }
        ))),
    }
}

async fn write_settings_obj(path: &Path, obj: &Map<String, Value>) -> Result<usize, ToolError> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| ToolError::Io(format!("Config: io error at {}: {e}", dir.display())))?;
    }
    let body = serde_json::to_vec_pretty(&Value::Object(obj.clone()))
        .map_err(|e| ToolError::Internal(format!("Config: serde error: {e}")))?;
    tokio::fs::write(path, &body)
        .await
        .map_err(|e| ToolError::Io(format!("Config: io error at {}: {e}", path.display())))?;
    Ok(body.len())
}

#[async_trait]
impl Tool for ConfigTool {
    fn name(&self) -> &str {
        CONFIG_TOOL_NAME
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
        false
    }
    fn is_read_only(&self, input: &Value) -> bool {
        input.get("op").and_then(Value::as_str) == Some("get")
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
                reason: "Config reads/writes ~/.claude/settings.json (admin action)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Read or write one of {model, outputStyle, theme, verbose} in ~/.claude/settings.json."
            .into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Config: get/set a single allowlisted field (model, outputStyle, theme, verbose).".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let op = input
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Config: missing or non-string op".into()))?;
        if op != "get" && op != "set" {
            return Err(ValidationError(format!(
                "Config: op must be 'get' or 'set' (got {op:?})"
            )));
        }
        let field_in = input
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Config: missing or non-string field".into()))?;
        let field = normalize_field(field_in);
        if !CONFIG_FIELDS_ALLOWED.contains(&field) {
            return Err(ValidationError(format!(
                "Config: field '{field_in}' is not in allowlist (allowed: {CONFIG_FIELDS_ALLOWED:?})",
            )));
        }
        if op == "set" {
            let v = input
                .get("value")
                .ok_or_else(|| ValidationError("Config: missing value for set op".into()))?;
            match field {
                "verbose" => {
                    if !v.is_boolean() {
                        return Err(ValidationError(
                            "Config: 'verbose' value must be boolean".into(),
                        ));
                    }
                }
                _ => {
                    if !v.is_string() && !v.is_null() {
                        return Err(ValidationError(format!(
                            "Config: '{field}' value must be string or null"
                        )));
                    }
                }
            }
        }
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

        let op = match input.get("op").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_op", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Config: missing or non-string op".into(),
                ));
            }
        };
        if op != "get" && op != "set" {
            emit_failed(&bus, "invalid_op", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Config: op must be 'get' or 'set' (got {op:?})"
            )));
        }
        let field_in = match input.get("field").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_field", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Config: missing or non-string field".into(),
                ));
            }
        };
        let field = normalize_field(&field_in).to_string();
        if !CONFIG_FIELDS_ALLOWED.contains(&field.as_str()) {
            emit_failed(
                &bus,
                "field_not_allowed",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Config: field '{field_in}' is not in allowlist (allowed: {CONFIG_FIELDS_ALLOWED:?})",
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("op".into(), verified_str(&op));
        md.insert("field".into(), verified_str(&field));
        bus.log_event(CONFIG_STARTED, md).await;

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let path = config_path(&home);

        let result = if op == "get" {
            let obj = match read_settings_obj(&path).await {
                Ok(o) => o,
                Err(e) => {
                    emit_failed(&bus, "io_read", started.elapsed().as_millis() as u64).await;
                    return Err(e);
                }
            };
            let value = obj.get(&field).cloned().unwrap_or(Value::Null);
            json!({
                "op": "get",
                "field": field,
                "value": value,
                "path": path.display().to_string(),
            })
        } else {
            let value = input.get("value").cloned().unwrap_or(Value::Null);
            // Validate value type.
            let type_ok = if field == "verbose" {
                value.is_boolean()
            } else {
                value.is_string() || value.is_null()
            };
            if !type_ok {
                emit_failed(&bus, "bad_value_type", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(format!(
                    "Config: invalid value type for field '{field}'"
                )));
            }
            let mut obj = match read_settings_obj(&path).await {
                Ok(o) => o,
                Err(e) => {
                    emit_failed(&bus, "io_read", started.elapsed().as_millis() as u64).await;
                    return Err(e);
                }
            };
            if value.is_null() {
                obj.remove(&field);
            } else {
                obj.insert(field.clone(), value.clone());
            }
            let bytes = match write_settings_obj(&path, &obj).await {
                Ok(n) => n,
                Err(e) => {
                    emit_failed(&bus, "io_write", started.elapsed().as_millis() as u64).await;
                    return Err(e);
                }
            };
            json!({
                "op": "set",
                "field": field,
                "value": value,
                "path": path.display().to_string(),
                "bytes_written": bytes,
            })
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("op".into(), verified_str(&op));
        md.insert("field".into(), verified_str(&field));
        md.insert("_PROTO_path".into(), pii_str(&path.display().to_string()));
        bus.log_event(CONFIG_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: result,
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
        assert_eq!(CONFIG_TOOL_NAME, "Config");
        assert_eq!(CONFIG_FILE_NAME, "settings.json");
        assert_eq!(CONFIG_SUBDIR, ".claude");
        assert_eq!(CONFIG_FIELDS_ALLOWED.len(), 4);
        assert_eq!(
            CONFIG_FIELDS_ALLOWED,
            ["model", "outputStyle", "theme", "verbose"]
        );
    }

    #[test]
    fn normalize_field_aliases_output_style() {
        assert_eq!(normalize_field("output_style"), "outputStyle");
        assert_eq!(normalize_field("outputStyle"), "outputStyle");
        assert_eq!(normalize_field("model"), "model");
    }

    #[tokio::test]
    async fn get_returns_null_when_missing() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(
                json!({"op": "get", "field": "model"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["value"], Value::Null);
    }

    #[tokio::test]
    async fn set_then_get_roundtrip() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        let r = tool
            .call(
                json!({"op": "set", "field": "model", "value": "claude-sonnet-4-5"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("set ok");
        assert!(r.data["bytes_written"].as_u64().unwrap() > 0);
        let r = tool
            .call(
                json!({"op": "get", "field": "model"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("get ok");
        assert_eq!(r.data["value"], json!("claude-sonnet-4-5"));
    }

    #[tokio::test]
    async fn set_verbose_accepts_bool() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        tool.call(
            json!({"op": "set", "field": "verbose", "value": true}),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
    }

    #[tokio::test]
    async fn set_verbose_rejects_string() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({"op": "set", "field": "verbose", "value": "true"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("must reject");
        assert!(format!("{err}").contains("invalid value type"));
    }

    #[tokio::test]
    async fn rejects_field_outside_allowlist() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({"op": "get", "field": "telemetry_enabled"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("not allowed");
        assert!(format!("{err}").contains("not in allowlist"));
    }

    #[tokio::test]
    async fn output_style_snake_case_alias_accepted() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = ConfigTool::new(shell_test_ctx(dummy_out()));
        tool.call(
            json!({"op": "set", "field": "output_style", "value": "default"}),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("alias accepted");
        let r = tool
            .call(
                json!({"op": "get", "field": "outputStyle"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("get camel");
        assert_eq!(r.data["value"], json!("default"));
    }
}
