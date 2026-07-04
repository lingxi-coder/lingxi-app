//! Orchestrator → TUI event bridge. (M6-03)
//!
//! The `BridgeOutputStream` is an [`traits::OutputStream`] impl that
//! forwards every orchestrator callback as a [`TurnEvent`] on an mpsc
//! channel. The TUI render loop drains the receiver and feeds events into
//! `crate::streaming::apply_event`, which mutates `AppState` and pokes a
//! `tokio::sync::Notify`.
//!
//! The lifecycle:
//! 1. TUI app creates an `mpsc::unbounded_channel::<TurnEvent>()`.
//! 2. TUI wraps the sender in `BridgeOutputStream` and passes it to the
//!    `ConversationOrchestrator` as its `output: Arc<dyn OutputStream>`.
//! 3. TUI keeps the receiver and the sender (so the channel stays open
//!    across multiple turns).
//! 4. For each user submit: TUI emits `TurnEvent::TurnStarted` directly,
//!    spawns `run_turn_streaming_with_cancel`, then awaits delta events.
//! 5. On `emit_end_turn` the bridge fires `TurnEvent::TurnEnded(_)`.

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use traits::{ContextPressureBanner, CostSnapshot, OutputStream, TurnOutcome};

/// Events flowing from the orchestrator into the TUI render loop.
///
/// Created in M6-03 as a TUI-local enum (not exposed on any orchestrator
/// trait). The bridge translates `OutputStream` callbacks into this enum.
/// `PermissionRequest` is wired in M6-05; `ThinkingDelta` in M5 (2.1.198
/// live-streaming parity).
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Streaming text chunk from the assistant.
    TextDelta(String),
    /// A completed assistant thinking block (M5 live streaming). The
    /// orchestrator's `emit_thinking` fires once per COMPLETED block (not
    /// per-delta — see `traits::OutputStream::emit_thinking`), so one event
    /// carries the whole reasoning text.
    ThinkingDelta(String),
    /// A tool invocation is about to dispatch.
    ToolUseStart {
        /// Stable id (the model-supplied `tool_use_id`) — correlates with
        /// the matching `ToolUseResult`.
        id: protocol::ToolUseId,
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// A tool result has returned.
    ToolUseResult {
        /// Correlator with the paired `ToolUseStart`.
        id: protocol::ToolUseId,
        /// Tool name (used to gate Bash → ANSI parser at render time).
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
    },
    /// Permission gate fired. Wired in M6-05; the variant is reserved
    /// here so the enum stays append-only.
    PermissionRequest {
        /// Name of the tool the permission gate is checking.
        tool: String,
        /// JSON input the permission gate is being asked to approve.
        input: serde_json::Value,
    },
    /// Fired SYNCHRONOUSLY before the orchestrator future is awaited so
    /// the UI shows the spinner immediately on Enter.
    TurnStarted,
    /// Fired when the orchestrator returns. Carries the [`TurnOutcome`].
    TurnEnded(TurnOutcome),
    /// Updated session-cumulative cost, formatted as `$0.0000` (4-decimal
    /// claude-code parity). M6-06: fired by [`BridgeOutputStream::emit_end_turn`]
    /// using the `CostSnapshot` the orchestrator now populates. The TUI
    /// `apply_event` writes the value into `state.status.cost`, refreshing
    /// the status-line render.
    CostUpdated(String),
    /// The live context-pressure banner (or `None` to clear it). `apply_event`
    /// stores it on `state.context_pressure`; the prompt chrome renders it as a
    /// `<TokenWarning>`-equivalent line. Emitted by the orchestrator before
    /// every API call with the current `compaction::token_warning_banner`.
    ContextPressure {
        /// `Some` shows the banner; `None` clears a previously-shown one.
        banner: Option<ContextPressureBanner>,
        /// Context usage as a 0-1 fraction of the model's effective context
        /// window (fed to the custom statusline's `context_window.used_percentage`).
        used_fraction: f32,
    },
    /// An allowlisted terminal escape sequence a hook returned (#6 main-loop
    /// parity). `apply_event` stages it on `state.pending_terminal_sequence`;
    /// the async pump writes the bytes directly to the TUI's stdout (the host
    /// that actually owns the controlling terminal — claude-code `BEo`). Already
    /// validated + BEL-normalized by the orchestrator.
    TerminalSequence {
        /// The validated OSC/BEL sequence to write to the terminal.
        seq: String,
    },
    /// A successful `force_compact` finished. The TUI appends a
    /// `CompactBoundary` variant, rendered by `CompactBoundaryMessage`
    /// (M7-04) as `✻ Conversation compacted (ctrl+o for history)`. (M6-08
    /// emitted a `[Compacted N → M messages]` `SystemText` placeholder;
    /// M7-04 replaced it.)
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction.
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
    },
    /// Unified rate-limit header snapshot (llm-client future-work batch 3,
    /// Task 9). Mirrors `traits::OutputEvent::RateLimit`'s nine fields —
    /// see that variant's per-field docs for the
    /// `anthropic-ratelimit-unified-*` header each value comes from. The
    /// orchestrator emits on-change only; `apply_event` additionally dedupes
    /// on the COMPOSED text so identical consecutive notices never stack.
    RateLimit {
        /// `anthropic-ratelimit-unified-status`.
        status: Option<String>,
        /// `anthropic-ratelimit-unified-representative-claim`.
        rate_limit_type: Option<String>,
        /// Representative claim's 0-1 utilization fraction.
        utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-reset` (Unix-epoch seconds).
        resets_at: Option<u64>,
        /// Per-claim reset (Unix-epoch seconds).
        claim_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-status`.
        overage_status: Option<String>,
        /// `anthropic-ratelimit-unified-overage-reset` (Unix-epoch seconds).
        overage_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-disabled-reason`.
        overage_disabled_reason: Option<String>,
        /// `anthropic-ratelimit-unified-fallback` == `available`.
        fallback_available: Option<bool>,
    },
    /// Raw per-window utilization snapshot (llm-client future-work batch 5,
    /// Task 4). Mirrors `traits::OutputEvent::RawUtilization`'s four fields —
    /// tracked on every API response (unlike the warning-gated
    /// [`Self::RateLimit`]) and stored on `AppState.raw_utilization` for the
    /// statusline command input's `rate_limits` field (`StatusLine.tsx:50-65`).
    /// Windows are atomic: a window's two fields are both `Some` or both `None`.
    RawUtilization {
        /// `anthropic-ratelimit-unified-5h-utilization` (0-1 fraction).
        five_hour_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-5h-reset` (Unix-epoch seconds).
        five_hour_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-7d-utilization` (0-1 fraction).
        seven_day_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-7d-reset` (Unix-epoch seconds).
        seven_day_resets_at: Option<u64>,
    },
    /// A one-off system notice for the transcript, fired by an app-level
    /// async effect that isn't itself a turn (e.g. the `/web` picker's
    /// secret/settings save or test-search result). NOT emitted by
    /// [`BridgeOutputStream`] — the embedding CLI sends it directly on the
    /// same `TurnEvent` channel so the result lands in the transcript on the
    /// next render tick.
    SystemNotice {
        /// The notice text.
        body: String,
        /// `true` → render as an error (red); `false` → dim informational.
        is_error: bool,
    },
    /// Captured output of a `!`-prefixed bash-mode command (run off the model
    /// path). Rendered as a `UserBashOutput` cell — ANSI-parsed stdout then
    /// error-tinted stderr. Sent by the CLI `on_bash` closure after the
    /// sandboxed [`crate::bash_runner::BashRunner`] returns.
    BashOutput {
        /// Captured stdout.
        stdout: String,
        /// Captured stderr.
        stderr: String,
    },
    /// A provider gained a usable credential mid-session (a successful
    /// `/connect` StoreKey / OAuth / Copilot login). The CLI's connect closure
    /// sends this so the widget flips its live availability map — which gates
    /// the `/model` picker (and badges the `/connect` picker) — WITHOUT a
    /// restart. NOT emitted by [`BridgeOutputStream`]; sent directly like
    /// [`Self::SystemNotice`]. Keyed by `profile_name` (e.g. `"openrouter"`),
    /// the same key space as the launch availability map.
    ProviderConnected {
        /// The provider/profile that just became available.
        provider_id: String,
    },
}

