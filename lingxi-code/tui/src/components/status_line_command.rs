//! (A6) Custom status-line external command — input building + execution.
//!
//! Ports the data path behind claude-code's `statusLine: {type:'command'}`
//! setting (`StatusLine.tsx` + `hooks.ts:executeStatusLineCommand`):
//!   1. [`build_status_line_input`] builds the JSON payload the command reads on
//!      stdin (mirroring `buildStatusLineCommandInput`).
//!   2. [`run_status_line_command`] spawns the configured shell command with a
//!      5s timeout, captures stdout, and runs it through
//!      [`crate::components::status_line::format_custom_status_line`].
//!
//! ## Fidelity / divergences vs. claude-code
//!
//! **Input payload is a documented SUBSET.** claude-code's
//! `buildStatusLineCommandInput` (`StatusLine.tsx:36-127`) emits a large object;
//! Rust only includes the fields the orchestrator currently surfaces onto the
//! TUI `StatusSnapshot` (model, workspace dirs, version, a cost snapshot, and a
//! context-window percentage). Keys present:
//!   - `hook_event_name`  (constant `"Status"`, from `createBaseHookInput`)
//!   - `model.id`, `model.display_name`
//!   - `workspace.current_dir`, `workspace.project_dir`, `workspace.added_dirs`
//!   - `version`
//!   - `cost.total_cost_usd` (parsed from the pre-formatted cost string when it
//!     looks like `"$N.NNNN"`; the other `cost.*` timing/line counters are
//!     omitted — the orchestrator does not expose them to the TUI yet)
//!   - `context_window.used_percentage`, `context_window.remaining_percentage`
//!   - `rate_limits` (OPTIONAL — only when at least one window resolved,
//!     `StatusLine.tsx:99-101`; per-window `five_hour`/`seven_day` keys
//!     conditionally spread from the [`RawUtilizationSnapshot`],
//!     `StatusLine.tsx:50-65`)
//!
//! Keys claude-code emits that are OMITTED here (not available to the TUI):
//!   `session_name`, `output_style`, `cost.total_duration_ms`,
//!   `cost.total_api_duration_ms`, `cost.total_lines_added`,
//!   `cost.total_lines_removed`, `context_window.total_input_tokens`,
//!   `context_window.total_output_tokens`, `context_window.context_window_size`,
//!   `context_window.current_usage`, `exceeds_200k_tokens`,
//!   `vim`, `agent`, `remote`, `worktree`.
//!
//! **Trust gating is simplified.** claude-code gates execution on workspace
//! trust + managed-settings policy (`shouldDisableAllHooksIncludingManaged`,
//! `shouldSkipHookDueToTrust`, `shouldAllowManagedHooksOnly`). The TUI settings
//! loader does not expose those flags here, so [`StatusLineConfig::should_run`]
//! gates only on `type == "command"` + a `trusted` bool the caller supplies
//! (default-false fail-closed). Wiring the real trust store is a follow-up.
//!
//! **The execution pump is wired** (A6 batch-6 Task 2): the debounced,
//! single-flight pump in `root.rs` calls `crate::state::build_pump_payload`
//! (which calls [`build_status_line_input`]) then [`run_status_line_command`]
//! off-thread, and writes the formatted result onto `AppState.status_line_text`
//! (rendered by the `custom` prop on `StatusLine`). It re-arms only on
//! `TurnEvent::TurnEnded` (`AppState.status_line_dirty`), the TUI analog of
//! claude-code's `StatusLine.tsx` re-run on `lastAssistantMessageId`. The
//! payload's OPTIONAL `rate_limits` comes from `AppState.raw_utilization`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::components::status_line::format_custom_status_line;

/// The `"Status"` hook-event name claude-code stamps via `createBaseHookInput()`.
pub const STATUS_HOOK_EVENT_NAME: &str = "Status";

/// Default (and claude-code's) status-line command timeout: 5 seconds.
pub const STATUS_LINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Raw per-window utilization snapshot mirrored from
/// `OutputEvent::RawUtilization` (orchestrator `rawUtilization` track).
/// Windows are atomic: a window's two fields are both `Some` or both `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawUtilizationSnapshot {
    /// `anthropic-ratelimit-unified-5h-utilization` (0-1 fraction).
    pub five_hour_utilization: Option<f64>,
    /// `anthropic-ratelimit-unified-5h-reset` (Unix-epoch seconds).
    pub five_hour_resets_at: Option<u64>,
    /// `anthropic-ratelimit-unified-7d-utilization` (0-1 fraction).
    pub seven_day_utilization: Option<f64>,
    /// `anthropic-ratelimit-unified-7d-reset` (Unix-epoch seconds).
    pub seven_day_resets_at: Option<u64>,
}

