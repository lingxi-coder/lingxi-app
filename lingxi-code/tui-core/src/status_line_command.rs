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
//! `agent`, `remote`, `pr`, `worktree`. API duration, edit-line totals,
//! cumulative input/output usage, and the most recent usage decomposition are
//! supplied by the live cost snapshot. See [`StatusLineInputs`] for the
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
//! (rendered by the `custom` prop on `StatusLine`). Turn completion marks the
//! payload dirty immediately; `refreshInterval` additionally re-arms the pump
//! on its configured cadence. The payload's OPTIONAL `rate_limits` comes from
//! `AppState.raw_utilization`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
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

// Status-line commands are UI helpers, not bulk-output transports. Drain their
// pipe concurrently so a verbose command cannot deadlock before exit, but
// reject unreasonably large output instead of retaining it in the TUI process.
const MAX_STATUS_LINE_OUTPUT_BYTES: usize = 1024 * 1024;

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
    /// Optional periodic refresh cadence in seconds (minimum one second).
    pub refresh_interval: Option<Duration>,
    /// Suppress the built-in vim mode indicator when the custom command owns it.
    pub hide_vim_mode_indicator: bool,
    /// Settings tier that supplied the winning command.
    pub source: StatusLineSource,
    /// Frozen trust/hook-policy decision used at command spawn.
    pub execution_policy: StatusLineExecutionPolicy,
}

/// Parsed `subagentStatusLine` command configuration.
///
/// Subagent status commands share the same trust and managed-hook policy as
/// the main status line, but their output is a JSONL stream keyed by task id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentStatusLineConfig {
    /// The `subagentStatusLine.type` discriminator.
    pub kind: String,
    /// Shell command executed once for each refresh tick.
    pub command: String,
    /// Periodic refresh cadence. Claude refreshes this surface while agents
    /// are active; one second is the minimum/default cadence.
    pub refresh_interval: Duration,
    /// Settings tier that supplied the winning command.
    pub source: StatusLineSource,
    /// Frozen trust/hook-policy decision used before process creation.
    pub execution_policy: StatusLineExecutionPolicy,
}

/// Provenance of a configured status-line command.
///
/// `Unknown` is a sentinel, not a rung: the parser cannot attribute a command
/// it read in isolation, and the composition root must replace it. It is kept
/// as its own variant rather than folded into `Option` or into
/// [`protocol::Provenance`] because [`StatusLineExecutionPolicy::allows`] gates
/// child-process creation on this value, and an unattributed command must never
/// compare equal to a managed one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusLineSource {
    /// Parser-only/default provenance. Composition roots must replace this.
    #[default]
    Unknown,
    /// Attributed to a settings rung — `User`, `Project`, `Local`, `Flag` or
    /// `Managed` in practice.
    Known(protocol::Scope),
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
            && (!self.managed_hooks_only || source == StatusLineSource::Known(protocol::Scope::Managed))
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
        let refresh_interval = obj
            .get("refreshInterval")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds >= 1)
            .map(Duration::from_secs);
        let hide_vim_mode_indicator = obj
            .get("hideVimModeIndicator")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Some(Self {
            kind,
            command,
            padding,
            refresh_interval,
            hide_vim_mode_indicator,
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

impl SubagentStatusLineConfig {
    /// Parse a `subagentStatusLine` JSON object. Invalid and command-less
    /// values are ignored rather than producing an executable configuration.
    #[must_use]
    pub fn from_settings_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let command = obj.get("command").and_then(Value::as_str)?.to_string();
        let refresh_interval = obj
            .get("refreshInterval")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds >= 1)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(1));
        Some(Self {
            kind,
            command,
            refresh_interval,
            source: StatusLineSource::Unknown,
            execution_policy: StatusLineExecutionPolicy::default(),
        })
    }

    /// Attach settings provenance and the frozen execution policy.
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

    /// Whether this configuration may create a child process.
    #[must_use]
    pub fn should_run(&self, trusted: bool) -> bool {
        trusted
            && self.execution_policy.allows(self.source)
            && self.kind == "command"
            && !self.command.is_empty()
    }
}