/// `OutputStream` impl that forwards every callback as a `TurnEvent` on
/// an mpsc channel. Cloneable via `tx.clone()` if multiple producers are
/// ever needed (currently one bridge per session — the channel lives for
/// the whole TUI lifetime).
pub struct BridgeOutputStream {
    tx: UnboundedSender<TurnEvent>,
}

impl BridgeOutputStream {
    /// Wrap a sender. The receiver lives on the TUI side and is drained
    /// by the render loop.
    #[must_use]
    pub fn new(tx: UnboundedSender<TurnEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl OutputStream for BridgeOutputStream {
    async fn emit_text(&self, text: &str) {
        let _ = self.tx.send(TurnEvent::TextDelta(text.to_string()));
    }

    async fn emit_thinking(&self, thinking: &str, _signature: Option<&str>) {
        let _ = self.tx.send(TurnEvent::ThinkingDelta(thinking.to_string()));
    }

    async fn emit_tool_call(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        input: &serde_json::Value,
    ) {
        let _ = self.tx.send(TurnEvent::ToolUseStart {
            id: id.clone(),
            tool: tool.to_string(),
            input: input.clone(),
        });
    }

    async fn emit_tool_result(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        model_text: &str,
        result: &serde_json::Value,
    ) {
        // (gap-3 general) Many tools return a structured `data` object with no
        // human-display string (Read's `{type,file:{…}}`, …), so the scrollback
        // renderer pretty-printed the raw JSON. `model_text` is the human/model
        // -facing body the tool already produced ("the render rides on
        // model_content" — read.rs). Carry it into the result object as
        // `model_content` so `body_text` can surface it; the structured fields
        // stay intact for tools that render off them (Bash stdout/stderr,
        // Edit/Write diffs).
        let result = match result.as_object() {
            Some(obj) if !model_text.is_empty() && !obj.contains_key("model_content") => {
                let mut obj = obj.clone();
                obj.insert(
                    "model_content".to_string(),
                    serde_json::Value::String(model_text.to_string()),
                );
                serde_json::Value::Object(obj)
            }
            _ => result.clone(),
        };
        let _ = self.tx.send(TurnEvent::ToolUseResult {
            id: id.clone(),
            tool: tool.to_string(),
            result,
        });
    }

    async fn emit_compaction_completed(
        &self,
        messages_before: u32,
        messages_after: u32,
        bytes_saved: u64,
    ) {
        let _ = self.tx.send(TurnEvent::CompactionCompleted {
            messages_before,
            messages_after,
            bytes_saved,
        });
    }

    async fn emit_context_pressure(
        &self,
        banner: Option<ContextPressureBanner>,
        used_fraction: f32,
    ) {
        let _ = self.tx.send(TurnEvent::ContextPressure {
            banner,
            used_fraction,
        });
    }

    async fn emit_terminal_sequence(&self, seq: &str) {
        let _ = self.tx.send(TurnEvent::TerminalSequence {
            seq: seq.to_string(),
        });
    }

    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        // M6-06: emit a CostUpdated event before TurnEnded so the
        // status-line refreshes to the post-turn cost in the next
        // render pass. Format follows claude-code's `toFixed(4)` parity.
        let cost_str = format!("${:.4}", cost.total_usd);
        let _ = self.tx.send(TurnEvent::CostUpdated(cost_str));

        // Map stop_reason → TurnOutcome. Mirrors the M5-13 stdio REPL
        // mapping. Unknown/unrecognised reasons fall back to EndTurn.
        let outcome = match stop_reason {
            "max_tokens" => TurnOutcome::MaxTurns,
            _ => TurnOutcome::EndTurn,
        };
        let _ = self.tx.send(TurnEvent::TurnEnded(outcome));
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the trait method's nine header-derived fields (see traits::OutputStream::emit_rate_limit)"
    )]
    async fn emit_rate_limit(
        &self,
        status: Option<&str>,
        rate_limit_type: Option<&str>,
        utilization: Option<f64>,
        resets_at: Option<u64>,
        claim_resets_at: Option<u64>,
        overage_status: Option<&str>,
        overage_resets_at: Option<u64>,
        overage_disabled_reason: Option<&str>,
        fallback_available: Option<bool>,
    ) {
        let _ = self.tx.send(TurnEvent::RateLimit {
            status: status.map(str::to_owned),
            rate_limit_type: rate_limit_type.map(str::to_owned),
            utilization,
            resets_at,
            claim_resets_at,
            overage_status: overage_status.map(str::to_owned),
            overage_resets_at,
            overage_disabled_reason: overage_disabled_reason.map(str::to_owned),
            fallback_available,
        });
    }