/// Parsed `statusLine` setting (the subset this batch honors). Mirrors
/// claude-code's `settings.statusLine`; only `type == "command"` is executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLineConfig {
    /// The `statusLine.type` discriminator. Only `"command"` runs a hook.
    pub kind: String,
    /// The shell command to execute (`statusLine.command`).
    pub command: String,
    /// `statusLine.padding` — horizontal cells padded on each side of the
    /// rendered text. Defaults to `0` when absent.
    pub padding: usize,
}

impl StatusLineConfig {
    /// Parse a `statusLine` JSON object into a [`StatusLineConfig`]. Returns
    /// `None` when the value is absent/not an object or has no `command`
    /// string (a `command`-less config can never produce output).
    #[must_use]
    pub fn from_settings_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let command = obj.get("command").and_then(Value::as_str)?.to_string();
        // `padding` is a non-negative integer in claude-code. Read it as a u64
        // (negatives / non-integers fall through to the default `0`), then
        // narrow to `usize` for the column count.
        let padding = obj
            .get("padding")
            .and_then(Value::as_u64)
            .and_then(|p| usize::try_from(p).ok())
            .unwrap_or(0);
        Some(Self {
            kind,
            command,
            padding,
        })
    }

    /// claude-code `executeStatusLineCommand` runs only when
    /// `statusLine.type === 'command'`. `trusted` folds the simplified
    /// trust/managed gating: fail-closed when the workspace is not trusted.
    #[must_use]
    pub fn should_run(&self, trusted: bool) -> bool {
        trusted && self.kind == "command" && !self.command.is_empty()
    }
}

/// Build the JSON stdin payload for the status-line command, mirroring the
/// (subset of) `buildStatusLineCommandInput`. See the module docs for the
/// included/omitted key list.
///
/// `cost_usd` is the numeric total cost (dollars). `context_pct` is the
/// context-window *used* fraction in `[0.0, 1.0]` (the same value the built-in
/// row renders as `{:.0}%`); `remaining_percentage` is derived as
/// `100 - used_percentage`, matching claude-code's `calculateContextPercentages`.
/// `raw_utilization` is the latest per-window rate-limit snapshot (`None`
/// before any API response carried the headers) feeding `rate_limits`.
#[must_use]
// One parameter per JSON field the payload carries (mirroring the flat
// `buildStatusLineCommandInput` arg list). Bundling them into a struct would
// add a parallel type with no behavioral benefit, so the 9-arg form is kept.
#[allow(clippy::too_many_arguments)]
pub fn build_status_line_input(
    model_id: &str,
    model_display_name: &str,
    current_dir: &Path,
    project_dir: &Path,
    added_dirs: &[String],
    version: &str,
    cost_usd: f64,
    context_pct: f32,
    raw_utilization: Option<&RawUtilizationSnapshot>,
) -> Value {
    let used = f64::from(context_pct) * 100.0;
    let remaining = 100.0 - used;
    // `rate_limits` is OPTIONAL: TS only spreads it into the payload when at
    // least one window resolved (StatusLine.tsx:99-101
    // `...((rateLimits.five_hour || rateLimits.seven_day) && {rate_limits})`;
    // statuslineSetup.ts:67 documents it as "Only present for subscribers
    // after first API response"). A window with either header missing omits
    // its key entirely (TS conditional spread, not `null`).
    let mut rate_limits = serde_json::Map::new();
    if let Some(raw) = raw_utilization {
        if let (Some(u), Some(r)) = (raw.five_hour_utilization, raw.five_hour_resets_at) {
            rate_limits.insert(
                "five_hour".into(),
                json!({ "used_percentage": u * 100.0, "resets_at": r }),
            );
        }
        if let (Some(u), Some(r)) = (raw.seven_day_utilization, raw.seven_day_resets_at) {
            rate_limits.insert(
                "seven_day".into(),
                json!({ "used_percentage": u * 100.0, "resets_at": r }),
            );
        }
    }
    let mut payload = json!({
        "hook_event_name": STATUS_HOOK_EVENT_NAME,
        "model": {
            "id": model_id,
            "display_name": model_display_name,
        },
        "workspace": {
            "current_dir": current_dir.to_string_lossy(),
            "project_dir": project_dir.to_string_lossy(),
            "added_dirs": added_dirs,
        },
        "version": version,
        "cost": {
            "total_cost_usd": cost_usd,
        },
        "context_window": {
            "used_percentage": used,
            "remaining_percentage": remaining,
        },
    });
    if !rate_limits.is_empty() {
        payload["rate_limits"] = Value::Object(rate_limits);
    }
    payload
}