/// One task in the `subagentStatusLine` stdin payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStatusLineTask {
    /// Stable task id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Task type wire value.
    #[serde(rename = "type")]
    pub task_type: String,
    /// Lifecycle status wire value.
    pub status: String,
    /// User-facing task description.
    pub description: String,
    /// Compact label for status display.
    pub label: String,
    /// Start time as Unix epoch milliseconds, or zero when unavailable.
    pub start_time: u64,
    /// Resolved model id, or an empty string when unavailable.
    pub model: String,
    /// Resolved effort level, when available.
    pub effort: Option<String>,
    /// Effective context-window size.
    pub context_window_size: u64,
    /// Latest token count for this task.
    pub token_count: u64,
    /// Recent token-count samples.
    pub token_samples: Vec<u64>,
    /// Task working directory.
    pub cwd: String,
}

/// One valid JSONL row emitted by a `subagentStatusLine` command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SubagentStatusLineOutput {
    /// Target task id. Missing means the row is the default for tasks without
    /// an id-specific row.
    #[serde(default)]
    pub id: Option<String>,
    /// Rendered content. An empty string intentionally hides the task row.
    pub content: String,
}

/// Extend the normal status-line payload with terminal columns and task rows.
#[must_use]
pub fn build_subagent_status_line_input(
    base: &Value,
    columns: u16,
    tasks: &[SubagentStatusLineTask],
) -> Value {
    let mut payload = base.clone();
    if !payload.is_object() {
        payload = json!({});
    }
    payload["columns"] = json!(columns);
    payload["tasks"] = serde_json::to_value(tasks).unwrap_or_else(|_| json!([]));
    payload
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
    /// `cost.total_api_duration_ms` from the cumulative live cost snapshot.
    pub total_api_duration_ms: u64,
    /// `cost.total_lines_added` from committed edit accounting.
    pub total_lines_added: u64,
    /// See [`Self::total_lines_added`].
    pub total_lines_removed: u64,
    /// Cumulative input tokens across successful requests.
    pub total_input_tokens: u64,
    /// Cumulative output tokens across successful requests.
    pub total_output_tokens: u64,
    /// Most recent successful model-response usage.
    pub current_usage: Option<&'a platform_api::CurrentUsageSnapshot>,
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
    // percentages are null. The latest response is kept separately from
    // cumulative totals, matching Claude Code's `current_usage` shape.
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
            "total_input_tokens": i.total_input_tokens,
            "total_output_tokens": i.total_output_tokens,
            "context_window_size": i.context_window_tokens,
            "current_usage": i.current_usage.map(|usage| json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_input_tokens": usage.cache_read_input_tokens,
                "cache_creation_input_tokens": usage.cache_creation_input_tokens,
            })).unwrap_or(Value::Null),
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
    let stdout = run_command_stdout(command, stdin_json, timeout, None)?;
    let formatted = format_custom_status_line(&stdout);
    (!formatted.is_empty()).then_some(formatted)
}

/// Execute and parse a `subagentStatusLine` JSONL response. Invalid JSON,
/// duplicate task ids, non-zero exit, timeout, and spawn failures all return
/// `None` so the caller can keep the built-in agent UI.
#[must_use]
pub fn run_subagent_status_line_command(
    command: &str,
    stdin_json: &str,
    columns: u16,
    timeout: Duration,
) -> Option<Vec<SubagentStatusLineOutput>> {
    let stdout = run_command_stdout(command, stdin_json, timeout, Some(columns))?;
    let mut rows = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut saw_default = false;
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let row = serde_json::from_str::<SubagentStatusLineOutput>(line).ok()?;
        match row.id.as_deref() {
            Some(id) if !id.is_empty() => {
                if !ids.insert(id.to_string()) {
                    return None;
                }
            }
            _ => {
                if saw_default {
                    return None;
                }
                saw_default = true;
            }
        }
        rows.push(row);
    }
    (!rows.is_empty()).then_some(rows)
}

