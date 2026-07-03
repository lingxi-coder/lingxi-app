//! Orchestrator-level traits — the public surface that slash commands
//! (via `SlashContext`, M5-09) and the CLI binary (M5-12) consume.
//!
//! The concrete `ConversationOrchestrator` lives in `lingxi-orchestrator`;
//! these traits live here so consumers can depend on them without pulling
//! in the orchestrator (preserves the leaf position of `lingxi-traits`).
//!
//! See spec §2.3 (key traits) for the matched design.

use async_trait::async_trait;
use protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Snapshot of the latest provider rate-limit headers seen on the live path.
///
/// Returned by [`OrchestratorHandle::last_rate_limit_info`] so callers (e.g.
/// the TUI rate-limit status ticker) can inspect all three header values
/// without depending on the orchestrator-internal `RateLimitInfo` struct.
///
/// All fields are `Option<String>` — a missing header means the provider did
/// not send it in the most-recent 2xx response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitSnapshot {
    /// `anthropic-ratelimit-type` (e.g. `"five_hour"`, `"seven_day"`).
    pub rate_limit_type: Option<String>,
    /// Overage status: `"allowed"`, `"allowed_warning"`, or `"rejected"`.
    pub overage_status: Option<String>,
    /// Why overage is disabled (e.g. `"out_of_credits"`) — drives upsell copy
    /// parity. Maps `anthropic-ratelimit-unified-overage-disabled-reason`.
    pub overage_disabled_reason: Option<String>,
}

/// Snapshot of cumulative cost at a single point in time.
///
/// Lightweight echo of `cost::SessionCostSummary` — see that type for
/// the canonical session-scope rollup. We keep a leaf-friendly mirror here
/// so `lingxi-traits` does not need to depend on `lingxi-cost`.
///
/// M5-11 added the `total_usd`, `input_tokens`, `output_tokens`, `api_calls`,
/// and `session_duration` fields used by the `/cost` slash command's
/// locked render template. The legacy `total_nano_usd` and `total_tokens`
/// fields remain for back-compat with M5-02's `EndTurn` output event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostSnapshot {
    /// Session whose cost this snapshot describes.
    pub session_id: SessionId,
    /// Cumulative cost in nano-USD (legacy field — still consumed by `EndTurn`).
    pub total_nano_usd: u64,
    /// Cumulative tokens (input + output across all models — legacy field).
    pub total_tokens: u64,
    /// Cumulative cost in USD (4-decimal precision in displays).
    #[serde(default)]
    pub total_usd: f64,
    /// Cumulative input tokens across all turns.
    #[serde(default)]
    pub input_tokens: u64,
    /// Cumulative output tokens across all turns.
    #[serde(default)]
    pub output_tokens: u64,
    /// Cumulative cache-read input tokens across all turns.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Cumulative cache-creation input tokens across all turns.
    #[serde(default)]
    pub cache_creation_tokens: u64,
    /// Cumulative successful `messages_create` calls.
    #[serde(default)]
    pub api_calls: u32,
    /// Elapsed time since the session started.
    #[serde(default)]
    pub session_duration: std::time::Duration,
}

/// Result of a `force_compact` operation. M5-10 wires `/compact` against
/// this surface; M5-02 only defines the type for forward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSummary {
    /// Number of messages in the session BEFORE compaction.
    pub messages_before: u32,
    /// Number of messages in the session AFTER compaction.
    pub messages_after: u32,
    /// Approximate bytes saved (summary token count delta × 4, as a UX
    /// estimate — exact accounting lives in `lingxi-compaction`).
    pub bytes_saved: u64,
}

/// Errors surfaced through the orchestrator's public handle.
///
/// Distinct from `orchestrator::OrchestratorError` because the
/// handle surface deliberately hides the API-error variants from slash
/// command authors (they cannot meaningfully act on a 429). Implementations
/// MAY wrap `OrchestratorError` and project a coarse `HandleError`.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum HandleError {
    /// The requested action could not be completed (e.g. `clear` during a
    /// turn-in-flight). The payload is a human-readable reason.
    #[error("handle action failed: {0}")]
    ActionFailed(String),
    /// The requested operation is not implemented on this handle.
    /// Used by the default `OrchestratorHandle::run_turn_streaming_with_cancel`
    /// impl (M6-03) so existing handle implementations (M5-13 stdio REPL
    /// path) don't need to override.
    #[error("operation not implemented: {0}")]
    Unimplemented(String),
}

/// Outcome of a TUI-driven turn invoked via
/// [`OrchestratorHandle::run_turn_streaming_with_cancel`]. (M6-03)
///
/// Mirrors `orchestrator::TurnOutcome` so the trait surface in
/// `lingxi-traits` does not depend on the orchestrator crate. Map between
/// the two in `orchestrator::handle_impl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Model returned a natural stop reason and the turn loop ended.
    EndTurn,
    /// The orchestrator's `max_turns` budget was reached before `end_turn`.
    MaxTurns,
    /// The cancel token fired mid-turn; the orchestrator unwound the
    /// current API call and returned early.
    Cancelled,
}

/// Result of [`OrchestratorHandle::open_memory_editor`] (M5-10).
///
/// Returned to `/memory`'s handler so it can render the locked
/// `"Edited {path} (exit {code})."` template. Carries the path the editor
/// was launched against (which may have been created if absent) and the
/// editor process's exit code (0 on success; non-zero on user cancel /
/// editor error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEditorOutcome {
    /// The LINGXI.md path that was edited (may have been created if absent).
    pub edited_path: PathBuf,
    /// Exit code of the spawned `$EDITOR` process. 0 = success.
    pub exit_code: i32,
}

// ────────────────────────────────────────────────────────────────────────────
// M5-11 info structs (used by `/mcp`, `/hooks`, `/agents`, `/status`, `/doctor`)
// ────────────────────────────────────────────────────────────────────────────

/// One MCP server entry returned by [`OrchestratorHandle::list_mcp_servers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInfo {
    /// Server name as registered in settings.
    pub name: String,
    /// Connection status at snapshot time.
    pub status: McpStatus,
    /// Transport kind: `"stdio"`, `"sse"`, or `"http"`.
    pub transport: String,
}

/// Connection status for an MCP server in [`McpServerInfo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// Connected and healthy.
    Connected,
    /// Disconnected — either never connected or cleanly shut down.
    Disconnected,
    /// Connection failed with the wrapped reason.
    Error(String),
}