/// Parse a pre-formatted cost string (e.g. `"$0.0042"`) into a dollar amount.
/// Returns `0.0` when the string is not a recognizable `$N.NNNN`. The TUI
/// surfaces cost only as a formatted string, so this recovers the numeric value
/// for the JSON payload's `cost.total_cost_usd`.
#[must_use]
pub fn parse_cost_usd(cost: &str) -> f64 {
    cost.trim_start_matches('$')
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
}

/// Spawn `command` via the platform shell, feed `stdin_json` on stdin, and wait
/// up to `timeout` for it to exit. On a clean (exit code 0) completion the
/// captured stdout is passed through [`format_custom_status_line`] and returned
/// as `Some(text)` when non-empty. Returns `None` on: spawn failure, timeout
/// (the child is killed), non-zero exit, or empty/whitespace-only output —
/// matching `executeStatusLineCommand`'s `undefined` paths.
///
/// The command runs under `sh -c <command>` (POSIX) / `cmd /C <command>`
/// (Windows), the same single-string-command convention as claude-code's
/// `execCommandHook`.
#[must_use]
pub fn run_status_line_command(
    command: &str,
    stdin_json: &str,
    timeout: Duration,
) -> Option<String> {
    use std::io::Write;

    let mut cmd = build_shell_command(command);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().ok()?;

    // Write the JSON payload to the child's stdin, then close it so a command
    // that reads stdin to EOF (e.g. `cat`/`jq`) can terminate.
    if let Some(mut stdin) = child.stdin.take() {
        // A broken pipe (command ignored stdin and exited) is non-fatal.
        let _ = stdin.write_all(stdin_json.as_bytes());
    }

    // Poll for completion up to `timeout`. `wait_with_output` would block
    // indefinitely, so we busy-wait on `try_wait` with a short sleep — the
    // status hook is short-lived (5s cap) so the spin cost is bounded.
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().ok()?;
                if !status.success() {
                    return None;
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                let formatted = format_custom_status_line(&stdout);
                return if formatted.is_empty() {
                    None
                } else {
                    Some(formatted)
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    // Timed out — kill and reap so we don't leak a zombie.
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

/// Build the shell `Command` for a single command string, matching
/// claude-code's `sh -c` / `cmd /C` convention.
fn build_shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_status_line_input_shape() {
        let v = build_status_line_input(
            "claude-sonnet-4.5",
            "Claude Sonnet 4.5",
            &PathBuf::from("/work/cur"),
            &PathBuf::from("/work/proj"),
            &["/extra".to_string()],
            "0.8.0",
            0.0123,
            0.42,
            None,
        );
        // Top-level keys.
        assert_eq!(v["hook_event_name"], "Status");
        assert_eq!(v["version"], "0.8.0");
        // model
        assert_eq!(v["model"]["id"], "claude-sonnet-4.5");
        assert_eq!(v["model"]["display_name"], "Claude Sonnet 4.5");
        // workspace
        assert_eq!(v["workspace"]["current_dir"], "/work/cur");
        assert_eq!(v["workspace"]["project_dir"], "/work/proj");
        assert_eq!(v["workspace"]["added_dirs"][0], "/extra");
        // cost
        assert!((v["cost"]["total_cost_usd"].as_f64().unwrap() - 0.0123).abs() < 1e-9);
        // context_window percentages (42% used → 58% remaining). Tolerance is
        // loose enough to absorb the `0.42_f32 → f64` rounding.
        assert!((v["context_window"]["used_percentage"].as_f64().unwrap() - 42.0).abs() < 1e-3);
        assert!(
            (v["context_window"]["remaining_percentage"]
                .as_f64()
                .unwrap()
                - 58.0)
                .abs()
                < 1e-3
        );
    }

    // ── rate_limits (llm-client future-work batch 5, Task 4) ────────────

    /// `StatusLine.tsx:50-65`: each window spreads
    /// `{used_percentage: utilization * 100, resets_at}` into `rate_limits`
    /// only when BOTH values are present; an absent window leaves its key
    /// absent entirely (not `null`).
    #[test]
    fn rate_limits_field_mirrors_ts_shape() {
        let raw = RawUtilizationSnapshot {
            five_hour_utilization: Some(0.42),
            five_hour_resets_at: Some(1_750_000_000),
            seven_day_utilization: None,
            seven_day_resets_at: None,
        };
        let v = build_status_line_input(
            "claude-sonnet-4.5",
            "Claude Sonnet 4.5",
            &PathBuf::from("/work/cur"),
            &PathBuf::from("/work/proj"),
            &[],
            "0.8.0",
            0.0,
            0.0,
            Some(&raw),
        );
        let rl = v["rate_limits"]
            .as_object()
            .expect("rate_limits must be an object");
        let five = rl.get("five_hour").expect("five_hour window present");
        assert!((five["used_percentage"].as_f64().unwrap() - 42.0).abs() < 1e-9);
        assert_eq!(five["resets_at"].as_u64(), Some(1_750_000_000));
        assert!(
            rl.get("seven_day").is_none(),
            "absent window must omit its key (TS conditional spread)"
        );
    }

    /// `StatusLine.tsx:99-101`: `rate_limits` is OPTIONAL — TS spreads it in
    /// only when at least one window resolved
    /// (`...((rateLimits.five_hour || rateLimits.seven_day) && {...})`);
    /// with no raw utilization tracked the key is ABSENT entirely
    /// (statuslineSetup.ts:67 "Only present for subscribers after first API
    /// response").
    #[test]
    fn rate_limits_key_absent_when_no_window_resolved() {
        let v = build_status_line_input(
            "m",
            "M",
            &PathBuf::from("/c"),
            &PathBuf::from("/p"),
            &[],
            "0.8.0",
            0.0,
            0.0,
            None,
        );
        assert!(
            v.get("rate_limits").is_none(),
            "rate_limits must be omitted when no window resolved: {v:?}"
        );
    }

    #[test]
    fn parse_cost_usd_handles_formatted_and_garbage() {
        assert!((parse_cost_usd("$0.0042") - 0.0042).abs() < 1e-9);
        assert!((parse_cost_usd("$12.5") - 12.5).abs() < 1e-9);
        assert!(parse_cost_usd("").abs() < 1e-9);
        assert!(parse_cost_usd("n/a").abs() < 1e-9);
    }

    #[test]
    fn config_from_settings_parses_command_and_padding() {
        let v = json!({"type": "command", "command": "echo hi", "padding": 2});
        let cfg = StatusLineConfig::from_settings_value(&v).unwrap();
        assert_eq!(cfg.kind, "command");
        assert_eq!(cfg.command, "echo hi");
        assert_eq!(cfg.padding, 2);
        assert!(cfg.should_run(true));
        assert!(!cfg.should_run(false), "must fail-closed when untrusted");
    }

    #[test]
    fn config_rejects_non_command_type() {
        let v = json!({"type": "static", "command": "echo hi"});
        let cfg = StatusLineConfig::from_settings_value(&v).unwrap();
        assert!(!cfg.should_run(true), "only type==command runs");
    }

    #[test]
    fn config_none_when_no_command() {
        let v = json!({"type": "command"});
        assert!(StatusLineConfig::from_settings_value(&v).is_none());
    }

    #[test]
    fn config_padding_defaults_to_zero() {
        let v = json!({"type": "command", "command": "echo hi"});
        let cfg = StatusLineConfig::from_settings_value(&v).unwrap();
        assert_eq!(cfg.padding, 0);
    }

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_captures_and_transforms_stdout() {
        // `printf` emits two lines with surrounding blanks/ws; the transform
        // trims + drops the blank line.
        let out = run_status_line_command(
            "printf '  hello \\n\\n world \\n'",
            "{}",
            STATUS_LINE_TIMEOUT,
        );
        assert_eq!(out.as_deref(), Some("hello\nworld"));
    }

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_empty_output_is_none() {
        assert!(run_status_line_command("true", "{}", STATUS_LINE_TIMEOUT).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_nonzero_exit_is_none() {
        // Non-zero exit → None even if it wrote to stdout.
        let out = run_status_line_command("echo nope; exit 3", "{}", STATUS_LINE_TIMEOUT);
        assert!(out.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_reads_stdin_payload() {
        // `cat` echoes the JSON payload back; confirms stdin is fed + closed.
        let out = run_status_line_command("cat", "{\"k\":1}", STATUS_LINE_TIMEOUT);
        assert_eq!(out.as_deref(), Some("{\"k\":1}"));
    }

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_times_out() {
        // A command that sleeps past the (tiny) timeout returns None and is killed.
        let out = run_status_line_command("sleep 5", "{}", Duration::from_millis(50));
        assert!(out.is_none());
    }
}
