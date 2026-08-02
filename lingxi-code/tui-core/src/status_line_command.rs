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
//! **Input payload mirrors the 2.1.206 `Wj_` builder** — key set AND order:
//! `session_id`, `transcript_path`, `cwd`, `model{id,display_name}`,
//! `workspace{current_dir,project_dir,added_dirs}`, `version`,
//! `output_style{name}`, `cost{total_cost_usd,total_duration_ms,
//! total_api_duration_ms,total_lines_added,total_lines_removed}`,
//! `context_window{total_input_tokens,total_output_tokens,
//! context_window_size,current_usage,used_percentage,remaining_percentage}`,
//! `exceeds_200k_tokens`, `fast_mode`, [`effort{level}`], `thinking{enabled}`,
//! [`rate_limits`], [`vim{mode}`] (bracketed = conditional, omitted exactly
//! when the binary's conditional spread omits them). Groups the port does not
//! track are omitted — matching a claude-code session where that state is
//! absent: `session_name`, `workspace.git_worktree`/`repo`, `prompt_id`,
//! `agent`, `remote`, `pr`, `worktree`. RESIDUAL (key present, value
//! degraded): `cost.total_api_duration_ms`/`total_lines_added`/
//! `total_lines_removed` = 0, `context_window.total_output_tokens` = 0,
//! `current_usage` = null — the port has no per-call duration / edit-line /
//! usage-decomposition counters. See [`StatusLineInputs`] for the
//! multi-provider semantics of `fast_mode`/`effort`/`thinking`.
//!
//! **Trust and managed policy are evaluated before spawn.** The resolved config
//! carries its source and an immutable [`StatusLineExecutionPolicy`]; untrusted
//! workspaces, `disableAllHooks`, and managed-only violations produce no child
//! process.
//!
//! **The execution pump is wired** (A6 batch-6 Task 2): the debounced,
//! single-flight pump in `root.rs` calls `crate::state::build_pump_payload`
//! (which calls [`build_status_line_input`]) then [`run_status_line_command`]
//! off-thread, and writes the formatted result onto `AppState.status_line_text`
//! (rendered by the `custom` prop on `StatusLine`). It re-arms only on
//! `TurnEvent::TurnEnded` (`AppState.status_line_dirty`), the TUI analog of
//! claude-code's `StatusLine.tsx` re-run on `lastAssistantMessageId`. The
//! payload's OPTIONAL `rate_limits` comes from `AppState.raw_utilization`.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Post-process the command's stdout into the status line: trim, split on
/// newlines, trim each line, drop empties, join with `\n` (claude-code
/// `executeStatusLineCommand` output shaping; folded in from the retired
/// iocraft `status_line` component).
fn format_custom_status_line(stdout: &str) -> String {
    stdout
        .trim()
        .split('\n')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// (2.1.206 removed the payload's `hook_event_name` — `Rf()` emits none — so
// the old `STATUS_HOOK_EVENT_NAME` const is retired.)

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
    /// Settings tier that supplied the winning command.
    pub source: StatusLineSource,
    /// Frozen trust/hook-policy decision used at command spawn.
    pub execution_policy: StatusLineExecutionPolicy,
}

/// Provenance of a configured status-line command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusLineSource {
    /// Parser-only/default provenance. Composition roots must replace this.
    #[default]
    Unknown,
    /// User settings.
    User,
    /// Shared project settings.
    Project,
    /// Local project settings.
    Local,
    /// `--settings` flag input.
    Flag,
    /// Enterprise-managed settings.
    Managed,
}

/// Spawn-time policy for status-line commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusLineExecutionPolicy {
    /// Whether the current workspace passed the trust gate.
    pub workspace_trusted: bool,
    /// Effective all-hooks kill switch.
    pub disable_all_hooks: bool,
    /// Whether only managed hook-like commands may run.
    pub managed_hooks_only: bool,
}

impl Default for StatusLineExecutionPolicy {
    fn default() -> Self {
        Self {
            workspace_trusted: true,
            disable_all_hooks: false,
            managed_hooks_only: false,
        }
    }
}