/// Validate parsed subagent status rows against the task snapshot that was
/// supplied to the command. A command may emit one id-less default row, but
/// every active task must then be covered either by that default or by an
/// explicit row. Unknown ids and incomplete output fail open to the built-in
/// UI instead of silently hiding task state.
#[must_use]
pub fn validate_subagent_status_line_output(
    rows: &[SubagentStatusLineOutput],
    task_ids: &[String],
) -> bool {
    if rows.is_empty() || task_ids.is_empty() {
        return false;
    }

    let expected = task_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let mut explicit = std::collections::HashSet::new();
    let mut has_default = false;
    for row in rows {
        match row.id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) if expected.contains(id) => {
                if !explicit.insert(id) {
                    return false;
                }
            }
            Some(_) => return false,
            None if !has_default => has_default = true,
            None => return false,
        }
    }

    has_default || expected.iter().all(|id| explicit.contains(id))
}

fn run_command_stdout(
    command: &str,
    stdin_json: &str,
    timeout: Duration,
    columns: Option<u16>,
) -> Option<String> {
    use std::io::Write;

    let mut cmd = build_shell_command(command);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(columns) = columns {
        cmd.env("COLUMNS", columns.to_string());
    }

    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (stdout_tx, stdout_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut overflowed = false;
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let remaining = MAX_STATUS_LINE_OUTPUT_BYTES.saturating_sub(output.len());
                    let retained = remaining.min(read);
                    output.extend_from_slice(&chunk[..retained]);
                    overflowed |= retained != read;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = stdout_tx.send(None);
                    return;
                }
            }
        }
        let _ = stdout_tx.send(Some((output, overflowed)));
    });
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_json.as_bytes());
    }

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let remaining = timeout.saturating_sub(start.elapsed());
                let (output, overflowed) = stdout_rx.recv_timeout(remaining).ok()??;
                if overflowed {
                    return None;
                }
                return Some(String::from_utf8_lossy(&output).into_owned());
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
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
        let current_usage = platform_api::CurrentUsageSnapshot {
            input_tokens: 1_000,
            output_tokens: 200,
            cache_read_input_tokens: 300,
            cache_creation_input_tokens: 400,
        };
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
            total_input_tokens: 84_000,
            total_output_tokens: 2_000,
            current_usage: Some(&current_usage),
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
        // cost — full 206 block.
        assert!((v["cost"]["total_cost_usd"].as_f64().unwrap() - 0.0123).abs() < 1e-9);
        assert_eq!(v["cost"]["total_duration_ms"], 5000);
        assert_eq!(v["cost"]["total_api_duration_ms"], 0);
        assert_eq!(v["cost"]["total_lines_added"], 0);
        assert_eq!(v["cost"]["total_lines_removed"], 0);
        // context_window — full jj_ shape: integer o2n percentages
        // (84000/200000 → 42), cumulative totals, and latest-call usage.
        assert_eq!(v["context_window"]["total_input_tokens"], 84_000);
        assert_eq!(v["context_window"]["total_output_tokens"], 2_000);
        assert_eq!(v["context_window"]["context_window_size"], 200_000);
        assert_eq!(v["context_window"]["current_usage"]["input_tokens"], 1_000);
        assert_eq!(v["context_window"]["current_usage"]["output_tokens"], 200);
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
        let v = json!({
            "type": "command",
            "command": "echo hi",
            "padding": 2,
            "refreshInterval": 3,
            "hideVimModeIndicator": true
        });
        let cfg = StatusLineConfig::from_settings_value(&v).unwrap();
        assert_eq!(cfg.kind, "command");
        assert_eq!(cfg.command, "echo hi");
        assert_eq!(cfg.padding, 2);
        assert_eq!(cfg.refresh_interval, Some(Duration::from_secs(3)));
        assert!(cfg.hide_vim_mode_indicator);
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

    #[test]
    fn subagent_config_and_payload_preserve_task_contract() {
        let value = json!({
            "type": "command",
            "command": "render-agents",
            "refreshInterval": 2
        });
        let config = SubagentStatusLineConfig::from_settings_value(&value).unwrap();
        assert_eq!(config.refresh_interval, Duration::from_secs(2));
        assert!(config.should_run(true));

        let payload = build_subagent_status_line_input(
            &json!({"session_id":"s1","cwd":"/repo"}),
            132,
            &[SubagentStatusLineTask {
                id: "a123".into(),
                name: "Explore".into(),
                task_type: "local_agent".into(),
                status: "running".into(),
                description: "Map runtime".into(),
                label: "Map runtime".into(),
                start_time: 1_700_000_000_000,
                model: "claude-sonnet-5".into(),
                effort: Some("high".into()),
                context_window_size: 200_000,
                token_count: 42,
                token_samples: vec![12, 42],
                cwd: "/repo".into(),
            }],
        );
        assert_eq!(payload["session_id"], "s1");
        assert_eq!(payload["columns"], 132);
        assert_eq!(payload["tasks"][0]["id"], "a123");
        assert_eq!(payload["tasks"][0]["type"], "local_agent");
        assert_eq!(payload["tasks"][0]["startTime"], 1_700_000_000_000_u64);
        assert_eq!(payload["tasks"][0]["contextWindowSize"], 200_000);
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

    #[cfg(unix)]
    #[test]
    fn run_status_line_command_drains_and_rejects_oversized_stdout() {
        let started = Instant::now();
        let out =
            run_status_line_command("head -c 1048577 /dev/zero", "{}", Duration::from_secs(2));
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[cfg(unix)]
    #[test]
    fn subagent_status_line_parses_jsonl_and_sets_columns() {
        let command = "printf '{\"id\":\"a1\",\"content\":\"%s\"}\\n{\"content\":\"default\"}\\n' \"$COLUMNS\"";
        let rows = run_subagent_status_line_command(command, "{}", 91, STATUS_LINE_TIMEOUT)
            .expect("valid jsonl");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id.as_deref(), Some("a1"));
        assert_eq!(rows[0].content, "91");
        assert_eq!(rows[1].id, None);
        assert_eq!(rows[1].content, "default");
    }

    #[cfg(unix)]
    #[test]
    fn subagent_status_line_invalid_or_duplicate_rows_fail_open() {
        assert!(run_subagent_status_line_command(
            "printf 'not-json\\n'",
            "{}",
            80,
            STATUS_LINE_TIMEOUT
        )
        .is_none());
        assert!(run_subagent_status_line_command(
            "printf '{\"id\":\"a1\",\"content\":\"x\"}\\n{\"id\":\"a1\",\"content\":\"y\"}\\n'",
            "{}",
            80,
            STATUS_LINE_TIMEOUT
        )
        .is_none());
        assert!(
            run_subagent_status_line_command("printf '\\n'", "{}", 80, STATUS_LINE_TIMEOUT)
                .is_none()
        );
    }

    #[test]
    fn subagent_status_line_output_must_cover_only_known_tasks() {
        let tasks = vec!["a1".to_string(), "a2".to_string()];
        let explicit = vec![
            SubagentStatusLineOutput {
                id: Some("a1".to_string()),
                content: "one".to_string(),
            },
            SubagentStatusLineOutput {
                id: Some("a2".to_string()),
                content: "two".to_string(),
            },
        ];
        assert!(validate_subagent_status_line_output(&explicit, &tasks));

        let default = vec![SubagentStatusLineOutput {
            id: None,
            content: "all".to_string(),
        }];
        assert!(validate_subagent_status_line_output(&default, &tasks));

        assert!(!validate_subagent_status_line_output(
            &explicit[..1],
            &tasks
        ));
        assert!(!validate_subagent_status_line_output(
            &[SubagentStatusLineOutput {
                id: Some("other".to_string()),
                content: "unknown".to_string(),
            }],
            &tasks
        ));
    }
}
