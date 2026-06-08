//! Orchestrator runtime configuration.
//!
//! `MAX_TURNS_DEFAULT = 30` is the LingXi-locked default. Spec §7 OQ-1:
//! claude-code has no global `maxTurns` default (only per-agent
//! frontmatter), so we lock 30 as the main-loop ceiling. Override at
//! construction via `OrchestratorConfig { max_turns, .. }`.

use serde::{Deserialize, Serialize};

/// Default value for [`OrchestratorConfig::max_turns`]. **Locked at 30**
/// per spec §4.2 OQ-1 resolution (2026-05-25).
pub const MAX_TURNS_DEFAULT: u32 = 30;

/// Default model identifier. The actual model lives in user settings or
/// CLI flags (M3-01 + M5-12); this value is only used when the embedder
/// constructs an orchestrator with `OrchestratorConfig::default()` for
/// tests.
pub const DEFAULT_MODEL: &str = "claude-opus-4-7";

/// Runtime configuration for [`crate::ConversationOrchestrator`].
//
// Independent feature/auth flags — claude-code carries these as separate
// booleans too (interactive vs print, the otk/token-budget gates, the resolved
// subscription flags); collapsing them into a state enum would obscure the 1:1
// mapping, so we suppress `struct_excessive_bools` (as `anthropic-oauth`'s
// resolver does for the same reason).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Maximum number of turns before the loop aborts with
    /// [`crate::OrchestratorError::MaxTurnsReached`]. Default
    /// [`MAX_TURNS_DEFAULT`].
    pub max_turns: u32,

    /// Active model identifier (passed verbatim to
    /// `AnthropicProvider::messages_create_non_stream`).
    pub model: String,

    /// Opus-fallback model (claude-code `--fallback-model`). When `Some(id)`,
    /// the batched turn loop routes its primary API call through the
    /// fallback-aware api-client path so a consecutive-529 gate on a non-custom
    /// Opus primary model can surface [`api_client::ApiError::FallbackTriggered`];
    /// the turn loop then switches the session model to `id`, warns the user,
    /// and re-issues against it (1:1 with claude-code `query.ts:894-948`).
    ///
    /// `None` (the parity default) is a STRICT no-op: the primary call keeps
    /// using the plain `messages_create` seam (fallback disabled), so the locked
    /// turn-loop fixtures are byte-unaffected. claude-code restricts
    /// `--fallback-model` to `--print`/non-interactive mode; the CLI mirrors that
    /// guard (M5-12 `argv.rs`).
    #[serde(default)]
    pub fallback_model: Option<String>,

    /// Optional system prompt override. `None` means the default
    /// claude-code-equivalent system prompt is assembled (M5-03 wires
    /// the dynamic assembly; M5-02 leaves this `None` and the API call
    /// sends NO system prompt — the model receives only `messages`).
    pub system_prompt_override: Option<String>,

    /// When `true`, M5-12 CLI binary wires
    /// [`permission::InteractivePromptingGate`] over real stdin /
    /// stderr; when `false` (default), it wires
    /// [`crate::test_support::NoOpPermissionGate`]. The orchestrator
    /// itself doesn't read this flag — the `perms: Arc<dyn PermissionGate>`
    /// constructor argument decides; this field is the CLI's source of
    /// truth for which gate to construct. (M5-05)
    #[serde(default)]
    pub interactive_permissions: bool,

    /// If `Some(id)`, the orchestrator was started via `--resume <id>` or
    /// `/resume <id>` (M5-08) and must replay messages from the on-disk
    /// JSONL before running the first turn. The same `id` is re-used for
    /// new appends so the chain continues uninterrupted. `None` = fresh
    /// session.
    #[serde(default)]
    pub resume_session_id: Option<uuid::Uuid>,

    /// A1: gate for the 8k→64k output-token escalation retry (TS gate
    /// `tengu_otk_slot_v1`, 3P default `false` — not validated on
    /// Bedrock/Vertex). When `false` (the parity default), the loop performs
    /// ONLY the multi-turn "resume directly" nudge recovery on a `max_tokens`
    /// `stop_reason`; the single-shot escalation to
    /// [`crate::turn_loop::ESCALATED_MAX_TOKENS`] is skipped.
    ///
    /// DEFERRED: even when `true`, the escalation is currently a no-op because
    /// the api-client `messages_create` signature has no `max_tokens` override
    /// argument (see A1 spec — escalation needs an api-client change that is
    /// out of scope for this crate-local batch). The flag + const are wired so
    /// a follow-up can complete the escalation without a config migration.
    #[serde(default)]
    pub escalate_max_output_tokens: bool,

    /// A3: feature gate for token-budget auto-continuation (TS `feature('TOKEN_BUDGET')`).
    /// Defaults `false` for parity — TS gates the whole `+500k` continuation
    /// block. When `false` (or [`Self::token_budget`] is `None`), the turn
    /// loop's end-of-turn budget check is a NO-OP and the loop stops at the
    /// first `end_turn`, exactly as before. Set BOTH this flag `true` AND
    /// [`Self::token_budget`] to `Some(n)` to enable the continuation behaviour.
    #[serde(default)]
    pub enable_token_budget: bool,

    /// A3: the active per-turn output-token budget (e.g. `Some(500_000)` for
    /// `"+500k"`). `None` (the parity default) disables continuation. Callers
    /// may populate this from
    /// [`crate::token_budget::parse_token_budget`] applied to the user prompt,
    /// or set it explicitly. Only honoured when [`Self::enable_token_budget`]
    /// is also `true`.
    #[serde(default)]
    pub token_budget: Option<u64>,

    /// Pre-computed `isClaudeAISubscriber()` (`auth.ts:1564-1571`): `true` when
    /// the active session authenticates via a Claude.ai OAuth token carrying the
    /// `user:inference` scope (and Anthropic auth is enabled — no overriding env
    /// API key). Threaded into the fallback-aware api-client seam
    /// ([`crate::OrchestratorApiClient::messages_create_with_fallback`]) so the
    /// consecutive-529 Opus-fallback gate
    /// (`allow_fallback = fallback_for_all || (!is_subscriber && is_non_custom_opus)`)
    /// and the 429-retry gate (`retry_429_allowed = !is_subscriber || is_enterprise`)
    /// resolve to the same branch claude-code takes.
    ///
    /// `false` (the parity default) is byte-identical to the pre-wiring stub: the
    /// turn loop passed `is_subscriber = false` until OAuth subscription
    /// resolution landed. Populated at the composition root
    /// (`engine_desktop::build`) via `anthropic_oauth::subscription_from_scopes`.
    #[serde(default)]
    pub is_subscriber: bool,

    /// Pre-computed `isEnterpriseSubscriber()` (`auth.ts:1694`): `true` when the
    /// resolved subscription tier is Enterprise. Only consulted when
    /// [`Self::is_subscriber`] is `true`, where it re-enables the 429 retry that
    /// `is_subscriber` would otherwise suppress (`!is_subscriber || is_enterprise`).
    ///
    /// `false` (the parity default) is conservative. PARITY-GAP: enterprise tier
    /// requires the subscription type, which comes from a profile fetch
    /// (`anthropic_oauth::fetch_profile_from_oauth_token` + `apply_profile`) not
    /// performed in the desktop build hot path; `engine_desktop::build` leaves
    /// this `false` pending that fetch.
    #[serde(default)]
    pub is_enterprise: bool,

    /// OUTSTYLE.2: the active output-style name from `settings.outputStyle`
    /// (TS types it `z.string()`). `None` / `"default"` → no style section;
    /// a builtin name (`Explanatory` / `Learning`) injects its
    /// `# Output Style: <name>` section into the assembled system prompt via
    /// [`outputstyles::resolve_builtin_output_style`]. Populated at the
    /// composition root from merged settings.
    #[serde(default)]
    pub output_style: Option<String>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_turns: MAX_TURNS_DEFAULT,
            model: DEFAULT_MODEL.to_string(),
            fallback_model: None,
            system_prompt_override: None,
            interactive_permissions: false,
            resume_session_id: None,
            escalate_max_output_tokens: false,
            enable_token_budget: false,
            token_budget: None,
            is_subscriber: false,
            is_enterprise: false,
            output_style: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_turns_is_30() {
        assert_eq!(OrchestratorConfig::default().max_turns, 30);
        assert_eq!(MAX_TURNS_DEFAULT, 30);
    }

    #[test]
    fn default_model_is_locked_string() {
        assert_eq!(OrchestratorConfig::default().model, "claude-opus-4-7");
    }

    #[test]
    fn default_system_prompt_override_is_none() {
        assert!(OrchestratorConfig::default()
            .system_prompt_override
            .is_none());
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = OrchestratorConfig {
            max_turns: 5,
            model: "x".into(),
            fallback_model: Some("claude-sonnet-4-6".into()),
            system_prompt_override: Some("custom".into()),
            interactive_permissions: true,
            resume_session_id: None,
            escalate_max_output_tokens: true,
            enable_token_budget: true,
            token_budget: Some(500_000),
            is_subscriber: true,
            is_enterprise: true,
            output_style: Some("Explanatory".into()),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: OrchestratorConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.max_turns, 5);
        assert_eq!(back.model, "x");
        assert_eq!(back.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(back.system_prompt_override.as_deref(), Some("custom"));
        assert!(back.interactive_permissions);
        assert!(back.resume_session_id.is_none());
        assert!(back.escalate_max_output_tokens);
        assert!(back.enable_token_budget);
        assert_eq!(back.token_budget, Some(500_000));
        assert!(back.is_subscriber);
        assert!(back.is_enterprise);
        assert_eq!(back.output_style.as_deref(), Some("Explanatory"));
    }

    #[test]
    fn default_fallback_model_is_none() {
        assert!(OrchestratorConfig::default().fallback_model.is_none());
    }

    #[test]
    fn default_subscription_flags_are_false() {
        // The parity default: no subscription resolved → byte-identical to the
        // pre-wiring `is_subscriber = false` / `is_enterprise = false` stub.
        let cfg = OrchestratorConfig::default();
        assert!(!cfg.is_subscriber);
        assert!(!cfg.is_enterprise);
    }

    #[test]
    fn subscription_flags_default_when_absent_from_json() {
        // `#[serde(default)]` — a pre-subscription persisted config must
        // deserialize with both flags `false`.
        let s = r#"{"max_turns":7,"model":"m"}"#;
        let back: OrchestratorConfig = serde_json::from_str(s).unwrap();
        assert!(!back.is_subscriber);
        assert!(!back.is_enterprise);
    }

    #[test]
    fn fallback_model_defaults_when_absent_from_json() {
        // `#[serde(default)]` — a config JSON without `fallback_model` must
        // deserialize with `None` (back-compat with pre-fallback persisted cfgs).
        let s = r#"{"max_turns":7,"model":"m"}"#;
        let back: OrchestratorConfig = serde_json::from_str(s).unwrap();
        assert!(back.fallback_model.is_none());
        assert_eq!(back.model, "m");
    }

    #[test]
    fn default_token_budget_disabled() {
        let cfg = OrchestratorConfig::default();
        assert!(!cfg.enable_token_budget);
        assert!(cfg.token_budget.is_none());
    }

    #[test]
    fn default_escalate_max_output_tokens_is_false() {
        assert!(!OrchestratorConfig::default().escalate_max_output_tokens);
    }

    #[test]
    fn default_interactive_permissions_is_false() {
        assert!(!OrchestratorConfig::default().interactive_permissions);
    }

    #[test]
    fn default_resume_session_id_is_none() {
        assert!(OrchestratorConfig::default().resume_session_id.is_none());
    }

    #[test]
    fn resume_session_id_round_trips_through_json() {
        let sid = uuid::Uuid::from_bytes([7u8; 16]);
        let cfg = OrchestratorConfig {
            resume_session_id: Some(sid),
            ..OrchestratorConfig::default()
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: OrchestratorConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.resume_session_id, Some(sid));
    }
}