impl StatusLineExecutionPolicy {
    /// Whether `source` may create a child process.
    #[must_use]
    pub fn allows(self, source: StatusLineSource) -> bool {
        self.workspace_trusted
            && !self.disable_all_hooks
            && (!self.managed_hooks_only || source == StatusLineSource::Managed)
    }
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
            source: StatusLineSource::Unknown,
            execution_policy: StatusLineExecutionPolicy::default(),
        })
    }

    /// Attach settings provenance and the already-resolved execution policy.
    #[must_use]
    pub fn with_execution_policy(
        mut self,
        source: StatusLineSource,
        execution_policy: StatusLineExecutionPolicy,
    ) -> Self {
        self.source = source;
        self.execution_policy = execution_policy;
        self
    }

    /// claude-code `executeStatusLineCommand` runs only when
    /// `statusLine.type === 'command'`. `trusted` folds the simplified
    /// trust/managed gating: fail-closed when the workspace is not trusted.
    #[must_use]
    pub fn should_run(&self, trusted: bool) -> bool {
        trusted
            && self.execution_policy.allows(self.source)
            && self.kind == "command"
            && !self.command.is_empty()
    }
}

/// Inputs for [`build_status_line_input`] — one field per 2.1.206 payload
/// group the port can populate (the `Wj_` builder's data, minus the groups the
/// port does not track: `session_name`, `workspace.git_worktree`/`repo`,
/// `prompt_id`, `agent`, `remote`, `pr`, `worktree` — all CONDITIONAL spreads
/// in the binary, so omitting them matches a claude-code session where that
/// state is absent).
///
/// Multi-provider note (`fast_mode` / `effort` / `thinking` are CLAUDE-syntax
/// concepts): the values must reflect the PROVIDER-MAPPED reality of the
/// active model, not the Claude default —
/// - `effort_level`: `None` OMITS the key, matching the binary's `Bx(model)`
///   support gate; a provider without an effort/reasoning-effort equivalent
///   must pass `None`.
/// - `fast_mode`: always emitted (binary shape); a provider without a fast
///   tier is simply `false` (it can never be enabled there).
/// - `thinking_enabled`: always emitted; the value is whether the active
///   model's thinking/reasoning equivalent (Claude extended thinking, OpenAI
///   `reasoning_effort`, …) is enabled — `false` for models without one.
#[derive(Debug, Clone, Default)]
pub struct StatusLineInputs<'a> {
    /// `session_id` (binary `Rf()` base field).
    pub session_id: &'a str,
    /// `transcript_path` (binary `Rf()` base field).
    pub transcript_path: &'a str,
    /// Wire model id (`model.id`).
    pub model_id: &'a str,
    /// Human model label (`model.display_name`).
    pub model_display_name: &'a str,
    /// `cwd` + `workspace.current_dir`.
    pub current_dir: &'a str,
    /// `workspace.project_dir`.
    pub project_dir: &'a str,
    /// `workspace.added_dirs`.
    pub added_dirs: &'a [String],
    /// `version` string.
    pub version: &'a str,
    /// `output_style.name` (binary default `"default"`).
    pub output_style: &'a str,
    /// `cost.total_cost_usd`.
    pub cost_usd: f64,
    /// `cost.total_duration_ms` (wall time since session start).
    pub total_duration_ms: u64,
    /// `cost.total_api_duration_ms`. RESIDUAL: the port does not track
    /// cumulative API-call duration — always `0` (key present for script
    /// compatibility).
    pub total_api_duration_ms: u64,
    /// `cost.total_lines_added` / `total_lines_removed`. RESIDUAL: the port
    /// has no edit-line counters — always `0`.
    pub total_lines_added: u64,
    /// See [`Self::total_lines_added`].
    pub total_lines_removed: u64,
    /// Raw context token estimate (the auto-compact gate's input estimate) —
    /// `context_window.total_input_tokens` + the `exceeds_200k_tokens`
    /// derivation. `0` = no usage yet (percentages go `null`, binary `o2n`).
    pub used_tokens: u64,
    /// Effective context window size in tokens (`context_window_size`).
    pub context_window_tokens: u64,
    /// `fast_mode` (see the multi-provider note above).
    pub fast_mode: bool,
    /// `effort.level` — `None` omits the key (unsupported model/provider or
    /// untracked), matching the binary's `...Bx(y)&&{effort:{…}}` gate.
    pub effort_level: Option<&'a str>,
    /// `thinking.enabled` (see the multi-provider note above).
    pub thinking_enabled: bool,
    /// `vim.mode` — `Some("INSERT"|"NORMAL")` only when vim bindings are on
    /// (binary `...D$()&&{vim:{mode:u??"INSERT"}}`); `None` omits the key.
    pub vim_mode: Option<&'a str>,
    /// Latest per-window rate-limit snapshot → the optional `rate_limits`.
    pub raw_utilization: Option<&'a RawUtilizationSnapshot>,
}