/// One hook entry returned by [`OrchestratorHandle::list_hooks`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookInfo {
    /// Hook identifier.
    pub name: String,
    /// Hook event (e.g. `"PreToolUse"`, `"PostToolUse"`, `"Stop"`, `"Notification"`).
    pub event: String,
    /// Optional matcher regex (tool-name pattern).
    pub matcher: Option<String>,
    /// Timeout in milliseconds (default `60_000` if unset).
    pub timeout_ms: u64,
    /// (hooks-detail-fields-divergent) Executor kind (claude-code
    /// `config.type`): `"command"` / `"http"` / `"agent"` / `"prompt"`, or the
    /// LingXi-only `"builtin"` (an in-process Rust handler; no TS analogue).
    pub hook_type: String,
    /// (hooks-detail-fields-divergent) Human-readable origin (claude-code
    /// `hookSourceDescriptionDisplayString`), e.g. `"User settings
    /// (~/.lingxi/settings.json)"`.
    pub source: String,
    /// (hooks-detail-fields-divergent) The executor's primary content field
    /// (claude-code `getContentFieldValue`): the shell command line for
    /// `"command"`, the URL for `"http"`, the prompt for `"agent"`/`"prompt"`,
    /// the handler id for the LingXi-only `"builtin"`.
    pub content: String,
    /// (hooks-detail-fields-divergent) Custom status message shown while the
    /// hook runs, if the definition set one.
    pub status_message: Option<String>,
}

/// One subagent entry returned by [`OrchestratorHandle::list_agents`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentInfo {
    /// Agent name (matches the markdown filename without extension).
    pub name: String,
    /// Human-readable description (may be truncated by callers).
    pub description: String,
    /// Tool allow-list. Meaningful only when `wildcard_tools` is `false`;
    /// empty here then means "no tools", not "all tools" (agents-03).
    pub tools_allowed: Vec<String>,
    /// `true` when the agent's tool policy is `AgentToolPolicy::All`
    /// (claude-code: `tools` frontmatter omitted) — distinguishes "every
    /// tool" from an explicit empty allow-list, which `tools_allowed` alone
    /// cannot (both lower to an empty `Vec`).
    pub wildcard_tools: bool,
    /// (agents-08) Source-group display label (claude-code
    /// `AGENT_SOURCE_GROUPS`): `"User agents"`, `"Project agents"`, `"Local
    /// agents"`, `"Managed agents"`, `"Plugin agents"`, `"CLI arg agents"`,
    /// or `"Built-in agents"`. Drives the `/agents` list's section grouping.
    /// Empty string defaults rows into the trailing built-in section.
    pub source_group: String,
}

/// Aggregate diagnostic report returned by [`OrchestratorHandle::run_doctor_checks`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    /// Individual check results in execution order.
    pub checks: Vec<DoctorCheck>,
    /// Summary tallies (pass/warn/fail counts).
    pub summary: DoctorSummary,
}

/// One `/doctor` check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    /// Check identifier (`"config-dir"`, `"api-key"`, etc.).
    pub name: String,
    /// Pass/warn/fail outcome.
    pub status: CheckStatus,
    /// Optional detail string (rendered on a second indented line if `Some`).
    pub detail: Option<String>,
}

/// Outcome of a single [`DoctorCheck`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Check succeeded.
    Pass,
    /// Check produced a warning (non-fatal anomaly).
    Warn,
    /// Check failed.
    Fail,
}

/// Pass/warn/fail tallies in a [`DoctorReport`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorSummary {
    /// Number of checks that returned [`CheckStatus::Pass`].
    pub passed: u32,
    /// Number of checks that returned [`CheckStatus::Warn`].
    pub warnings: u32,
    /// Number of checks that returned [`CheckStatus::Fail`].
    pub failed: u32,
}

/// Snapshot returned by [`OrchestratorHandle::get_status_snapshot`] for the
/// `/status` panel. All fields are populated synchronously at snapshot time.
///
/// Note: `Eq` is intentionally not derived because `total_cost_usd: f64` does
/// not implement `Eq`. Use `PartialEq` for assertions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusSnapshot {
    /// Current session id (as a stable string for display).
    pub session_id: String,
    /// Active model name (e.g. `"claude-opus-4-7"`).
    pub model: String,
    /// Total messages in the session history.
    pub n_messages: u32,
    /// Cumulative cost in USD.
    pub total_cost_usd: f64,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// MCP servers currently in `Connected` state.
    pub n_mcp_connected: u32,
    /// MCP servers configured (any state).
    pub n_mcp_total: u32,
    /// Hooks registered.
    pub n_hooks: u32,
    /// Subagents available.
    pub n_agents: u32,
    /// Session start time, RFC 3339 (`"YYYY-MM-DDTHH:MM:SSZ"`, UTC).
    pub started_at: String,
    /// Working directory used to launch the session.
    pub cwd: PathBuf,
    /// Active coordinator-team workers at snapshot time (T21).
    ///
    /// `0` for a non-coordinator session (the `Default`), so existing
    /// constructors that use `..Default::default()` keep their behavior. A
    /// coordinator session surfaces the live `TeamRegistry::active_worker_count`
    /// here so `/status` can echo the same scalar the PUSH
    /// `CoordinatorStatus` feed carries.
    pub active_workers: u32,
    /// (settings-status-missing-mcp-and-setting-sources) Display strings for
    /// every settings-file tier that currently has a file on disk (claude-code
    /// `buildSettingSourcesProperties`'s `sourcesWithSettings` filter), e.g.
    /// `"Project settings (.lingxi/settings.json)"`. Empty when none exist
    /// (the `/status` row is omitted entirely, matching TS).
    pub setting_sources: Vec<String>,
}