    async fn emit_raw_utilization(
        &self,
        five_hour_utilization: Option<f64>,
        five_hour_resets_at: Option<u64>,
        seven_day_utilization: Option<f64>,
        seven_day_resets_at: Option<u64>,
    ) {
        let _ = self.tx.send(TurnEvent::RawUtilization {
            five_hour_utilization,
            five_hour_resets_at,
            seven_day_utilization,
            seven_day_resets_at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn emit_text_translates_to_text_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_text("hello").await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::TextDelta(ref s) if s == "hello"));
    }

    #[tokio::test]
    async fn emit_thinking_translates_to_thinking_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_thinking("let me reason", Some("sig-abc")).await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::ThinkingDelta(ref s) if s == "let me reason"));
    }

    #[tokio::test]
    async fn emit_tool_call_translates_to_tool_use_start() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let id = protocol::ToolUseId::new();
        bridge
            .emit_tool_call(&id, "Read", &serde_json::json!({"file_path": "/tmp/x"}))
            .await;
        match rx.recv().await.unwrap() {
            TurnEvent::ToolUseStart {
                id: gid,
                tool,
                input,
            } => {
                assert_eq!(gid, id);
                assert_eq!(tool, "Read");
                assert_eq!(input["file_path"], "/tmp/x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_end_turn_endturn_reason_translates_to_endturn() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = traits::CostSnapshot::default();
        bridge.emit_end_turn("end_turn", &cost).await;
        // M6-06: emit_end_turn now precedes TurnEnded with a CostUpdated event.
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostUpdated(ref s) if s == "$0.0000"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::EndTurn)
        ));
    }

    #[tokio::test]
    async fn emit_end_turn_max_tokens_translates_to_maxturns() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = traits::CostSnapshot::default();
        bridge.emit_end_turn("max_tokens", &cost).await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostUpdated(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::MaxTurns)
        ));
    }

    #[tokio::test]
    async fn emit_end_turn_formats_real_cost_4dp() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = traits::CostSnapshot {
            total_usd: 0.0234,
            ..traits::CostSnapshot::default()
        };
        bridge.emit_end_turn("end_turn", &cost).await;
        match rx.recv().await.unwrap() {
            TurnEvent::CostUpdated(s) => assert_eq!(s, "$0.0234"),
            other => panic!("expected CostUpdated($0.0234), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_rate_limit_translates_to_rate_limit_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_rate_limit(
                Some("rejected"),
                Some("five_hour"),
                Some(0.95),
                Some(1_900_000_000),
                Some(1_900_000_100),
                Some("allowed_warning"),
                Some(1_900_000_200),
                Some("out_of_credits"),
                Some(true),
            )
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::RateLimit {
                status,
                rate_limit_type,
                utilization,
                resets_at,
                claim_resets_at,
                overage_status,
                overage_resets_at,
                overage_disabled_reason,
                fallback_available,
            } => {
                assert_eq!(status.as_deref(), Some("rejected"));
                assert_eq!(rate_limit_type.as_deref(), Some("five_hour"));
                assert_eq!(utilization, Some(0.95));
                assert_eq!(resets_at, Some(1_900_000_000));
                assert_eq!(claim_resets_at, Some(1_900_000_100));
                assert_eq!(overage_status.as_deref(), Some("allowed_warning"));
                assert_eq!(overage_resets_at, Some(1_900_000_200));
                assert_eq!(overage_disabled_reason.as_deref(), Some("out_of_credits"));
                assert_eq!(fallback_available, Some(true));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_rate_limit_all_none_still_forwards() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_rate_limit(None, None, None, None, None, None, None, None, None)
            .await;
        assert!(matches!(
            rx.try_recv().expect("bridge must forward a TurnEvent"),
            TurnEvent::RateLimit { status: None, .. }
        ));
    }

    #[tokio::test]
    async fn emit_context_pressure_translates_to_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_context_pressure(
                Some(traits::ContextPressureBanner {
                    text: "Context low (8% remaining) \u{00b7} Run /compact to compact & continue"
                        .into(),
                    level: traits::ContextPressureLevel::Error,
                }),
                0.92,
            )
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::ContextPressure {
                banner: Some(b),
                used_fraction,
            } => {
                assert_eq!(
                    b.text,
                    "Context low (8% remaining) \u{00b7} Run /compact to compact & continue"
                );
                assert_eq!(b.level, traits::ContextPressureLevel::Error);
                assert!((used_fraction - 0.92).abs() < 1e-6);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_context_pressure_none_clears() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_context_pressure(None, 0.0).await;
        assert!(matches!(
            rx.try_recv().expect("bridge must forward a TurnEvent"),
            TurnEvent::ContextPressure {
                banner: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn emit_raw_utilization_translates_to_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_raw_utilization(Some(0.42), Some(1_750_000_000), None, None)
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::RawUtilization {
                five_hour_utilization,
                five_hour_resets_at,
                seven_day_utilization,
                seven_day_resets_at,
            } => {
                assert_eq!(five_hour_utilization, Some(0.42));
                assert_eq!(five_hour_resets_at, Some(1_750_000_000));
                assert_eq!(seven_day_utilization, None);
                assert_eq!(seven_day_resets_at, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn turn_started_is_caller_emitted_not_bridge() {
        // TurnStarted is fired by the SPAWNER (app.rs), not the bridge.
        // Documenting that contract: the bridge has no method that
        // produces TurnStarted; callers send it manually.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(TurnEvent::TurnStarted).unwrap();
        assert!(matches!(rx.recv().await.unwrap(), TurnEvent::TurnStarted));
    }
}