/// Build the JSON stdin payload for the status-line command — the 2.1.206
/// `Wj_` payload in its exact key ORDER: `session_id`, `transcript_path`,
/// `cwd`, `model`, `workspace`, `version`, `output_style`, `cost`,
/// `context_window`, `exceeds_200k_tokens`, `fast_mode`, [`effort`],
/// `thinking`, [`rate_limits`], [`vim`]. (2.1.206 dropped the older
/// `hook_event_name` field — `Rf()` does not emit one — so neither do we.)
#[must_use]
pub fn build_status_line_input(inputs: &StatusLineInputs<'_>) -> Value {
    let i = inputs;
    // `context_window` (binary `jj_`/`o2n`): with usage, used% =
    // round(tokens/window*100) clamped 0-100 as an INTEGER; without usage both
    // percentages are null. RESIDUALS: the port's estimate has no output-token
    // / cache decomposition, so `total_output_tokens` is `0` and
    // `current_usage` is `null` (binary sends the raw usage object).
    let has_usage = i.used_tokens > 0 && i.context_window_tokens > 0;
    let (used, remaining) = if has_usage {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = ((i.used_tokens as f64 / i.context_window_tokens as f64) * 100.0).round() as u64;
        let pct = pct.min(100);
        (json!(pct), json!(100 - pct))
    } else {
        (Value::Null, Value::Null)
    };
    // `exceeds_200k_tokens` (binary `AJn`: last usage total > 200000; the
    // port's estimate omits output tokens — a documented approximation).
    let exceeds_200k = i.used_tokens > 200_000;

    let mut payload = json!({
        "session_id": i.session_id,
        "transcript_path": i.transcript_path,
        "cwd": i.current_dir,
        "model": {
            "id": i.model_id,
            "display_name": i.model_display_name,
        },
        "workspace": {
            "current_dir": i.current_dir,
            "project_dir": i.project_dir,
            "added_dirs": i.added_dirs,
        },
        "version": i.version,
        "output_style": { "name": i.output_style },
        "cost": {
            "total_cost_usd": i.cost_usd,
            "total_duration_ms": i.total_duration_ms,
            "total_api_duration_ms": i.total_api_duration_ms,
            "total_lines_added": i.total_lines_added,
            "total_lines_removed": i.total_lines_removed,
        },
        "context_window": {
            "total_input_tokens": i.used_tokens,
            "total_output_tokens": 0,
            "context_window_size": i.context_window_tokens,
            "current_usage": Value::Null,
            "used_percentage": used,
            "remaining_percentage": remaining,
        },
        "exceeds_200k_tokens": exceeds_200k,
        "fast_mode": i.fast_mode,
    });
    // Conditional groups, in Wj_ spread order: effort → thinking →
    // rate_limits → vim.
    if let Some(level) = i.effort_level {
        payload["effort"] = json!({ "level": level });
    }
    payload["thinking"] = json!({ "enabled": i.thinking_enabled });
    // `rate_limits` is OPTIONAL: TS only spreads it into the payload when at
    // least one window resolved (`...((I.five_hour||I.seven_day)&&{rate_limits:I})`).
    // A window with either header missing omits its key entirely.
    let mut rate_limits = serde_json::Map::new();
    if let Some(raw) = i.raw_utilization {
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
    if !rate_limits.is_empty() {
        payload["rate_limits"] = Value::Object(rate_limits);
    }
    if let Some(mode) = i.vim_mode {
        payload["vim"] = json!({ "mode": mode });
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

    #[test]
    fn build_status_line_input_shape() {
        let v = build_status_line_input(&StatusLineInputs {
            session_id: "sess-1",
            transcript_path: "/t/sess-1.jsonl",
            model_id: "claude-sonnet-4.5",
            model_display_name: "Claude Sonnet 4.5",
            current_dir: "/work/cur",
            project_dir: "/work/proj",
            added_dirs: &["/extra".to_string()],
            version: "0.8.0",
            output_style: "default",
            cost_usd: 0.0123,
            total_duration_ms: 5000,
            used_tokens: 84_000,
            context_window_tokens: 200_000,
            thinking_enabled: true,
            ..Default::default()
        });
        // 206 base fields (Rf): session_id / transcript_path / cwd — and NO
        // hook_event_name (removed in 2.1.206).
        assert_eq!(v["session_id"], "sess-1");
        assert_eq!(v["transcript_path"], "/t/sess-1.jsonl");
        assert_eq!(v["cwd"], "/work/cur");
        assert!(v.get("hook_event_name").is_none());
        assert_eq!(v["version"], "0.8.0");
        // model
        assert_eq!(v["model"]["id"], "claude-sonnet-4.5");
        assert_eq!(v["model"]["display_name"], "Claude Sonnet 4.5");
        // workspace
        assert_eq!(v["workspace"]["current_dir"], "/work/cur");
        assert_eq!(v["workspace"]["project_dir"], "/work/proj");
        assert_eq!(v["workspace"]["added_dirs"][0], "/extra");
        // output_style
        assert_eq!(v["output_style"]["name"], "default");
        // cost — full 206 block (durations/lines are residual zeros).
        assert!((v["cost"]["total_cost_usd"].as_f64().unwrap() - 0.0123).abs() < 1e-9);
        assert_eq!(v["cost"]["total_duration_ms"], 5000);
        assert_eq!(v["cost"]["total_api_duration_ms"], 0);
        assert_eq!(v["cost"]["total_lines_added"], 0);
        assert_eq!(v["cost"]["total_lines_removed"], 0);
        // context_window — full jj_ shape: integer o2n percentages
        // (84000/200000 → 42), token totals, null current_usage.
        assert_eq!(v["context_window"]["total_input_tokens"], 84_000);
        assert_eq!(v["context_window"]["total_output_tokens"], 0);
        assert_eq!(v["context_window"]["context_window_size"], 200_000);
        assert!(v["context_window"]["current_usage"].is_null());
        assert_eq!(v["context_window"]["used_percentage"], 42);
        assert_eq!(v["context_window"]["remaining_percentage"], 58);
        // flags
        assert_eq!(v["exceeds_200k_tokens"], false);
        assert_eq!(v["fast_mode"], false);
        assert_eq!(v["thinking"]["enabled"], true);
        // Conditional groups absent when untracked/off.
        assert!(v.get("effort").is_none());
        assert!(v.get("vim").is_none());
        assert!(v.get("rate_limits").is_none());
    }

    #[test]
    fn no_usage_yields_null_percentages_and_exceeds_derives_from_tokens() {
        // No usage yet → o2n's null percentages.
        let v = build_status_line_input(&StatusLineInputs {
            context_window_tokens: 200_000,
            ..Default::default()
        });
        assert!(v["context_window"]["used_percentage"].is_null());
        assert!(v["context_window"]["remaining_percentage"].is_null());
        assert_eq!(v["exceeds_200k_tokens"], false);
        // 250k tokens → exceeds_200k true, used% clamped to 100.
        let v = build_status_line_input(&StatusLineInputs {
            used_tokens: 250_000,
            context_window_tokens: 200_000,
            ..Default::default()
        });
        assert_eq!(v["exceeds_200k_tokens"], true);
        assert_eq!(v["context_window"]["used_percentage"], 100);
        assert_eq!(v["context_window"]["remaining_percentage"], 0);
    }

    #[test]
    fn conditional_effort_and_vim_render_when_present() {
        let v = build_status_line_input(&StatusLineInputs {
            effort_level: Some("high"),
            vim_mode: Some("NORMAL"),
            fast_mode: true,
            ..Default::default()
        });
        assert_eq!(v["effort"]["level"], "high");
        assert_eq!(v["vim"]["mode"], "NORMAL");
        assert_eq!(v["fast_mode"], true);
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
        let v = build_status_line_input(&StatusLineInputs {
            model_id: "claude-sonnet-4.5",
            model_display_name: "Claude Sonnet 4.5",
            current_dir: "/work/cur",
            project_dir: "/work/proj",
            version: "0.8.0",
            raw_utilization: Some(&raw),
            ..Default::default()
        });
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
        let v = build_status_line_input(&StatusLineInputs {
            model_id: "m",
            model_display_name: "M",
            current_dir: "/c",
            project_dir: "/p",
            version: "0.8.0",
            ..Default::default()
        });
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