/// One model entry for the grouped `/model` picker. Sourced from the llm-client
/// provider catalog: `display_model` is the human label, `request_model` is the
/// wire id passed to `switch_model`, `provider_id` is the stable grouping key,
/// and `provider_label` is the human provider header (e.g. "`GitHub` Copilot").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListing {
    /// Human-facing model label (e.g. "`DeepSeek` Chat").
    pub display_model: String,
    /// Provider-local wire model id (what `switch_model` accepts).
    pub request_model: String,
    /// Stable provider key for grouping + recents (the catalog profile name).
    pub provider_id: String,
    /// Human provider header (e.g. "`DeepSeek`", "`GitHub` Copilot").
    pub provider_label: String,
    /// Optional one-line model description, rendered as a dimmed line beneath the
    /// row in the `/model` picker (claude-code `ListItem` renders it under the
    /// label with `paddingLeft={2}` + `color="inactive"`). `None` ⇒ no extra line.
    #[serde(default)]
    pub description: Option<String>,
}

/// Parse a (possibly `profile/model`) text reference against the live model
/// listings into `(request_model, profile)`.
///
/// Qualified only when the first `/`-segment is a known profile (`provider_id`)
/// AND the remainder is a `request_model` under that profile; otherwise the
/// whole string is returned as a bare id (so openrouter ids that contain `/`,
/// e.g. `openai/gpt-4o`, route as bare ids). `profile = None` ⇒ unscoped.
#[must_use]
pub fn parse_model_ref(input: &str, listings: &[ModelListing]) -> (String, Option<String>) {
    if let Some((prefix, rest)) = input.split_once('/') {
        if listings
            .iter()
            .any(|l| l.provider_id == prefix && l.request_model == rest)
        {
            return (rest.to_string(), Some(prefix.to_string()));
        }
    }
    (input.to_string(), None)
}

/// The curated "latest few" models surfaced per provider in the `/model` picker
/// and the no-arg `/model` listing. claude-code hand-picks a short list
/// (`utils/model/modelOptions.ts`) instead of dumping the whole catalog (~460
/// models, 338 of them OpenRouter); we mirror that. Keyed by the stable
/// `provider_id` (the catalog profile name, e.g. `"glm-coding"`, NOT the slice
/// filename) + the wire `request_model`. Note the wire ids differ per provider
/// (Anthropic/native `claude-opus-4-8` dashes vs the GitHub Copilot proxy's
/// `claude-opus-4.8` dots). Any provider/model not listed is non-curated;
/// callers keep the user's current + recent models visible separately.
/// OpenRouter is intentionally absent — an aggregator passthrough, so a
/// connected user should pick a first-class provider for a curated set.
///
/// Shared by the TUI picker (`build_model_entries`/`is_shown_model`) and the
/// mobile/CLI listings so the whitelist has ONE source of truth.
#[must_use]
pub fn is_curated_model(provider_id: &str, request_model: &str) -> bool {
    match provider_id {
        "anthropic" | "builtin" => matches!(
            request_model,
            // claude-sonnet-5: the 2.1.198 default first-party Sonnet.
            "claude-sonnet-5"
                | "claude-sonnet-4-6"
                | "claude-opus-4-8"
                | "claude-haiku-4-5"
                | "claude-fable-5"
        ),
        "openai" => matches!(request_model, "gpt-5.5" | "gpt-5.4" | "gpt-5.4-mini"),
        "openai-chatgpt" => matches!(request_model, "gpt-5.3-codex" | "gpt-5-codex"),
        "deepseek" => matches!(
            request_model,
            "deepseek-chat" | "deepseek-reasoner" | "deepseek-v4-pro"
        ),
        "gemini" => matches!(
            request_model,
            "gemini-3.5-flash" | "gemini-3.1-pro-preview" | "gemini-3-pro-preview"
        ),
        "github-copilot" => matches!(
            request_model,
            "claude-opus-4.8"
                | "claude-sonnet-4.6"
                | "claude-haiku-4.5"
                | "claude-fable-5"
                | "gpt-5.5"
                | "gemini-3.1-pro-preview"
        ),
        "zai" => matches!(request_model, "glm-5.1" | "glm-5" | "glm-5-turbo"),
        // The profile name is "glm-coding" (catalog presets); "zhipuai-coding-plan"
        // is only the vendored slice's filename.
        "glm-coding" => matches!(request_model, "glm-5.1" | "glm-5-turbo" | "glm-4.7"),
        _ => false,
    }
}

/// Curate a flat list of model names for the no-arg/mobile listing surfaces
/// (`ClientEvent::ModelList`, `/model` text command) that carry only `Vec<String>`.
///
/// Mobile/CLI lack the TUI's per-provider availability maps, so this can't gate
/// `[Connect]`; instead it trims the catalog to the [`is_curated_model`] short
/// list (display names, de-duplicated, `current` kept first so it's always
/// selectable). When `listings` is empty (library/stub callers with no routing
/// client / catalog) it falls back to the raw `available` list unchanged —
/// there's nothing to curate against and the caller's prior behavior is kept.
#[must_use]
pub fn curated_model_names(
    listings: &[ModelListing],
    available: &[String],
    current: &str,
) -> Vec<String> {
    if listings.is_empty() {
        return available.to_vec();
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if !current.is_empty() {
        seen.insert(current.to_string());
        out.push(current.to_string());
    }
    for l in listings {
        if is_curated_model(&l.provider_id, &l.request_model)
            && seen.insert(l.display_model.clone())
        {
            out.push(l.display_model.clone());
        }
    }
    out
}

#[cfg(test)]
mod parse_model_ref_tests {
    use super::{parse_model_ref, ModelListing};
    fn listing(provider_id: &str, request_model: &str) -> ModelListing {
        ModelListing {
            display_model: request_model.to_string(),
            request_model: request_model.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            description: None,
        }
    }
    fn fixture() -> Vec<ModelListing> {
        vec![
            listing("openai", "gpt-5.2"),
            listing("openai", "gpt-4o"),
            listing("github-copilot", "gpt-5.2"),
            listing("openrouter", "openai/gpt-4o"),
        ]
    }
    #[test]
    fn bare_id_no_slash() {
        assert_eq!(
            parse_model_ref("gpt-5.2", &fixture()),
            ("gpt-5.2".into(), None)
        );
    }
    #[test]
    fn qualified_two_segments() {
        assert_eq!(
            parse_model_ref("openai/gpt-5.2", &fixture()),
            ("gpt-5.2".into(), Some("openai".into()))
        );
        assert_eq!(
            parse_model_ref("github-copilot/gpt-5.2", &fixture()),
            ("gpt-5.2".into(), Some("github-copilot".into()))
        );
    }
    #[test]
    fn two_segment_prefers_qualified_when_model_in_profile() {
        assert_eq!(
            parse_model_ref("openai/gpt-4o", &fixture()),
            ("gpt-4o".into(), Some("openai".into()))
        );
    }
    #[test]
    fn fully_qualified_openrouter_slash_id() {
        assert_eq!(
            parse_model_ref("openrouter/openai/gpt-4o", &fixture()),
            ("openai/gpt-4o".into(), Some("openrouter".into()))
        );
    }
    #[test]
    fn unknown_prefix_is_bare() {
        assert_eq!(
            parse_model_ref("foo/bar", &fixture()),
            ("foo/bar".into(), None)
        );
    }
    #[test]
    fn degenerate_inputs_safe() {
        assert_eq!(parse_model_ref("", &fixture()), ("".into(), None));
        assert_eq!(parse_model_ref("/", &fixture()), ("/".into(), None));
    }
}

#[cfg(test)]
mod curated_model_tests {
    use super::{curated_model_names, is_curated_model, ModelListing};

    fn listing(provider_id: &str, request_model: &str, display: &str) -> ModelListing {
        ModelListing {
            display_model: display.to_string(),
            request_model: request_model.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            description: None,
        }
    }

    #[test]
    fn glm_coding_keyed_on_profile_name_not_filename() {
        assert!(is_curated_model("glm-coding", "glm-5.1"));
        assert!(is_curated_model("glm-coding", "glm-4.7"));
        assert!(!is_curated_model("zhipuai-coding-plan", "glm-5.1"));
    }

    #[test]
    fn curates_catalog_to_short_list_and_keeps_current() {
        let listings = vec![
            listing("openai", "gpt-5.5", "GPT-5.5"),
            listing("openai", "gpt-4o", "GPT-4o"), // not curated → dropped
            listing("anthropic", "claude-opus-4-8", "claude-opus-4-8"),
            listing("gemini", "gemini-3.5-flash", "Gemini 3.5 Flash"),
        ];
        let available = vec!["GPT-5.5".to_string(), "GPT-4o".to_string()];
        let out = curated_model_names(&listings, &available, "claude-opus-4-8");
        // current first, then curated catalog; non-curated GPT-4o excluded.
        assert_eq!(out[0], "claude-opus-4-8", "current kept first");
        assert!(out.contains(&"GPT-5.5".to_string()));
        assert!(out.contains(&"Gemini 3.5 Flash".to_string()));
        assert!(!out.contains(&"GPT-4o".to_string()), "non-curated dropped");
        // current not duplicated even though it is also curated.
        assert_eq!(out.iter().filter(|m| *m == "claude-opus-4-8").count(), 1);
    }

    #[test]
    fn empty_listings_falls_back_to_raw_available() {
        let available = vec!["a".to_string(), "b".to_string()];
        assert_eq!(curated_model_names(&[], &available, "a"), available);
    }
}

/// Public handle to the orchestrator that slash commands operate against.
///
/// Wired in M5-09 (slash-command surface). M5-02 only defines the trait —
/// `ConversationOrchestrator` does NOT yet implement it.
#[async_trait]
pub trait OrchestratorHandle: Send + Sync {
    /// The session id currently driving the conversation.
    async fn current_session_id(&self) -> SessionId;

    /// Clear the in-memory session and start fresh.
    async fn clear_session(&self) -> Result<(), HandleError>;

    /// Force a compaction pass and return the summary.
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError>;

    /// Snapshot the cumulative cost.
    async fn snapshot_cost(&self) -> CostSnapshot;

    /// Switch the active model (and optional provider profile). Subsequent
    /// turns use the new model; the profile disambiguates shared model ids
    /// across providers (e.g. `"gpt-5.2"` on `"github-copilot"` vs `"openai"`).
    /// `None` profile = resolve unscoped (default / legacy behaviour).
    async fn switch_model(&self, model: &str, profile: Option<&str>) -> Result<(), HandleError>;

    // M5-10 additions:

    /// Set the orchestrator's internal `should_exit` flag.
    ///
    /// The REPL (M5-13) checks this after each turn and breaks out of the
    /// loop. The flag is one-way: once set, it cannot be cleared (so a
    /// double-`/exit` is idempotent).
    ///
    /// Wired by M5-10 (`/exit` handler).
    async fn request_exit(&self);

    /// Read the current value of the `should_exit` flag without mutating it.
    ///
    /// The REPL (M5-13) calls this after every slash-command dispatch to
    /// decide whether to break the loop. Returns `true` iff `request_exit`
    /// has been called at least once.
    async fn current_should_exit(&self) -> bool;

    /// Open `$EDITOR` on `<config-dir>/claude/LINGXI.md` (creating the file
    /// if it does not exist), block until the editor exits, then return the
    /// outcome.
    ///
    /// The editor lookup order is `EDITOR` → `VISUAL` → `"vi"` (Unix) /
    /// `"notepad.exe"` (Windows). Empty env values are treated as missing.
    ///
    /// Wired by M5-10 (`/memory` handler).
    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>;

    // M5-11 additions:

    /// Enumerate currently registered MCP servers + their connection state.
    /// Used by `/mcp` and `/status`. Returns an empty vector when no MCP
    /// servers are configured.
    async fn list_mcp_servers(&self) -> Vec<McpServerInfo>;

    /// Enumerate registered hooks (built-in + user). Used by `/hooks` and
    /// `/status`.
    async fn list_hooks(&self) -> Vec<HookInfo>;

    /// Enumerate registered subagents (markdown-defined + built-in). Used
    /// by `/agents` and `/status`.
    async fn list_agents(&self) -> Vec<AgentInfo>;

    /// Run the 6 doctor checks and return the aggregated report. Used by
    /// `/doctor`.
    async fn run_doctor_checks(&self) -> DoctorReport;

    /// Snapshot the full status panel. Used by `/status`.
    async fn get_status_snapshot(&self) -> StatusSnapshot;

    /// Open `$EDITOR` on `<config-dir>/claude/config.json` (creating if
    /// absent). Used by `/config`.
    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Open `$EDITOR` on `<config-dir>/claude/permissions.json` (creating
    /// if absent). Used by `/permissions`.
    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError>;

    /// Enumerate model names the orchestrator will accept via
    /// [`Self::switch_model`]. Used by `/model` (no-arg list mode).
    async fn list_available_models(&self) -> Vec<String>;

    /// Richer model listing for the grouped `/model` picker (display name +
    /// provider label + wire id), sourced from the llm-client catalog. The
    /// DEFAULT returns empty so existing impls/mocks compile unchanged; only the
    /// live `ConversationOrchestrator` overrides it.
    async fn list_model_listings(&self) -> Vec<ModelListing> {
        Vec::new()
    }

    /// Return the most recently observed provider rate-limit header snapshot.
    ///
    /// Returns `Some(snapshot)` when the underlying `ProviderApiAdapter` has
    /// received at least one successful 2xx response with
    /// `anthropic-ratelimit-unified-*` headers; `None` until then.
    ///
    /// The returned [`RateLimitSnapshot`] avoids leaking the
    /// orchestrator-internal `RateLimitInfo` struct through the `traits` crate
    /// (which must not depend on `orchestrator`).  It carries all three
    /// header-derived fields including `overage_disabled_reason`.
    ///
    /// **TUI wiring note:** the TUI's existing `rate_limit.rs` renderer is fed
    /// from `RenderedMessage::RateLimit` events that flow through the output
    /// stream (protocol layer).  Feeding this method's value into that path
    /// would require a new protocol event or a separate status-poll tick — both
    /// changes are outside the scope of this task (frozen protocol guard).
    /// This method is the handle-layer surface; callers that need live polling
    /// can call it from a ticker and push a `RenderedMessage::RateLimit` when
    /// the value changes.
    async fn last_rate_limit_info(&self) -> Option<RateLimitSnapshot> {
        None
    }

    // M6-03 addition:

    /// Streaming twin of the M5-13 cancel-aware turn entry point. The
    /// TUI calls this so Ctrl-C aborts the in-flight SSE stream cleanly.
    ///
    /// Default impl returns `Err(HandleError::Unimplemented(..))` so
    /// existing stdio REPL implementations (M5-13) need no override.
    /// `OrchestratorHandleImpl` (M6-03) overrides this to delegate to
    /// `ConversationOrchestrator::run_turn_streaming_with_cancel`.
    async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnOutcome, HandleError> {
        let _ = (prompt, cancel);
        Err(HandleError::Unimplemented(
            "run_turn_streaming_with_cancel".into(),
        ))
    }

    /// Streaming turn carrying pasted image file paths (TUI paste→image). Each
    /// path is read + base64-encoded into a `ContentBlock::Image` on the
    /// outgoing user message.
    ///
    /// Default delegates to [`Self::run_turn_streaming_with_cancel`] ignoring
    /// images, so non-TUI handle impls need no override.
    async fn run_turn_streaming_with_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnOutcome, HandleError> {
        let _ = image_paths;
        self.run_turn_streaming_with_cancel(prompt, cancel).await
    }

    // ────────────────────────────────────────────────────────────────────
    // engine-data-commands additions (`/export`, `/files`, `/context`,
    // `/resume`). Each carries a benign default so the production
    // `ConversationOrchestrator` (orchestrator/src/handle_impl.rs) and the
    // test `MockOrchestratorHandle` (orchestrator/src/test_support.rs) keep
    // compiling unchanged; the production impl overrides them with real data.
    // ────────────────────────────────────────────────────────────────────

    /// Clone of the live, ordered conversation history.
    ///
    /// Single read-only accessor backing `/export` (render the transcript to
    /// a file) and underpinning `/summary` and `/diff`. Returns the already
    /// public [`protocol::ConversationMessage`] so no new type is introduced.
    ///
    /// Default returns an empty `Vec`, so handle impls that do not track a
    /// session (e.g. the test mock) need no override.
    async fn conversation_transcript(&self) -> Vec<protocol::ConversationMessage> {
        Vec::new()
    }

    /// File paths currently tracked in the session's read-file-state cache.
    ///
    /// Backs `/files`, which renders each path relative to the cwd (cwd comes
    /// from [`Self::get_status_snapshot`]) and prints `"No files in context"`
    /// when empty — 1:1 with `files.ts`.
    ///
    /// Default returns an empty `Vec`, matching the TS "No files in context"
    /// branch when no read-file-state cache is wired.
    async fn files_in_context(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// `(used_tokens, max_tokens)` for the current context window.
    ///
    /// Minimal primitive-tuple accessor backing the `/context` flat panel
    /// (`**Tokens:** {used} / {max} ({pct}%)`). `used_tokens` comes from the
    /// session's cumulative usage; `max_tokens` from the active model's
    /// context budget. The model name itself already comes from
    /// [`Self::get_status_snapshot`], so no new struct is needed.
    ///
    /// Default returns `(0, 0)` when no usage is recorded.
    async fn context_window_usage(&self) -> (u64, u64) {
        (0, 0)
    }

    /// `Vec<(session_id, label)>` of prior on-disk sessions, newest-first.
    ///
    /// Single accessor for a non-interactive `/resume` listing (one
    /// `id` + `label` per line), built from `std` types only. The interactive
    /// picker and replaying a chosen session are handled elsewhere; this
    /// delivers only the enumeration half.
    ///
    /// Default returns an empty `Vec` when no on-disk store is present.
    async fn list_resumable_sessions(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Adopt a previously-loaded session IN PLACE: replace the live history with
    /// `history`, adopt the NAMED `session_id` (resume does NOT mint a fresh id —
    /// that is `clear_session`'s job), and seed the JSONL parent-uuid chain to
    /// `last_jsonl_uuid` so any future append chains via `parent_uuid` off the
    /// resumed tail. The model is unchanged — resume keeps the live model.
    ///
    /// The symmetric twin of [`Self::clear_session`]: where `clear_session`
    /// wipes + re-mints, `resume_session` adopts the on-disk session's id +
    /// transcript so the next turn continues with the prior context. The
    /// caller (engine host) has already loaded + validated the JSONL via the
    /// orchestrator's resume path; this method only swaps the loaded values
    /// into the running orchestrator.
    ///
    /// Default returns `Err(HandleError::Unimplemented(..))` so non-resuming
    /// handle impls (the stdio REPL path, the test `MockOrchestratorHandle`)
    /// keep compiling unchanged; the production `ConversationOrchestrator`
    /// overrides it.
    async fn resume_session(
        &self,
        session_id: SessionId,
        history: Vec<protocol::ConversationMessage>,
        last_jsonl_uuid: Option<String>,
    ) -> Result<(), HandleError> {
        let _ = (session_id, history, last_jsonl_uuid);
        Err(HandleError::Unimplemented("resume_session".into()))
    }
}

/// Captured output emission. Useful for tests and (M5-13) the stdio sink.
///
/// The enum is `non_exhaustive` so M5-04 can add a `StreamingDelta` variant
/// without a breaking change.
///
/// M5-11: `Eq` was dropped (and replaced with `PartialEq` only) because
/// `CostSnapshot` now carries `f64` + `Duration` fields whose `Eq` impl is
/// not defined. Callers that need set semantics should bucket by the
/// `session_id` or other integer fields instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Plain text from the assistant.
    Text {
        /// The text payload emitted.
        text: String,
    },
    /// An allowlisted terminal escape sequence to write to the terminal
    /// (#6 main-loop parity, [`OutputStream::emit_terminal_sequence`]).
    TerminalSequence {
        /// The validated OSC/BEL sequence.
        seq: String,
    },
    /// A tool invocation about to dispatch.
    ToolCall {
        /// Stable id (the `tool_use_id` echoed in the matching `ToolResult`).
        /// Added in M6-04 so the TUI can correlate calls with results and
        /// key the per-tool expanded-state map.
        id: protocol::ToolUseId,
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// A tool result returning to the conversation.
    ToolResult {
        /// Correlator with the matching `ToolCall`.
        id: protocol::ToolUseId,
        /// Name of the tool that returned.
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
    },
    /// End-of-turn marker with cost.
    EndTurn {
        /// Stop reason reported by the model (e.g. `"end_turn"`, `"max_tokens"`).
        stop_reason: String,
        /// Cumulative cost snapshot at end of turn.
        cost: CostSnapshot,
    },
    /// Emitted once a successful `force_compact` finishes. (M6-08)
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction (including the appended
        /// `[Compacted]` boundary marker).
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
    },
    /// A streaming reasoning ("thinking") delta as it arrives. (§0.7
    /// "light up thinking/usage"). Recorded by `MockOutputStream` so the
    /// orchestrator SSE-pump tests can assert `emit_thinking` fired. The
    /// `signature` is `None` for live deltas (it only arrives on the
    /// completed thinking block, not per-delta).
    Thinking {
        /// The reasoning fragment emitted.
        thinking: String,
        /// Cryptographic signature, `None` for live deltas.
        signature: Option<String>,
    },
    /// An incremental token-usage update for the latest API call. (§0.7
    /// "light up thinking/usage"). Recorded by `MockOutputStream` so the
    /// orchestrator SSE-pump tests can assert `emit_usage` fired with the
    /// right counts.
    Usage {
        /// Input tokens billed.
        input_tokens: u64,
        /// Output tokens billed.
        output_tokens: u64,
        /// Input tokens served from cache.
        cache_read_tokens: u64,
        /// Input tokens used to create a fresh cache entry.
        cache_creation_tokens: u64,
    },
    /// The latest unified rate-limit header snapshot, forwarded by the
    /// orchestrator after a completed API call ONLY when it differs from the
    /// previously emitted snapshot (emit-on-change). (llm-client future-work
    /// batch 3, Task 8.) Each field is parsed from an
    /// `anthropic-ratelimit-unified-*` response header (claude-code
    /// `claudeAiLimits.ts`); `None` means the provider did not send that
    /// header on the most recent 2xx response.
    RateLimit {
        /// `anthropic-ratelimit-unified-status` — `"allowed"` /
        /// `"allowed_warning"` / `"rejected"`.
        status: Option<String>,
        /// `anthropic-ratelimit-unified-representative-claim` — which window
        /// is representative (`"five_hour"` / `"seven_day"` /
        /// `"seven_day_opus"` / `"seven_day_sonnet"`).
        rate_limit_type: Option<String>,
        /// Per-claim `anthropic-ratelimit-unified-{abbrev}-utilization` —
        /// the representative claim's 0-1 utilization fraction (abbrev `5h`
        /// / `7d` / `overage`).
        utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-reset` — Unix-epoch seconds when the
        /// representative window resets.
        resets_at: Option<u64>,
        /// Per-claim `anthropic-ratelimit-unified-{abbrev}-reset` —
        /// Unix-epoch seconds when the representative claim's window resets.
        claim_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-status` — `"allowed"` /
        /// `"allowed_warning"` / `"rejected"`.
        overage_status: Option<String>,
        /// `anthropic-ratelimit-unified-overage-reset` — Unix-epoch seconds
        /// when the overage window resets.
        overage_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-disabled-reason` — why
        /// overage spend is disabled (e.g. `"out_of_credits"`).
        overage_disabled_reason: Option<String>,
        /// `anthropic-ratelimit-unified-fallback` strict-equals
        /// `"available"`; `None` when the header is absent.
        fallback_available: Option<bool>,
    },
    /// Raw per-window unified rate-limit utilization — claude-code
    /// `rawUtilization` (`claudeAiLimits.ts:145-179`), tracked on every API
    /// response (unlike the warning-gated [`Self::RateLimit`] fields) and
    /// consumed by the statusline command input (`StatusLine.tsx:50-65`). A
    /// window is `None` when the response lacked either of its two headers.
    /// (llm-client future-work batch 5, Task 1.)
    RawUtilization {
        /// `anthropic-ratelimit-unified-5h-utilization` (0-1 fraction).
        five_hour_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-5h-reset` (unix epoch seconds).
        five_hour_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-7d-utilization` (0-1 fraction).
        seven_day_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-7d-reset` (unix epoch seconds).
        seven_day_resets_at: Option<u64>,
    },
}

/// Severity / color of a context-pressure banner — mirrors the `<Text>` color
/// in claude-code's `TokenWarning.tsx:169` (`dimColor` for the auto-compact
/// countdown, `error` / `warning` for the "Context low" line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPressureLevel {
    /// `dimColor` — the auto-compact countdown branch.
    Dim,
    /// `color="warning"` — "Context low" below the error threshold.
    Warning,
    /// `color="error"` — "Context low" at/above the error threshold.
    Error,
}

/// A rendered context-pressure banner: the byte-exact label + its severity.
///
/// Computed by the orchestrator each turn (1:1 with claude-code's `TokenWarning`
/// component, via `compaction::token_warning_banner`) and pushed to the UI
/// through [`OutputStream::emit_context_pressure`]. `None` at that sink clears a
/// previously-shown banner once the context drops back below the warning
/// threshold (e.g. after a compaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPressureBanner {
    /// The byte-exact banner text (no surrounding chrome).
    pub text: String,
    /// The text color / severity.
    pub level: ContextPressureLevel,
}

/// Sink for orchestrator-emitted output events.
///
/// The stdio CLI (M5-12) and the future TUI (M6) both implement this.
/// M5-02 ships `MockOutputStream` (in `lingxi-orchestrator::test_support`)
/// for unit tests.
#[async_trait]
pub trait OutputStream: Send + Sync {
    /// Emit a piece of plain assistant text. In M5-02 this is called once
    /// per `Text` content block per turn (whole-body). M5-04 will switch
    /// to per-SSE-delta emission without changing this signature.
    async fn emit_text(&self, text: &str);

    /// Emit a tool-call notification immediately before dispatch.
    ///
    /// `id` is the `tool_use_id` echoed in the matching ToolResult. Added
    /// in M6-04 so consumers can correlate calls with results.
    async fn emit_tool_call(&self, id: &protocol::ToolUseId, tool: &str, input: &serde_json::Value);

    /// Emit a tool-result notification immediately after dispatch.
    ///
    /// `model_text` is the exact string the model saw for this tool result
    /// (the SDK frame's `tool_result.content`); `result` is the pure-metadata
    /// `data` payload (carried in `toolUseResult`).
    async fn emit_tool_result(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        model_text: &str,
        result: &serde_json::Value,
    );

    /// Emit the end-of-turn marker with the cost snapshot.
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);

    /// Emit a compaction-completed event. Default no-op for adapters
    /// that don't care (e.g. NDJSON sink may flush a one-line marker).
    /// (M6-08)
    async fn emit_compaction_completed(
        &self,
        _messages_before: u32,
        _messages_after: u32,
        _bytes_saved: u64,
    ) {
    }

    /// Emit a streaming reasoning ("thinking") delta as it arrives.
    ///
    /// Added by the §0.7 "light up thinking/usage" follow-up. Called once
    /// per `ThinkingDelta` SSE chunk from `event_router`. `signature` is
    /// `None` for live deltas — the cryptographic signature only arrives on
    /// the completed thinking block (`SignatureDelta`), not per-delta, so
    /// the live-delta path always passes `None`.
    ///
    /// **Default no-op**: pre-existing sinks (TUI, CLI, `MockOutputStream`)
    /// that don't render reasoning keep compiling unchanged. The
    /// client-adapter overrides this to surface a `ClientEvent::ThinkingDelta`.
    async fn emit_thinking(&self, _thinking: &str, _signature: Option<&str>) {}

    /// Emit an incremental token-usage update for the latest API call.
    ///
    /// Added by the §0.7 "light up thinking/usage" follow-up. Called from
    /// `event_router` when a `MessageDelta`/`MessageStart` SSE event carries
    /// a `usage` payload. Counts are passed as bare `u64`s (rather than a
    /// `cost::TokenUsage`) to keep `lingxi-traits` a leaf crate: `lingxi-cost`
    /// already depends on `lingxi-traits`, so a `cost` dependency here would
    /// form a cycle. The four arguments map field-for-field onto both
    /// `cost::TokenUsage` (caller side, in the orchestrator) and
    /// `ClientEvent::UsageUpdate` (adapter side).
    ///
    /// **Default no-op**: pre-existing sinks keep compiling unchanged. The
    /// client-adapter overrides this to surface a `ClientEvent::UsageUpdate`.
    async fn emit_usage(
        &self,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_read_tokens: u64,
        _cache_creation_tokens: u64,
    ) {
    }

    /// Emit a coordinator team-status update (active worker count + team name).
    ///
    /// Added by the coordinator-activation program. Called by the
    /// `CoordinatorStatusSink` after each teammate status transition that
    /// changes the active-worker tally, so a coordinator-mode session can
    /// surface a live roster count.
    ///
    /// **Default no-op**: pre-existing sinks (TUI, CLI, `MockOutputStream`)
    /// keep compiling unchanged. The client-adapter overrides this to surface
    /// a `ClientEvent::CoordinatorStatus`.
    async fn emit_coordinator_status(&self, _active_workers: u32, _team: Option<&str>) {}

    /// Emit the latest unified rate-limit header snapshot.
    ///
    /// Added by llm-client future-work batch 3 (Task 8). Called by the
    /// orchestrator turn drivers after each completed API call whose
    /// rate-limit snapshot DIFFERS from the previously emitted one — the
    /// orchestrator dedupes, so sinks only ever see changes. The nine
    /// arguments map field-for-field onto [`OutputEvent::RateLimit`]; see
    /// that variant's per-field docs for the `anthropic-ratelimit-unified-*`
    /// header each value is parsed from (claude-code `claudeAiLimits.ts`).
    ///
    /// **Default no-op**: pre-existing sinks (TUI, CLI, `MockOutputStream`)
    /// keep compiling unchanged. The TUI bridge overrides this to surface
    /// the rate-limit status message.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors OutputEvent::RateLimit's nine header-derived fields; the bare-argument shape matches the emit_usage convention on this trait"
    )]
    async fn emit_rate_limit(
        &self,
        _status: Option<&str>,
        _rate_limit_type: Option<&str>,
        _utilization: Option<f64>,
        _resets_at: Option<u64>,
        _claim_resets_at: Option<u64>,
        _overage_status: Option<&str>,
        _overage_resets_at: Option<u64>,
        _overage_disabled_reason: Option<&str>,
        _fallback_available: Option<bool>,
    ) {
    }

    /// Push the current context-pressure banner, or `None` to clear it.
    ///
    /// Called by the turn driver before every API call with the result of
    /// `compaction::token_warning_banner` for the current context estimate — the
    /// orchestrator-side twin of claude-code's `<TokenWarning>` render
    /// (`PromptInput/Notifications.tsx:321`), which recomputes
    /// `calculateTokenWarningState` as `tokenUsage` grows. Default no-op so
    /// non-interactive sinks (print mode, tests) ignore it.
    /// `used_fraction` is the current context usage as a 0-1 fraction of the
    /// model's effective context window (claude-code
    /// `calculateContextPercentages(currentUsage, contextWindowSize).used`),
    /// carried alongside the (optional) banner so a consumer that wants the raw
    /// percentage — the custom statusline's `context_window.used_percentage` —
    /// gets it on every turn, not only when the warning banner is showing.
    async fn emit_context_pressure(
        &self,
        _banner: Option<ContextPressureBanner>,
        _used_fraction: f32,
    ) {
    }

    /// Push a raw-utilization snapshot.
    ///
    /// Added by llm-client future-work batch 5 (Task 1). Called by the
    /// orchestrator turn drivers after every completed API call — unlike the
    /// deduped, warning-gated [`Self::emit_rate_limit`], this mirrors
    /// claude-code's `rawUtilization` tracking (`claudeAiLimits.ts:145-179`),
    /// which records the per-window headers on every response for the
    /// statusline command input (`StatusLine.tsx:50-65`). The four arguments
    /// map field-for-field onto [`OutputEvent::RawUtilization`]; see that
    /// variant's per-field docs for the `anthropic-ratelimit-unified-*`
    /// header each value is parsed from.
    ///
    /// **Default no-op**: hosts without a statusline ignore it, and every
    /// pre-existing sink (TUI, CLI, `MockOutputStream`) keeps compiling
    /// unchanged.
    async fn emit_raw_utilization(
        &self,
        _five_hour_utilization: Option<f64>,
        _five_hour_resets_at: Option<u64>,
        _seven_day_utilization: Option<f64>,
        _seven_day_resets_at: Option<u64>,
    ) {
    }

    /// Write an allowlisted terminal escape sequence to the active terminal.
    ///
    /// The orchestrator calls this (#6 main-loop parity) after a hook returns a
    /// `terminalSequence` that PASSES the OSC/BEL allowlist validator
    /// (claude-code `szn`→`BEo`, which writes the validated sequence to the
    /// controlling terminal). The orchestrator process holds no TTY — the TUI
    /// owns the terminal — so it forwards the validated bytes through this seam;
    /// the interactive host (TUI) writes them to its stdout. `seq` is the
    /// already-validated, BEL-normalized string.
    ///
    /// **Default no-op**: non-interactive hosts (print mode, tests, CLI sink)
    /// and any host without a controlling terminal ignore it, and every
    /// pre-existing sink keeps compiling unchanged.
    async fn emit_terminal_sequence(&self, _seq: &str) {}

    /// Signal that an Anthropic API message has begun streaming (i.e. on
    /// `message_start`). Passes the API-assigned message id and model so sinks
    /// that accumulate per-message blocks can record them before any deltas arrive.
    ///
    /// **Default no-op**: every pre-existing `OutputStream` impl keeps compiling
    /// unchanged.
    ///
    /// # Arguments
    /// * `message_id` — the provider-assigned message id (e.g. `msg_01…`).
    /// * `model` — the model id that produced this message.
    async fn emit_message_start(&self, _message_id: &str, _model: &str) {}

    /// Signal that one Anthropic API message has completed streaming.
    ///
    /// Called by the streaming loop immediately after [`RouterAction::EndOfStream`]
    /// (i.e. when the `message_stop` SSE event arrives). Consumers that need to
    /// batch text+thinking+tool_use blocks into a single `assistant` frame
    /// (stream-json output) flush their accumulator here.
    ///
    /// **Default no-op**: every pre-existing `OutputStream` impl (TUI,
    /// CLI `SinkAdapter`, `MockOutputStream`) keeps compiling unchanged.
    ///
    /// # Arguments
    /// * `stop_reason` — the `stop_reason` from the final `message_delta` (e.g.
    ///   `"end_turn"`), or `None` if the delta carried no stop reason.
    /// * `request_id` — the HTTP `request-id` header from the API response, when
    ///   available (used by stream-json's `assistant.request_id` field).
    async fn emit_message_boundary(&self, _stop_reason: Option<&str>, _request_id: Option<&str>) {}

    /// Emit a raw SSE stream event frame (`stream_event`) for
    /// `--include-partial-messages`.
    ///
    /// `event_json` is a JSON string reconstructed from the parsed `LlmEvent`
    /// (semantically equivalent to the original Anthropic SSE event, but NOT
    /// byte-for-byte identical since LingXi already parsed the raw bytes).
    /// `is_message_start` is `true` only for the `message_start` event, which
    /// is the only event that carries `ttft_ms` in the GROUND-TRUTH output.
    ///
    /// **Default no-op**: every pre-existing `OutputStream` impl keeps
    /// compiling unchanged. Only `StreamJsonStream` overrides this.
    async fn emit_stream_event(&self, _event_json: &str, _is_message_start: bool) {}

    /// Emit a `system/hook_started` frame for `--include-hook-events`.
    ///
    /// Called before each hook dispatches. `hook_id` and `hook_name` identify
    /// the hook; `hook_event` is the `Debug` name of the `HookEventType`
    /// (e.g. `"SessionStart"`, `"PreToolUse"`).
    ///
    /// **Default no-op**: every pre-existing `OutputStream` impl keeps
    /// compiling unchanged. Only `StreamJsonStream` overrides this.
    async fn emit_hook_started(&self, _hook_id: &str, _hook_name: &str, _hook_event: &str) {}

    /// Emit a `system/hook_response` frame for `--include-hook-events`.
    ///
    /// Called after each hook completes. Parameters carry the hook identity
    /// fields (same as [`Self::emit_hook_started`]) plus the full result:
    /// `output` is the combined hook response text (stdout + any
    /// `systemMessage`); `exit_code` is `None` for non-command hooks;
    /// `outcome` is `"success"`, `"error"`, `"block"`, or `"timeout"`.
    ///
    /// **Default no-op**: every pre-existing `OutputStream` impl keeps
    /// compiling unchanged. Only `StreamJsonStream` overrides this.
    #[allow(clippy::too_many_arguments)]
    async fn emit_hook_response(
        &self,
        _hook_id: &str,
        _hook_name: &str,
        _hook_event: &str,
        _output: &str,
        _stdout: &str,
        _stderr: &str,
        _exit_code: Option<i32>,
        _outcome: &str,
    ) {
    }
}

#[cfg(test)]
#[path = "orchestrator_test.rs"]
mod orchestrator_test;
