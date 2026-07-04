//! The main chat surface: transcript + bottom pane + per-turn state (plan
//! Phase 6).
//!
//! Modeled on codex-rs `tui/src/chatwidget.rs` (UI architecture pattern only —
//! no codex product types; `LingXi` keeps its own `RenderedMessage` /
//! [`SessionInfo`] / [`TurnEvent`] / [`PermissionExchange`] data model).
//! [`ChatWidget`] consumes orchestrator [`TurnEvent`]s into the
//! [`Transcript`]'s committed/active history cells, routes keys and pastes
//! through the [`BottomPane`] and maps the pane's local outcomes to app-level
//! [`ChatOutcome`]s, dispatches slash commands through the [`crate::command`]
//! registry, and serializes permission prompts through a pending queue (one
//! prompt owns the keyboard; later arrivals wait their turn).
//!
//! Its render surface is the exact four-method contract codex's `App` draws
//! its chat widget through (`codex-rs/tui/src/app.rs::render_chat_widget_frame`):
//! [`ChatWidget::desired_height`], [`ChatWidget::render`],
//! [`ChatWidget::cursor_pos`], and [`ChatWidget::cursor_style`].

use std::collections::VecDeque;
use std::io;
use std::io::Write;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio_util::sync::CancellationToken;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::{theme_for, Theme, ThemeName, ThemeSetting};

use crate::bottom_pane::permission_view::PermissionView;
use crate::bottom_pane::screen_view::ScreenView;
use crate::bottom_pane::theme_picker_view::ThemePickerView;
use crate::bottom_pane::{
    BottomPane, BottomPaneOutcome, BottomPaneStatus, CommandAction, ConnectAction, WebAction,
};
use crate::history_cell::message::AssistantTextCell;
use crate::history_cell::message::ThinkingCell;
use crate::renderable::Renderable;
use crate::session::SessionInfo;
use crate::transcript::Transcript;

/// What one routed key press or paste means to the owning event loop.
pub enum ChatOutcome {
    /// Keep looping.
    Continue,
    /// Exit the app.
    Quit,
    /// The user submitted `prompt`; the caller should drive a turn for it,
    /// honoring the paired [`CancellationToken`] (the widget cancels it on
    /// Ctrl-C).
    Submit(String, CancellationToken),
    /// The user picked a model in `/model`; the caller should switch to
    /// `(request_model, profile)` via `OrchestratorHandle::switch_model`.
    SwitchModel(String, Option<String>),
    /// The user committed a theme in the `/theme` picker. The widget has
    /// already applied it live; the caller should persist the setting
    /// (best-effort, `tui_core::theme_persist`).
    SetTheme(ThemeSetting),
    /// `/copy` resolved this text; the caller should write it to the system
    /// clipboard (best-effort, [`crate::copy::copy_to_clipboard_native`]).
    /// The confirmation message is already in the transcript.
    CopyToClipboard(String),
    /// A `/web` view asked the caller to run a secret/settings save or a test
    /// search. The caller runs it asynchronously and reports the result back
    /// through `TurnEvent::SystemNotice`.
    WebAction(WebAction),
    /// A `/connect` view asked the caller to store an API key or kick off a
    /// Copilot/OAuth sign-in. The caller runs it asynchronously and reports
    /// the result back through `TurnEvent::SystemNotice`.
    ConnectAction(ConnectAction),
}

/// The chat surface: owns the conversation state and the interactive footer,
/// leaving only loop plumbing (terminal, channels, callbacks) to the app.
pub struct ChatWidget {
    /// Conversation history: committed cells + the active streaming cell +
    /// the native-scrollback commit cursor + verbose/render mode.
    transcript: Transcript,
    /// The interactive footer: composer + completion + vim + status hints +
    /// the transient view stack. The pane routes local input; app-level
    /// intents come back as [`BottomPaneOutcome`]s the widget maps to
    /// [`ChatOutcome`]s.
    bottom_pane: BottomPane,
    /// Startup snapshot the read-only screens and the model picker render from.
    session: SessionInfo,
    /// Theme for transcript rendering (native-scrollback flush).
    theme: Theme,
    /// The active theme *preference* (drives the `/theme` picker's current
    /// marker; `Auto` re-resolves on apply).
    theme_setting: ThemeSetting,
    /// Resolved name of the active theme (the `/status`/`/config` rows).
    theme_name: ThemeName,
    /// Cancellation token for the in-flight turn, if any.
    current_turn: Option<CancellationToken>,
    /// When the in-flight turn began, for the spinner's elapsed-seconds counter.
    turn_started_at: Option<std::time::Instant>,
    /// Human label for what the turn is currently doing (e.g. `Running Bash`),
    /// set from `ToolUseStart` and shown by the spinner instead of a bare verb.
    activity: Option<String>,
    /// Permission requests waiting for the currently open prompt to resolve
    /// (prompts are serialized: one owns the keyboard at a time).
    pending_permissions: VecDeque<PermissionExchange>,
    /// Widget-lifetime clock driving the spinner's animation frame (and the
    /// `/stats` session-duration row).
    start: std::time::Instant,
    /// Where `/export` writes transcripts (default `~/.lingxi/exports`;
    /// overridable so tests and embedders stay hermetic).
    export_dir: std::path::PathBuf,
    /// Session-cumulative cost (`$0.0000`, 4-decimal claude-code parity) from
    /// the latest [`TurnEvent::CostUpdated`]; shown in the status row.
    cost: Option<String>,
    /// The last rate-limit notice text pushed to the transcript, so identical
    /// consecutive [`TurnEvent::RateLimit`] snapshots never stack (the old
    /// backend's `state.last_rate_limit_text` dedupe slot; reset by `/clear`
    /// with the scrollback).
    last_rate_limit_text: Option<String>,
    /// One-shot guard for the overage-transition notice
    /// (`useRateLimitWarningNotification.tsx` `hasShownOverageNotification`):
    /// set when the notice fires, reset when a `RateLimit` event shows the
    /// session has LEFT overage. Deliberately survives `/clear` — TS
    /// component state outlives transcript clears.
    has_shown_overage_notification: bool,
    /// Composition-root-shared subscription slot (`None` until the embedder
    /// wires one via [`Self::set_subscription`]; the root's background fetch
    /// fills it). Read at rate-limit compose time for subscription-granular
    /// copy; absent/unfilled degrades to the unknown-subscription default
    /// (TS-conservative), exactly like the old backend's
    /// `AppState::subscription_snapshot`.
    subscription: Option<traits::subscription::SharedSubscription>,
    /// Composition-root-shared custom-statusline slot (`None` unless the
    /// embedder wires one via [`Self::set_status_line`]). The widget writes the
    /// live pump inputs (cost / rate-limit utilization) + marks it dirty on
    /// `TurnEnded`; the async pump (CLI `run_ratatui`) runs the configured
    /// command and writes back `text`, which the bottom pane renders.
    status_line: Option<crate::status_line::SharedStatusLine>,
    /// Validated terminal escape sequences from
    /// [`TurnEvent::TerminalSequence`] (hook-returned, allowlisted +
    /// BEL-normalized by the orchestrator), staged until the app loop drains
    /// them via [`Self::take_terminal_sequences`] and writes the bytes to the
    /// terminal that owns the controlling tty (claude-code `BEo`).
    pending_terminal_sequences: Vec<String>,
    /// Live tool-call inputs keyed by `tool_use_id`, populated on
    /// [`TurnEvent::ToolUseStart`] and consumed on the paired
    /// [`TurnEvent::ToolUseResult`] to recover the Edit/Write diff fields
    /// (`old_string`/`new_string`/`file_path`) for the result cell — the same
    /// correlation the resume path does with its `tool_inputs` side-table.
    tool_inputs: std::collections::HashMap<protocol::ToolUseId, serde_json::Value>,
    /// Composition-root-shared `/web` config snapshot slot (`None` until the
    /// embedder wires one via [`Self::set_web_snapshot`]). [`Self::cmd_web`]
    /// reads a clone to seed the picker; the async `on_web_action` effect
    /// closure (CLI `run_ratatui`) updates it in place after a save/test so
    /// the NEXT `/web` open reflects the latest persisted state.
    web_snapshot: Option<std::sync::Arc<std::sync::Mutex<crate::web::picker::WebConfigSnapshot>>>,
    /// Per-provider login-method tag (from the catalog auth strategy), keyed
    /// by profile_name — the real data `/connect`'s picker groups/labels
    /// from. Empty (default) until [`Self::set_connect_data`] wires it.
    connect_auth_methods: std::collections::BTreeMap<String, String>,
    /// Per-provider availability flag (already has a usable credential),
    /// keyed by profile_name — joined into the `/connect` picker's `✓`
    /// marker. Empty (default) until [`Self::set_connect_data`] wires it.
    connect_availability: std::collections::BTreeMap<String, bool>,
}

impl ChatWidget {
    /// Build a widget seeded with an initial conversation (may be empty) and
    /// the startup [`SessionInfo`] snapshot.
    #[must_use]
    pub fn new(messages: Vec<RenderedMessage>, session: SessionInfo) -> Self {
        let theme = Theme::dark();
        Self {
            transcript: Transcript::from_messages(messages),
            bottom_pane: BottomPane::new(theme),
            session,
            theme,
            theme_setting: ThemeSetting::Named(ThemeName::Dark),
            theme_name: ThemeName::Dark,
            current_turn: None,
            turn_started_at: None,
            activity: None,
            pending_permissions: VecDeque::new(),
            start: std::time::Instant::now(),
            export_dir: crate::export::default_export_dir(),
            cost: None,
            last_rate_limit_text: None,
            has_shown_overage_notification: false,
            subscription: None,
            status_line: None,
            pending_terminal_sequences: Vec::new(),
            tool_inputs: std::collections::HashMap::new(),
            web_snapshot: None,
            connect_auth_methods: std::collections::BTreeMap::new(),
            connect_availability: std::collections::BTreeMap::new(),
        }
    }

    /// Override where `/export` writes transcripts (tests/embedders; the
    /// default is [`crate::export::default_export_dir`]).
    pub fn set_export_dir(&mut self, dir: std::path::PathBuf) {
        self.export_dir = dir;
    }

    /// Replace the startup [`SessionInfo`] snapshot the screens and the
    /// model picker render from.
    pub fn set_session(&mut self, session: SessionInfo) {
        self.session = session;
    }

    /// Apply a theme preference live: resolve it (`Auto` consults the OSC-11
    /// cache / `$COLORFGBG` / color depth) and swap the render palette on the
    /// transcript flush path and the bottom pane. Used by the startup theme
    /// load in [`crate::app::run_app`] and the `/theme` picker commit.
    pub fn set_theme(&mut self, setting: ThemeSetting) {
        self.theme_setting = setting;
        self.theme_name = setting.resolve();
        self.theme = theme_for(self.theme_name);
        self.bottom_pane.set_theme(self.theme);
    }

    /// The active theme preference (`/theme` picker current marker; tests).
    #[must_use]
    pub fn theme_setting(&self) -> ThemeSetting {
        self.theme_setting
    }

    /// The resolved name of the active theme (`/status` row; tests).
    #[must_use]
    pub fn theme_name(&self) -> ThemeName {
        self.theme_name
    }

    /// Route one key press: feed the pane the current turn status (Ctrl-C
    /// routing depends on it), route through the pane's layered input
    /// handling (active view, completion, vim, composer), execute the
    /// returned intent, then surface the next queued permission if the key
    /// resolved the open prompt.
    pub fn handle_key(&mut self, key: KeyEvent) -> ChatOutcome {
        self.bottom_pane.set_task_running(self.pane_status());
        let outcome = self.bottom_pane.handle_key(key);
        let outcome = self.on_pane_outcome(outcome);
        self.open_next_queued_permission();
        outcome
    }

    /// Route a bracketed paste through the pane (active view first, then
    /// image-path detection, then composer insertion).
    pub fn handle_paste(&mut self, text: &str) -> ChatOutcome {
        let outcome = self.bottom_pane.handle_paste(text);
        let outcome = self.on_pane_outcome(outcome);
        self.open_next_queued_permission();
        outcome
    }

    /// Fold one streaming event from the orchestrator bridge into the
    /// transcript: `TurnStarted` opens an empty active assistant cell,
    /// `TextDelta` mutates it in place, `ToolUseStart`/`ToolUseResult` set and
    /// clear the spinner activity, `TurnEnded` finalizes the active cell
    /// (moves it to the committed history) and clears the in-flight cancel
    /// token. `CostUpdated` refreshes the status-row cost, `ContextPressure`
    /// sets/clears the pane banner, `CompactionCompleted` folds a
    /// compact-boundary marker, `RateLimit` composes a transcript notice, and
    /// `TerminalSequence` stages a write-through escape — every bridge-emitted
    /// variant is handled (fix round 1: no wildcard drop).
    /// Commit the active streaming cell — but DISCARD it if it's the empty
    /// `AssistantTextCell` placeholder [`TurnEvent::TurnStarted`] opens. Used
    /// at every flush point that can fire before any assistant text streams (a
    /// leading tool call, a thinking-first turn, or a tool-only turn), so no
    /// stray bare `●` marker is committed with no body.
    fn flush_or_discard_active(&mut self) {
        let empty = self
            .transcript
            .mutate_active(|cell| {
                cell.as_any()
                    .downcast_ref::<AssistantTextCell>()
                    .is_some_and(|a| a.body().is_empty())
            })
            .unwrap_or(false);
        if empty {
            self.transcript.discard_active();
        } else {
            self.transcript.flush_active();
        }
    }

    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                self.turn_started_at = Some(std::time::Instant::now());
                self.activity = None;
                // A straggler active cell (missed TurnEnded) is finalized (or
                // discarded if it's the empty placeholder) before the new
                // streaming reply opens.
                self.flush_or_discard_active();
                self.transcript
                    .set_active(Box::new(AssistantTextCell::new(String::new())));
            }
            TurnEvent::TextDelta(delta) => {
                let appended = self
                    .transcript
                    .mutate_active(|cell| {
                        if let Some(assistant) =
                            cell.as_any_mut().downcast_mut::<AssistantTextCell>()
                        {
                            assistant.append(&delta);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !appended {
                    self.transcript.flush_active();
                    self.transcript
                        .set_active(Box::new(AssistantTextCell::new(delta)));
                }
            }
            TurnEvent::ThinkingDelta(delta) => {
                // M5 cc2.1.198 thinking streaming: append to the active
                // ThinkingCell (collapsed by default; Ctrl-O reveals the body),
                // mirroring the TextDelta path. A non-thinking active cell is
                // flushed first so a fresh collapsed thinking block opens; a
                // following TextDelta likewise flushes this thinking cell and
                // opens the assistant reply.
                let appended = self
                    .transcript
                    .mutate_active(|cell| {
                        if let Some(thinking) = cell.as_any_mut().downcast_mut::<ThinkingCell>() {
                            thinking.append(&delta);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !appended {
                    self.flush_or_discard_active();
                    self.transcript.set_active(Box::new(ThinkingCell::new(delta)));
                }
            }
            TurnEvent::ToolUseStart { id, tool, input } => {
                self.activity = Some(activity_label(&tool));
                // Commit any streamed assistant text ABOVE the tool call, then
                // render the tool-use header (`● {tool}` + input) into the
                // transcript — claude-code parity: each tool invocation shows
                // as its own scrollback cell between the assistant's text
                // segments. Without this flush the whole turn's text merged
                // into one active cell and every tool call was invisible.
                // Remember the input so the paired result can render the
                // Edit/Write diff (see `ToolUseResult`).
                self.flush_or_discard_active();
                self.tool_inputs.insert(id.clone(), input.clone());
                self.transcript
                    .push_message(RenderedMessage::AssistantToolUse { id, tool, input });
            }
            TurnEvent::ToolUseResult { id, tool, result } => {
                self.activity = None;
                // Recover the originating call's input (for diff tools) from the
                // side-table, mirroring the resume path's correlation, then
                // render the `⎿ {summary}` result cell (or an Edit/Write diff).
                let (old_string, new_string, file_path) = self
                    .tool_inputs
                    .remove(&id)
                    .map_or((None, None, None), |input| {
                        tui_core::active_turn::diff_inputs_for(&tool, &input)
                    });
                self.transcript.push_message(RenderedMessage::UserToolResult {
                    id,
                    tool,
                    result,
                    old_string,
                    new_string,
                    file_path,
                });
            }
            TurnEvent::TurnEnded(_) => {
                self.flush_or_discard_active();
                self.current_turn = None;
                self.turn_started_at = None;
                self.activity = None;
                // Re-arm the statusline pump (claude-code executes the command
                // on turn boundaries; the pump is debounced single-flight).
                self.with_status_line(|s| s.dirty = true);
            }
            TurnEvent::CostUpdated(cost_str) => {
                // Update the status-row cost so the next render pass shows the
                // post-turn dollar amount (old backend: `state.status.cost`).
                self.with_status_line(|s| s.data.cost = cost_str.clone());
                self.cost = Some(cost_str);
            }
            TurnEvent::ContextPressure {
                banner,
                used_fraction,
            } => {
                // (TokenWarning) The orchestrator-computed context-pressure
                // banner renders as its own pane row next pass; `None` clears a
                // previously-shown banner once the context drops below the
                // warning threshold (claude-code's `<TokenWarning>` returning
                // null).
                self.bottom_pane.set_context_pressure(banner);
                // Feed the live context usage into the statusline payload's
                // `context_window.used_percentage` (0-1 fraction).
                self.with_status_line(|s| s.data.context_pct = used_fraction);
            }
            TurnEvent::TerminalSequence { seq } => {
                // #6: stage the validated terminal escape sequence; the app
                // loop drains [`Self::take_terminal_sequences`] and writes the
                // bytes to the terminal that owns the controlling tty
                // (claude-code `BEo`; the old backend's async
                // `pump_terminal_sequence`).
                self.pending_terminal_sequences.push(seq);
            }
            TurnEvent::CompactionCompleted {
                messages_before,
                messages_after,
                ..
            } => {
                // M7-04 parity: fold a `CompactBoundary` marker (renders
                // `✻ Conversation compacted (ctrl+o for history)`; counts are
                // retained on the variant for debug/telemetry parity but not
                // rendered). Consecutive boundaries de-dupe to one marker —
                // the old backend's `push_compact_boundary` contract.
                self.push_compact_boundary(messages_before, messages_after);
            }
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
                self.apply_rate_limit(&crate::rate_limit_messages::RateLimitInfo {
                    status,
                    rate_limit_type,
                    utilization,
                    resets_at,
                    claim_resets_at,
                    overage_status,
                    overage_resets_at,
                    overage_disabled_reason,
                    fallback_available,
                });
            }
            // `RawUtilization`'s ONLY consumer is the configured statusline
            // command's `rate_limits` input: fold it into the shared pump slot
            // (no visible row of its own).
            TurnEvent::RawUtilization {
                five_hour_utilization,
                five_hour_resets_at,
                seven_day_utilization,
                seven_day_resets_at,
            } => {
                self.with_status_line(|s| {
                    s.data.raw_utilization =
                        Some(tui_core::status_line_command::RawUtilizationSnapshot {
                            five_hour_utilization,
                            five_hour_resets_at,
                            seven_day_utilization,
                            seven_day_resets_at,
                        });
                });
            }
            // The reserved `PermissionRequest` variant is never emitted by the
            // bridge — live prompts arrive through the `permission_bridge`
            // channel into [`Self::open_permission`] instead.
            TurnEvent::PermissionRequest { .. } => {}
            // A `/web` async effect (secret/settings save, test search)
            // finished off-loop; surface its result as a transcript line —
            // red for a failure, dim grey otherwise (`RenderedMessage::
            // SystemText`'s existing severity mapping).
            TurnEvent::SystemNotice { body, is_error } => {
                self.transcript.push_message(RenderedMessage::SystemText {
                    body,
                    timestamp: 0,
                    is_error,
                });
            }
        }
    }

    /// Fold a compaction boundary into the transcript, de-duping consecutive
    /// boundaries (exactly one marker renders even if two sources report the
    /// same compaction — the old backend's `push_compact_boundary`).
    fn push_compact_boundary(&mut self, messages_before: u32, messages_after: u32) {
        if self
            .transcript
            .committed_cells()
            .last()
            .is_some_and(|cell| {
                cell.as_any()
                    .downcast_ref::<crate::history_cell::system::CompactBoundaryCell>()
                    .is_some()
            })
        {
            return;
        }
        self.transcript
            .push_message(RenderedMessage::CompactBoundary {
                messages_before,
                messages_after,
            });
    }

    /// Fold one `TurnEvent::RateLimit` header snapshot: compose the
    /// claude-code notice (`getRateLimitMessage`) and push it unless it
    /// duplicates the last rendered text, then fire the one-shot
    /// overage-transition notice (`useRateLimitWarningNotification.tsx`) on
    /// entering overage — reset the flag on leaving it. Ported from the old
    /// backend's `streaming::apply_event` `RateLimit` arm.
    fn apply_rate_limit(&mut self, info: &crate::rate_limit_messages::RateLimitInfo) {
        // Subscription-granular copy: the snapshot the composition root
        // resolved (`None`/unfilled until the background fetch lands →
        // default = unknown subscription, TS-conservative).
        let sub = self.subscription_snapshot().unwrap_or_default();
        if let Some(composed) = crate::rate_limit_messages::compose_rate_limit(info, &sub) {
            if self.last_rate_limit_text.as_deref() != Some(composed.text.as_str()) {
                self.last_rate_limit_text = Some(composed.text.clone());
                self.transcript.push_message(RenderedMessage::RateLimit {
                    text: composed.text,
                    upsell: composed.upsell,
                });
            }
        }
        // Overage-transition notice: fire ONCE on entering overage when
        // `!isTeamOrEnterprise || hasBillingAccess` (tsx :62); reset the
        // one-shot flag on leaving overage (tsx :70-72). Guarded by its own
        // flag (not the text dedupe slot) and survives `/clear`, like the TS
        // component state.
        if crate::rate_limit_messages::is_using_overage(info) {
            if !self.has_shown_overage_notification
                && (!sub.is_team_or_enterprise() || sub.has_claude_ai_billing_access())
            {
                self.has_shown_overage_notification = true;
                self.transcript.push_message(RenderedMessage::RateLimit {
                    text: crate::rate_limit_messages::using_overage_text(info, &sub),
                    upsell: None,
                });
            }
        } else {
            self.has_shown_overage_notification = false;
        }
    }

    /// Current resolved subscription snapshot, if the embedder wired a slot
    /// and the seed/background fetch has filled it. A poisoned lock degrades
    /// to `None` (conservative copy, never a panic in the event-fold path) —
    /// the documented `SharedSubscription` reader stance.
    fn subscription_snapshot(&self) -> Option<traits::subscription::SubscriptionSnapshot> {
        self.subscription
            .as_ref()
            .and_then(|s| s.read().ok())
            .and_then(|guard| guard.clone())
    }

    /// Wire the composition root's shared subscription slot so the rate-limit
    /// composer reads the live snapshot at compose time (the old backend's
    /// `Runtime::with_subscription`).
    pub fn set_subscription(&mut self, slot: traits::subscription::SharedSubscription) {
        self.subscription = Some(slot);
    }

    /// Wire the composition root's shared custom-statusline slot and seed its
    /// static pump inputs (model + cwd) from the current session. The async
    /// pump reads this slot; `TurnEvent`s keep the live cost / rate-limit
    /// inputs fresh (see [`Self::apply_turn_event`]).
    pub fn set_status_line(&mut self, slot: crate::status_line::SharedStatusLine) {
        if let Ok(mut s) = slot.lock() {
            let (id, display) = self.current_model_id_display();
            s.data.model_id = id;
            s.data.model = display;
            s.data.cwd = std::path::PathBuf::from(&self.session.doctor.cwd);
            // Run once on startup (claude-code executes the statusline on mount,
            // not only on turn boundaries).
            s.dirty = true;
        }
        self.status_line = Some(slot);
    }

    /// Wire the composition root's shared `/web` config snapshot slot, preloaded
    /// from real config + credential-store presence at startup. [`Self::cmd_web`]
    /// reads a clone of it to seed the picker; the async `on_web_action` effect
    /// closure keeps it current across saves/tests.
    pub fn set_web_snapshot(
        &mut self,
        slot: std::sync::Arc<std::sync::Mutex<crate::web::picker::WebConfigSnapshot>>,
    ) {
        self.web_snapshot = Some(slot);
    }

    /// Wire the composition root's real per-provider login-method +
    /// availability maps (derived from the live multi-provider catalog at
    /// startup). [`Self::cmd_connect`] reads clones of both to build the
    /// `/connect` picker; empty maps (the default) render an empty picker.
    pub fn set_connect_data(
        &mut self,
        auth_methods: std::collections::BTreeMap<String, String>,
        availability: std::collections::BTreeMap<String, bool>,
    ) {
        self.connect_auth_methods = auth_methods;
        self.connect_availability = availability;
    }

    /// The current model's `(wire_id, display)` pair (falls back to
    /// `("(default)", "(default)")`), for the statusline payload's
    /// `model.id`/`model.display_name`.
    fn current_model_id_display(&self) -> (String, String) {
        self.session.models.iter().find(|m| m.is_current).map_or_else(
            || ("(default)".to_string(), "(default)".to_string()),
            |m| (m.request_model.clone(), m.display.clone()),
        )
    }

    /// The current model's display string (falls back to `(default)`), reused
    /// for the welcome banner + statusline seeding.
    fn current_model_display(&self) -> String {
        self.current_model_id_display().1
    }

    /// Update one field of the shared statusline slot (no-op when unwired).
    fn with_status_line<F: FnOnce(&mut crate::status_line::StatusLineShared)>(&self, f: F) {
        if let Some(slot) = &self.status_line {
            if let Ok(mut s) = slot.lock() {
                f(&mut s);
            }
        }
    }

    /// Drain the staged `TurnEvent::TerminalSequence` escapes (FIFO order).
    /// The app loop writes them through to the terminal's writer.
    #[must_use]
    pub fn take_terminal_sequences(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_terminal_sequences)
    }

    /// Open a permission prompt for `exchange` by pushing a
    /// [`PermissionView`]: it owns the keyboard until the user resolves it
    /// (Enter/1-3 approve or deny, Esc denies) and delivers the response
    /// through the exchange's one-shot channel exactly once. When a prompt is
    /// already open the exchange queues instead — prompts are serialized, and
    /// the queued one surfaces as soon as the open prompt resolves.
    pub fn open_permission(&mut self, exchange: PermissionExchange) {
        if self.has_open_permission() {
            self.pending_permissions.push_back(exchange);
        } else {
            self.bottom_pane.show_permission(exchange);
        }
    }

    /// Whether a permission prompt is anywhere on the view stack (queued
    /// exchanges wait in [`Self::open_permission`]'s queue until it resolves).
    #[must_use]
    pub fn has_open_permission(&self) -> bool {
        self.bottom_pane.view_stack().contains::<PermissionView>()
    }

    /// Route a recognized slash command through the [`crate::command`]
    /// registry. Returns `Some(outcome)` when the input resolves to a
    /// registered command (screen open, clear, exit, …), or `None` to fall
    /// through and send the input as a normal prompt.
    pub fn handle_slash(&mut self, input: &str) -> Option<ChatOutcome> {
        let (command, args) = crate::command::resolve(input)?;
        Some((command.run)(self, args))
    }

    /// Desired widget height at `width` columns: the streaming live tail (the
    /// active cell's rendered lines — zero when idle) stacked above the pane's
    /// own height (status + composer, grown to fit the active stacked view or
    /// the completion popup). The owner applies its viewport clamp policy.
    ///
    /// Uses [`BottomPane::desired_height_for`] with the freshly computed
    /// [`Self::pane_status`] running flag rather than
    /// `self.bottom_pane.desired_height` directly: the pane's own running
    /// flag only updates inside `handle_key`/`render`, so right after
    /// `apply_turn_event` starts or ends a turn — before either of those runs
    /// again — the pane's stored flag is stale. Height is now
    /// running-dependent (the status indicator adds a row), so measuring
    /// against the stale flag would undersize the viewport for one tick and
    /// clip the live tail.
    #[must_use]
    pub fn desired_height(&self, width: u16) -> u16 {
        let running = self.pane_status().running;
        self.live_tail_height(width)
            .saturating_add(self.status_line_height())
            .saturating_add(self.bottom_pane.desired_height_for(width, running))
    }

    /// Draw the widget into `area` of `buf`: refreshes the pane's task status
    /// first so the spinner text/animation reflect this tick's turn state,
    /// then draws the streaming live tail ABOVE the pane (this is the only
    /// render path for in-flight text — it is never committed to native
    /// scrollback until finalized), then the pane (status line + composer
    /// + overlays) pinned at the bottom.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.bottom_pane.set_task_running(self.pane_status());
        let tail = self.live_tail(area.width);
        let (tail_area, pane_area) = self.split_area(area, line_count(&tail));
        // A clipped tail shows its LAST rows (it is a tail: the newest
        // streamed lines stay visible next to the pane).
        let skip = tail.len().saturating_sub(usize::from(tail_area.height));
        for (row, line) in tail.iter().skip(skip).enumerate() {
            let row_area = Rect::new(
                tail_area.x,
                tail_area.y + u16::try_from(row).unwrap_or(u16::MAX),
                tail_area.width,
                1,
            );
            Renderable::render(line, row_area, buf);
        }
        // The custom statusline row(s) sit directly above the pane (below the
        // live tail). Carve them off the top of the pane area, drawn dim.
        let sl = self.status_line_lines();
        let sl_h = u16::try_from(sl.len()).unwrap_or(0).min(pane_area.height);
        for (row, line) in sl.iter().enumerate() {
            let row_area = Rect::new(
                pane_area.x,
                pane_area.y + u16::try_from(row).unwrap_or(u16::MAX),
                pane_area.width,
                1,
            );
            ratatui::widgets::Widget::render(
                ratatui::widgets::Paragraph::new(line.clone()),
                row_area,
                buf,
            );
        }
        let pane_area = Rect::new(
            pane_area.x,
            pane_area.y + sl_h,
            pane_area.width,
            pane_area.height.saturating_sub(sl_h),
        );
        self.bottom_pane.render(pane_area, buf);
    }

    /// The widget's cursor claim within `area` (the composer's cursor, or a
    /// full-frame view's; `None` → cursor hidden), positioned within the
    /// pane's sub-area below the live tail.
    #[must_use]
    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let (_, pane_area) = self.split_area(area, self.live_tail_height(area.width));
        // The statusline row(s) sit above the pane (see `render`); shift the
        // pane down by their height so the composer cursor lands correctly.
        let sl_h = self.status_line_height().min(pane_area.height);
        let pane_area = Rect::new(
            pane_area.x,
            pane_area.y + sl_h,
            pane_area.width,
            pane_area.height.saturating_sub(sl_h),
        );
        self.bottom_pane.cursor_pos(pane_area)
    }

    /// The cursor style applied when [`Self::cursor_pos`] claims a cursor.
    #[must_use]
    pub fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        let (_, pane_area) = self.split_area(area, self.live_tail_height(area.width));
        self.bottom_pane.cursor_style(pane_area)
    }

    /// Draw the widget into one terminal [`crate::terminal::Frame`] through
    /// its own render contract — the thin adapter codex keeps at the terminal
    /// draw boundary (`App::render_chat_widget_frame`): render into the
    /// frame's buffer, then copy the widget's cursor claim onto the frame (no
    /// claim → the terminal hides the cursor).
    pub fn render_frame(&mut self, frame: &mut crate::terminal::Frame<'_>) {
        let area = frame.area();
        self.render(area, frame.buffer_mut());
        if let Some(pos) = self.cursor_pos(area) {
            frame.set_cursor_position(pos);
            frame.set_cursor_style(self.cursor_style(area));
        }
    }

    /// Commit finalized transcript cells into the terminal's native scrollback
    /// via [`crate::terminal::Terminal::insert_history_lines`] (written ABOVE
    /// the bottom viewport), delegating to
    /// [`Transcript::flush_to_native_scrollback`]. The actively-streaming cell
    /// is never committed here (it still grows in place); it commits once as a
    /// whole when `TurnEnded` finalizes it.
    ///
    /// Generic over the backend so tests can drive it with a test backend; the
    /// runtime passes [`crate::RataTerminal`].
    ///
    /// # Errors
    /// Propagates the first terminal IO error from the history insertion.
    pub fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let width = terminal.size()?.width.max(1);
        self.transcript
            .flush_to_native_scrollback(terminal, width, &self.theme)
    }

    /// Read-only access to the transcript (rendering/tests).
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Read-only access to the bottom pane (rendering/tests).
    #[must_use]
    pub fn bottom_pane(&self) -> &BottomPane {
        &self.bottom_pane
    }

    /// Whether a turn is currently in flight.
    #[must_use]
    pub fn turn_running(&self) -> bool {
        self.current_turn.is_some()
    }

    // ===== Registry-dispatched command handlers (`crate::command::BUILTIN`) =====

    /// `/help`: open the shortcuts + slash-commands screen.
    pub(crate) fn cmd_help(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::help()));
        ChatOutcome::Continue
    }

    /// `/model`: open the model picker over [`SessionInfo::models`]. An empty
    /// model list still opens the picker — the view renders its own
    /// user-facing empty message (plan Phase 11 step 5; previously an
    /// app-side transcript dump).
    pub(crate) fn cmd_model(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_model_picker(self.session.models.clone());
        ChatOutcome::Continue
    }

    /// `/doctor`: open the diagnostics screen.
    pub(crate) fn cmd_doctor(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_view(Box::new(ScreenView::doctor(&self.session.doctor)));
        ChatOutcome::Continue
    }

    /// `/mcp`: open the MCP servers listing.
    pub(crate) fn cmd_mcp(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "MCP servers",
            "MCP servers",
            &self.session.mcp,
            "No MCP servers configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/hooks`: open the hooks listing.
    pub(crate) fn cmd_hooks(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Hooks",
            "Hooks",
            &self.session.hooks,
            "No hooks configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/agents`: open the agents listing.
    pub(crate) fn cmd_agents(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Agents",
            "Agents",
            &self.session.agents,
            "No agents configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/skills`: open the skills listing (captured at launch from the
    /// on-disk `.lingxi/skills/` directories).
    pub(crate) fn cmd_skills(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Skills",
            "Skills",
            &self.session.skills,
            "No skills found. Create skills in .lingxi/skills/ or ~/.lingxi/skills/",
        )));
        ChatOutcome::Continue
    }

    /// `/memory`: open the LINGXI.md memory-file listing (the tiers captured
    /// at launch; read-only — this backend has no in-TUI editor).
    pub(crate) fn cmd_memory(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Memory files",
            "LINGXI.md memory files",
            &self.session.memory,
            "No memory files found.",
        )));
        ChatOutcome::Continue
    }

    /// `/status`: open the session status screen (snapshot facts + live
    /// editor toggles).
    pub(crate) fn cmd_status(&mut self, _args: &str) -> ChatOutcome {
        let view = ScreenView::status(
            &self.session.doctor,
            self.session.models.iter().find(|m| m.is_current),
            self.bottom_pane.vim_enabled(),
            self.transcript.verbose(),
            self.theme_name,
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/config`: open the read-only settings screen (session-scoped
    /// settings + on-disk settings files).
    pub(crate) fn cmd_config(&mut self, _args: &str) -> ChatOutcome {
        let view = ScreenView::settings(
            self.theme_name,
            self.bottom_pane.vim_enabled(),
            self.transcript.verbose(),
            &self.session.doctor.lingxi_home,
            &self.session.doctor.cwd,
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/stats`: open the session statistics screen (live widget state:
    /// duration, prompt/reply counts, transcript size, current model).
    pub(crate) fn cmd_stats(&mut self, _args: &str) -> ChatOutcome {
        use crate::history_cell::message::{AssistantTextCell, UserTextCell};
        let cells = self.transcript.committed_cells();
        let prompts = cells
            .iter()
            .filter(|c| c.as_any().downcast_ref::<UserTextCell>().is_some())
            .count();
        let replies = cells
            .iter()
            .filter(|c| c.as_any().downcast_ref::<AssistantTextCell>().is_some())
            .count();
        let view = ScreenView::stats(
            self.start.elapsed().as_secs(),
            prompts,
            replies,
            cells.len(),
            self.session.models.iter().find(|m| m.is_current),
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/export [filename]`: write the transcript (every committed cell's
    /// copy-friendly raw lines — the raw-scrollback text) to a `.txt` file in
    /// the export dir, echoing the outcome as a `system` message. No arg →
    /// the timestamped default name; an existing target is never clobbered.
    pub(crate) fn cmd_export(&mut self, args: &str) -> ChatOutcome {
        use std::fmt::Write as _;
        let mut body = String::new();
        for cell in self.transcript.committed_cells() {
            for line in cell.raw_lines() {
                let _ = writeln!(body, "{line}");
            }
        }
        let (display, is_error) = match crate::export::write_export(&self.export_dir, args, &body) {
            Ok(path) => (
                format!("Conversation exported to: {}", path.display()),
                false,
            ),
            Err(crate::export::ExportError::Exists(path)) => (
                format!(
                    "Failed to export conversation: {} already exists (pass a different filename)",
                    path.display()
                ),
                true,
            ),
            Err(crate::export::ExportError::Io(err)) => {
                (format!("Failed to export conversation: {err}"), true)
            }
        };
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        ChatOutcome::Continue
    }

    /// `/copy [N]`: resolve the Nth-latest assistant text (1 = latest,
    /// default), echo the byte-locked confirmation/error, and hand the text
    /// to the caller for the actual clipboard write.
    pub(crate) fn cmd_copy(&mut self, args: &str) -> ChatOutcome {
        use crate::history_cell::message::AssistantTextCell;
        // Newest-first non-empty assistant bodies, capped at MAX_LOOKBACK
        // (claude-code `collectRecentAssistantTexts`).
        let texts: Vec<String> = self
            .transcript
            .committed_cells()
            .iter()
            .rev()
            .filter_map(|c| c.as_any().downcast_ref::<AssistantTextCell>())
            .map(|c| c.body().to_string())
            .filter(|body| !body.is_empty())
            .take(crate::copy::MAX_LOOKBACK)
            .collect();
        let (display, copy_text) = match crate::copy::parse_copy_command(&texts, args) {
            crate::copy::CopyCommand::Copy { text, display } => (display, Some(text)),
            crate::copy::CopyCommand::Error { display } => (display, None),
        };
        let is_error = copy_text.is_none();
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        copy_text.map_or(ChatOutcome::Continue, ChatOutcome::CopyToClipboard)
    }

    /// `/theme`: open the theme picker on the active setting. The commit
    /// comes back as [`CommandAction::SetTheme`] via the view stack.
    pub(crate) fn cmd_theme(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_view(Box::new(ThemePickerView::new(self.theme_setting)));
        ChatOutcome::Continue
    }

    /// `/web`: open the WebSearch provider picker, seeded from the shared
    /// snapshot (real config + credential-store presence, preloaded at
    /// startup and kept current by the async `on_web_action` effect closure).
    /// Falls back to the default (unconfigured) snapshot when no slot is
    /// wired (headless / tests).
    pub(crate) fn cmd_web(&mut self, _args: &str) -> ChatOutcome {
        let snapshot = self
            .web_snapshot
            .as_ref()
            .map(|m| m.lock().unwrap().clone())
            .unwrap_or_default();
        self.bottom_pane.show_web_picker(snapshot);
        ChatOutcome::Continue
    }

    /// `/connect [provider]`: with no argument, open the grouped provider
    /// picker seeded from the real catalog maps ([`Self::set_connect_data`]).
    /// With a `provider` argument, skip the picker and route directly into
    /// that provider's method-choice screen (multi-method) or its single
    /// flow — the key-entry view for `api_key`, or a
    /// [`ChatOutcome::ConnectAction`] effect for Copilot/OAuth providers.
    pub(crate) fn cmd_connect(&mut self, args: &str) -> ChatOutcome {
        let provider = args.trim();
        if provider.is_empty() {
            self.bottom_pane.show_connect_picker(
                self.connect_auth_methods.clone(),
                self.connect_availability.clone(),
            );
        } else {
            // /connect <provider>: route directly (skip picker) if known,
            // else open the picker.
            let tag = self.connect_auth_methods.get(provider).cloned();
            let methods = crate::connect::picker::provider_methods(provider, tag.as_deref());
            let label = crate::connect::picker::provider_label(provider);
            if methods.len() > 1 {
                self.bottom_pane.show_view(Box::new(
                    crate::bottom_pane::connect_method_view::ConnectMethodView::new(
                        provider.to_string(),
                        label,
                        methods,
                    ),
                ));
            } else {
                match methods.first().copied() {
                    Some(crate::connect::picker::ConnectMethod::ApiKey) | None => {
                        self.bottom_pane.show_view(Box::new(
                            crate::bottom_pane::connect_key_view::ConnectKeyView::new(provider),
                        ));
                    }
                    Some(crate::connect::picker::ConnectMethod::CopilotDevice) => {
                        return ChatOutcome::ConnectAction(ConnectAction::Copilot {
                            provider_id: provider.to_string(),
                        });
                    }
                    Some(_) => {
                        return ChatOutcome::ConnectAction(ConnectAction::OAuth {
                            provider_id: provider.to_string(),
                        });
                    }
                }
            }
        }
        ChatOutcome::Continue
    }

    /// `/color [name]`: set/clear/list the session accent color (tints the
    /// composer's `›` prompt). Pure parse ([`crate::color::parse_color_command`]) +
    /// a byte-locked `system` echo; the accent itself is session-only.
    pub(crate) fn cmd_color(&mut self, args: &str) -> ChatOutcome {
        let (display, is_error) = match crate::color::parse_color_command(args) {
            crate::color::ColorCommand::List { display } => (display, false),
            crate::color::ColorCommand::Reset { display } => {
                self.bottom_pane.set_accent(None);
                (display, false)
            }
            crate::color::ColorCommand::Set { name, display } => {
                self.bottom_pane
                    .set_accent(Some(crate::color::accent_color(&name)));
                (display, false)
            }
            crate::color::ColorCommand::Invalid { display } => (display, true),
        };
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        ChatOutcome::Continue
    }

    /// `/vim`: toggle vim editing mode and echo the new state.
    pub(crate) fn cmd_vim(&mut self, _args: &str) -> ChatOutcome {
        let now_on = self.bottom_pane.toggle_vim();
        self.transcript.push_message(RenderedMessage::SystemText {
            body: format!("Vim mode {}.", if now_on { "enabled" } else { "disabled" }),
            timestamp: 0,
            is_error: false,
        });
        ChatOutcome::Continue
    }

    /// `/clear`: drop the transcript (committed + active + commit cursor).
    /// The rate-limit dedupe slot resets with the scrollback: a suppressed
    /// identical notice would otherwise never reappear in the now-empty
    /// transcript. (`has_shown_overage_notification` deliberately NOT reset:
    /// the TS flag is component state, surviving transcript clears.)
    pub(crate) fn cmd_clear(&mut self, _args: &str) -> ChatOutcome {
        self.clear_transcript();
        ChatOutcome::Continue
    }

    /// Shared `/clear` effect (slash path and [`CommandAction::ClearTranscript`]
    /// view path): drop the transcript and the rate-limit dedupe slot together.
    fn clear_transcript(&mut self) {
        self.transcript.clear();
        self.last_rate_limit_text = None;
    }

    /// `/image <path>`: record an image message for `path` so a graphics
    /// terminal shows the real pixels.
    pub(crate) fn cmd_image(&mut self, args: &str) -> ChatOutcome {
        self.push_image(args)
    }

    /// The active streaming cell's rendered lines at `width` (empty when
    /// idle) — [`Transcript::visible_live_tail`] under the widget's theme.
    fn live_tail(&self, width: u16) -> Vec<ratatui::text::Line<'static>> {
        self.transcript.visible_live_tail(width, &self.theme)
    }

    /// How many rows the live tail wants at `width` (zero when idle).
    fn live_tail_height(&self, width: u16) -> u16 {
        line_count(&self.live_tail(width))
    }

    /// The custom-statusline command output as dim rows, read from the shared
    /// pump slot (empty when no statusline is configured or the command has not
    /// produced output yet). Rendered just above the bottom pane.
    fn status_line_lines(&self) -> Vec<ratatui::text::Line<'static>> {
        /// Cap on statusline rows so a pathological multi-line command cannot
        /// steal the whole pane and hide the composer (claude-code statuslines
        /// are single-line by convention; this is a defensive bound).
        const MAX_STATUS_LINE_ROWS: usize = 3;
        let Some(slot) = &self.status_line else {
            return Vec::new();
        };
        let Ok(shared) = slot.lock() else {
            return Vec::new();
        };
        let Some(text) = shared.text.as_deref() else {
            return Vec::new();
        };
        // `statusLine.padding` → left pad (claude-code `<Box paddingX>`); 0 default.
        let pad = " ".repeat(shared.config.as_ref().map_or(0, |c| c.padding));
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        text.lines()
            .take(MAX_STATUS_LINE_ROWS)
            .map(|l| {
                ratatui::text::Line::from(ratatui::text::Span::styled(
                    format!("{pad}{l}"),
                    ratatui::style::Style::default().fg(dim),
                ))
            })
            .collect()
    }

    /// Row count the custom statusline wants (0 when none).
    fn status_line_height(&self) -> u16 {
        u16::try_from(self.status_line_lines().len()).unwrap_or(u16::MAX)
    }

    /// Split `area` into `(tail_area, pane_area)`: the pane keeps (at least)
    /// its desired height pinned at the bottom; the live tail takes the rows
    /// above, clipped to its own `tail_lines` count. Idle (empty tail) the
    /// pane keeps the WHOLE area, preserving the Phase 5 layout zones
    /// unchanged.
    fn split_area(&self, area: Rect, tail_lines: u16) -> (Rect, Rect) {
        let pane_desired = self.bottom_pane.desired_height(area.width);
        let tail_height = tail_lines.min(area.height.saturating_sub(pane_desired.min(area.height)));
        let tail_area = Rect::new(area.x, area.y, area.width, tail_height);
        let pane_area = Rect::new(
            area.x,
            area.y + tail_height,
            area.width,
            area.height - tail_height,
        );
        (tail_area, pane_area)
    }

    /// The pane's task-status input, recomputed from the widget's turn state
    /// (the pane holds no turn state of its own — plan Phase 5 boundary).
    fn pane_status(&self) -> BottomPaneStatus {
        BottomPaneStatus {
            running: self.current_turn.is_some(),
            text: self.spinner_text(),
            cost: self.cost.clone(),
        }
    }

    /// Execute the app-level intent the pane returned from a key or paste.
    fn on_pane_outcome(&mut self, outcome: BottomPaneOutcome) -> ChatOutcome {
        match outcome {
            BottomPaneOutcome::Consumed => ChatOutcome::Continue,
            BottomPaneOutcome::Quit => ChatOutcome::Quit,
            BottomPaneOutcome::Interrupt => {
                if let Some(token) = self.current_turn.take() {
                    token.cancel();
                }
                self.turn_started_at = None;
                self.activity = None;
                ChatOutcome::Continue
            }
            BottomPaneOutcome::ToggleVerbose => {
                self.transcript.toggle_verbose();
                self.bottom_pane.set_verbose(self.transcript.verbose());
                ChatOutcome::Continue
            }
            BottomPaneOutcome::Submitted(text) => self.dispatch_submission(text),
            BottomPaneOutcome::SubmitPrompt(prompt) => self.submit_prompt(prompt),
            BottomPaneOutcome::SwitchModel {
                request_model,
                profile,
            } => {
                self.transcript.push_message(RenderedMessage::SystemText {
                    body: format!("Switching model to {request_model}…"),
                    timestamp: 0,
                    is_error: false,
                });
                // Refresh the statusline model + re-arm the pump so the command
                // reports the new model (claude-code re-runs on model change).
                let display = self
                    .session
                    .models
                    .iter()
                    .find(|m| m.request_model == request_model)
                    .map_or_else(|| request_model.clone(), |m| m.display.clone());
                self.with_status_line(|s| {
                    s.data.model_id = request_model.clone();
                    s.data.model = display;
                    s.dirty = true;
                });
                ChatOutcome::SwitchModel(request_model, profile)
            }
            BottomPaneOutcome::RunCommand(action) => self.run_command(action),
            BottomPaneOutcome::PastedImage(path) => self.push_image(&path),
            BottomPaneOutcome::RunWebAction(action) => ChatOutcome::WebAction(action),
            BottomPaneOutcome::RunConnectAction(action) => ChatOutcome::ConnectAction(action),
        }
    }

    /// Route a submitted composer buffer: a registered slash command
    /// dispatches through the registry; anything else is sent as a prompt.
    fn dispatch_submission(&mut self, text: String) -> ChatOutcome {
        if let Some(outcome) = self.handle_slash(&text) {
            return outcome;
        }
        self.submit_prompt(text)
    }

    /// Record `text` as the user's prompt and hand it to the caller with a
    /// fresh per-turn cancellation token. Shared by the composer submit path
    /// and [`BottomPaneOutcome::SubmitPrompt`].
    fn submit_prompt(&mut self, text: String) -> ChatOutcome {
        self.transcript.push_message(RenderedMessage::UserText {
            body: text.clone(),
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        ChatOutcome::Submit(text, token)
    }

    /// Execute a command effect a view requested via
    /// [`BottomPaneOutcome::RunCommand`].
    fn run_command(&mut self, action: CommandAction) -> ChatOutcome {
        match action {
            CommandAction::ClearTranscript => {
                self.clear_transcript();
                ChatOutcome::Continue
            }
            CommandAction::Quit => ChatOutcome::Quit,
            CommandAction::SetTheme(setting) => {
                // Applied live here; the caller persists it (best-effort).
                self.set_theme(setting);
                ChatOutcome::SetTheme(setting)
            }
        }
    }

    /// Record an image message for `path` (metadata = the file name). Shared
    /// by `/image <path>` and pasted-image-path routing.
    fn push_image(&mut self, path: &str) -> ChatOutcome {
        let path = path.trim().to_string();
        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from);
        self.transcript.push_message(RenderedMessage::UserImage {
            image_id: None,
            metadata: name,
            source_path: Some(path),
        });
        ChatOutcome::Continue
    }

    /// Surface the oldest queued permission once no prompt is open (called
    /// after every key/paste, i.e. after a resolution could have happened).
    fn open_next_queued_permission(&mut self) {
        if !self.has_open_permission() {
            if let Some(exchange) = self.pending_permissions.pop_front() {
                self.bottom_pane.show_permission(exchange);
            }
        }
    }

    /// The current streaming-spinner text: an animated Claude-accent glyph, the
    /// live activity (`Running Bash` from `ToolUseStart`, else `Working`), and an
    /// elapsed-seconds counter with an interrupt hint — claude-code status parity.
    fn spinner_text(&self) -> String {
        const FRAMES: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"];
        let idx =
            usize::try_from(self.start.elapsed().as_millis() / 120).unwrap_or(0) % FRAMES.len();
        let verb = self.activity.as_deref().unwrap_or("Working");
        let secs = self.turn_started_at.map_or(0, |t| t.elapsed().as_secs());
        format!("{} {verb}… ({secs}s · esc to interrupt)", FRAMES[idx])
    }
}

/// `lines.len()` as a saturating `u16` (row heights are `u16` everywhere).
fn line_count(lines: &[ratatui::text::Line<'static>]) -> u16 {
    u16::try_from(lines.len()).unwrap_or(u16::MAX)
}

/// Human label shown in the spinner for an in-flight tool call, mapping the
/// tool name to a claude-code-style gerund (`Bash` → `Running Bash`).
fn activity_label(tool: &str) -> String {
    match tool {
        "Bash" | "BashOutput" => "Running Bash".to_string(),
        "Read" => "Reading".to_string(),
        "Write" => "Writing".to_string(),
        "Edit" | "MultiEdit" => "Editing".to_string(),
        "Grep" | "Glob" => "Searching".to_string(),
        "WebFetch" | "WebSearch" => "Browsing".to_string(),
        "Task" => "Delegating".to_string(),
        other => format!("Running {other}"),
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use crossterm::event::{KeyCode, KeyModifiers};
    use permission::gate::{PermissionRequest, PermissionResponse};
    use ratatui::layout::Position;
    use tokio::sync::oneshot;

    use super::*;
    use crate::bottom_pane::model_picker_view::ModelPickerView;
    use crate::bottom_pane::{BottomPaneView, ViewOutcome};
    use crate::history_cell::attachments::UserImageCell;
    use crate::session::ModelRow;
    use crate::terminal::test_support::TestWriteBackend;
    use crate::terminal::Terminal;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn widget() -> ChatWidget {
        ChatWidget::new(Vec::new(), SessionInfo::default())
    }

    fn typ(widget: &mut ChatWidget, s: &str) {
        for c in s.chars() {
            widget.handle_key(press(KeyCode::Char(c)));
        }
    }

    fn submit_command(widget: &mut ChatWidget, cmd: &str) -> ChatOutcome {
        typ(widget, cmd);
        widget.handle_key(press(KeyCode::Enter))
    }

    /// The transcript's cells in order — committed then the active
    /// (streaming) cell — for per-variant cell-level assertions (the
    /// message-cells split replaced the old `messages()` reconstruction).
    fn cells(widget: &ChatWidget) -> Vec<&dyn crate::history_cell::HistoryCell> {
        let mut out: Vec<&dyn crate::history_cell::HistoryCell> = widget
            .transcript
            .committed_cells()
            .iter()
            .map(AsRef::as_ref)
            .collect();
        out.extend(widget.transcript.active_cell());
        out
    }

    /// Downcast transcript cell `idx` (committed order, active last) to its
    /// concrete cell type.
    fn cell<T: 'static>(widget: &ChatWidget, idx: usize) -> &T {
        cells(widget)[idx]
            .as_any()
            .downcast_ref::<T>()
            .expect("concrete cell type")
    }

    fn tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (resp_tx, resp_rx) = oneshot::channel();
        let request = PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "ls -la" }),
            default_decision: permission::gate::PromptDefault::DenyByDefault,
        };
        (
            PermissionExchange {
                request,
                resp_tx,
                worker: None,
            },
            resp_rx,
        )
    }

    fn widget_with_models() -> ChatWidget {
        ChatWidget::new(
            Vec::new(),
            SessionInfo {
                models: vec![
                    ModelRow {
                        display: "Opus".into(),
                        request_model: "claude-opus".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: true,
                    },
                    ModelRow {
                        display: "Sonnet".into(),
                        request_model: "claude-sonnet".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: false,
                    },
                ],
                ..Default::default()
            },
        )
    }

    /// An 80x24 bottom-anchored test terminal with a 4-row viewport.
    fn test_terminal() -> Terminal<TestWriteBackend> {
        let mut terminal =
            Terminal::with_options(TestWriteBackend::new(80, 24)).expect("test terminal");
        terminal.set_bottom_viewport_height(4).expect("viewport");
        terminal
    }

    /// A tool call with NO preamble text (the common agentic case) renders the
    /// tool-use + result cells and does NOT leave a stray empty assistant
    /// placeholder — the `TurnStarted` empty cell is discarded, not committed.
    #[test]
    fn tool_first_turn_renders_tool_cells_without_empty_placeholder() {
        use crate::history_cell::tool::{ToolResultCell, ToolUseCell};
        let mut widget = widget();
        submit_command(&mut widget, "run it");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        // Straight to a tool call — no TextDelta first.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            input: serde_json::json!({ "file_path": "/tmp/x" }),
        });
        widget.apply_turn_event(TurnEvent::ToolUseResult {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            result: serde_json::json!("file body"),
        });
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        // Exactly: user "run it" · ● Read · ⎿ result — NO empty assistant cell.
        assert_eq!(widget.transcript.committed_cells().len(), 3);
        assert_eq!(cell::<ToolUseCell>(&widget, 1).tool(), "Read");
        assert!(cells(&widget)[2].as_any().downcast_ref::<ToolResultCell>().is_some());
        // The (empty) placeholder must not survive as an assistant cell.
        assert!(cells(&widget)
            .iter()
            .all(|c| c.as_any().downcast_ref::<AssistantTextCell>().is_none()));
    }

    #[test]
    fn new_seeds_transcript_and_starts_idle() {
        let seed = RenderedMessage::SystemText {
            body: "seeded".to_string(),
            timestamp: 0,
            is_error: false,
        };
        let widget = ChatWidget::new(vec![seed], SessionInfo::default());
        assert_eq!(widget.transcript().committed_cells().len(), 1);
        assert!(widget.transcript().active_cell().is_none());
        assert!(!widget.turn_running());
        assert!(!widget.has_open_permission());
        assert!(widget.pending_permissions.is_empty());
    }

    #[test]
    fn event_lifecycle_started_deltas_tools_ended() {
        let mut widget = widget();
        let outcome = submit_command(&mut widget, "go");
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "go"));
        assert!(widget.turn_running());

        // TurnStarted opens an empty active assistant cell and starts the clock.
        widget.apply_turn_event(TurnEvent::TurnStarted);
        assert!(widget.turn_started_at.is_some());
        assert!(widget.transcript.active_cell().is_some());

        // Deltas grow the active cell in place.
        widget.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        widget.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        assert_eq!(cell::<AssistantTextCell>(&widget, 1).body(), "Hello");

        // ToolUseStart sets the spinner activity AND commits the streamed
        // text above the tool call, then renders the tool-use cell; the paired
        // ToolUseResult renders the result cell and clears the activity.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert_eq!(widget.activity.as_deref(), Some("Running Bash"));
        assert!(widget.spinner_text().contains("Running Bash"));
        // The "Hello" reply flushed to a committed cell; the tool-use cell
        // renders after it (cells: user, Hello, ● Bash).
        assert_eq!(
            cell::<crate::history_cell::tool::ToolUseCell>(&widget, 2).tool(),
            "Bash"
        );
        widget.apply_turn_event(TurnEvent::ToolUseResult {
            id: protocol::ToolUseId::from("t1"),
            tool: "Bash".to_string(),
            result: serde_json::json!({}),
        });
        assert!(widget.activity.is_none());

        // TurnEnded finalizes the reply and clears every per-turn field.
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(!widget.turn_running());
        assert!(widget.turn_started_at.is_none());
        assert!(widget.transcript.active_cell().is_none(), "cell flushed");
        // user "hi" · assistant "Hello" · ● Bash tool-use · ⎿ Bash result = 4.
        assert_eq!(widget.transcript.committed_cells().len(), 4);
    }

    #[test]
    fn active_streaming_tail_stays_live_until_turn_ends() {
        let mut widget = widget();
        submit_command(&mut widget, "hi");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::TextDelta("stream-tail-text".to_string()));
        let theme = Theme::dark();
        let tail: String = widget
            .transcript()
            .visible_live_tail(80, &theme)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(tail.contains("stream-tail-text"), "live tail: {tail}");

        // Flushing commits the finalized user message but holds the tail back.
        let mut terminal = test_terminal();
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 1);

        // TurnEnded finalizes: the reply commits as a whole, the tail empties.
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 2);
        assert!(widget.transcript().visible_live_tail(80, &theme).is_empty());
    }

    #[test]
    fn ctrl_c_cancels_the_turn_and_clears_activity() {
        let mut widget = widget();
        typ(&mut widget, "x");
        let ChatOutcome::Submit(_, token) = widget.handle_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            input: serde_json::json!({}),
        });
        assert!(!token.is_cancelled());
        let outcome = widget.handle_key(ctrl(KeyCode::Char('c')));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(token.is_cancelled());
        assert!(!widget.turn_running());
        assert!(widget.activity.is_none());
        assert!(widget.turn_started_at.is_none());
    }

    #[test]
    fn slash_dispatch_routes_through_the_registry() {
        let mut widget = widget();
        // Registered command: focused view opens, no prompt turn starts.
        let outcome = submit_command(&mut widget, "/help");
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(widget.bottom_pane().view_stack().contains::<ScreenView>());
        assert!(widget.transcript().is_empty(), "no scrollback dump");
        assert!(!widget.turn_running());
        widget.handle_key(press(KeyCode::Esc)); // close /help

        // Aliases dispatch to the same registry entry.
        assert!(matches!(
            submit_command(&mut widget, "/quit"),
            ChatOutcome::Quit
        ));

        // Unregistered input falls through as a normal prompt.
        let outcome = submit_command(&mut widget, "/frobnicate");
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "/frobnicate"));

        // Argument gating mirrors the registry: no-arg commands reject
        // trailing args, arg-taking commands require them.
        assert!(widget.handle_slash("/help extra").is_none());
        assert!(widget.handle_slash("/image").is_none());
    }

    #[test]
    fn slash_image_dispatches_with_arguments() {
        let mut widget = widget();
        let outcome = submit_command(&mut widget, "/image /tmp/pic.png");
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert_eq!(cells(&widget).len(), 1);
        let image = cell::<UserImageCell>(&widget, 0);
        assert_eq!(image.source_path(), Some("/tmp/pic.png"));
        assert_eq!(image.metadata(), Some("pic.png"));
    }

    #[test]
    fn second_permission_request_queues_until_the_first_resolves() {
        let mut widget = widget();
        let (first, first_rx) = tool_exchange();
        let (second, second_rx) = tool_exchange();
        widget.open_permission(first);
        widget.open_permission(second);
        assert!(widget.has_open_permission());
        assert_eq!(
            widget.bottom_pane().view_stack().len(),
            1,
            "second prompt queues instead of stacking"
        );
        assert_eq!(widget.pending_permissions.len(), 1);

        // Resolving the first ('1' = AllowOnce) surfaces the queued prompt.
        widget.handle_key(press(KeyCode::Char('1')));
        assert_eq!(
            first_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        assert!(widget.has_open_permission(), "queued prompt opened");
        assert!(widget.pending_permissions.is_empty());

        // The surfaced prompt owns the keyboard until resolved (Esc denies).
        widget.handle_key(press(KeyCode::Esc));
        assert_eq!(second_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        assert!(!widget.has_open_permission());

        // Responses were one-shot; the keyboard belongs to the composer again.
        widget.handle_key(press(KeyCode::Char('z')));
        assert_eq!(widget.bottom_pane().composer().text(), "z");
    }

    #[test]
    fn queued_permissions_serialize_across_variants_and_drop_closes_channels() {
        let mut widget = widget();
        let (first, first_rx) = tool_exchange();
        let (plan_tx, plan_rx) = oneshot::channel();
        let plan = PermissionExchange {
            request: PermissionRequest::ExitPlanMode {
                plan: "1. Foo".to_string(),
            },
            resp_tx: plan_tx,
            worker: None,
        };
        widget.open_permission(first);
        widget.open_permission(plan);
        assert_eq!(widget.pending_permissions.len(), 1);
        // Resolve the first; the queued plan-approval prompt surfaces with its
        // own variant-specific keyboard ('2' = auto-accept edits).
        widget.handle_key(press(KeyCode::Esc));
        assert_eq!(first_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        assert!(widget.has_open_permission(), "queued plan prompt surfaced");
        widget.handle_key(press(KeyCode::Char('2')));
        assert_eq!(
            plan_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
        assert!(!widget.has_open_permission());

        // Dropping the widget with an open AND a queued exchange closes both
        // channels unsent (the gate maps a dropped resp_tx to a deny) instead
        // of leaking unanswerable prompts.
        let mut widget = ChatWidget::new(Vec::new(), SessionInfo::default());
        let (open, open_rx) = tool_exchange();
        let (queued, queued_rx) = tool_exchange();
        widget.open_permission(open);
        widget.open_permission(queued);
        drop(widget);
        assert!(open_rx.blocking_recv().is_err());
        assert!(queued_rx.blocking_recv().is_err());
    }

    #[test]
    fn model_switch_reports_request_model_and_profile() {
        let mut widget = widget_with_models();
        assert!(matches!(
            submit_command(&mut widget, "/model"),
            ChatOutcome::Continue
        ));
        assert!(widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // Picker owns the keyboard: move up to the first (Opus) row and confirm.
        widget.handle_key(press(KeyCode::Up));
        let outcome = widget.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ChatOutcome::SwitchModel(ref m, ref p)
                if m == "claude-opus" && p.as_deref() == Some("anthropic")
        ));
        assert!(!widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // The switch is echoed as a system message.
        let body = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body();
        assert!(body.contains("Switching model to claude-opus"), "{body}");
    }

    #[test]
    fn skills_memory_status_config_open_focused_views_not_scrollback_dumps() {
        // Criterion 18: each command opens a focused ScreenView and leaves
        // the transcript untouched (no scrollback dump), then Esc returns to
        // the composer.
        let session = SessionInfo {
            skills: vec![crate::session::InfoRow::new(
                "brainstorming",
                Some("Project skills · explore ideas".to_string()),
            )],
            memory: vec![crate::session::InfoRow::new(
                "Project memory",
                Some("Checked in at ./LINGXI.md".to_string()),
            )],
            ..Default::default()
        };
        for cmd in ["/skills", "/memory", "/status", "/config"] {
            let mut widget = ChatWidget::new(Vec::new(), session.clone());
            let outcome = submit_command(&mut widget, cmd);
            assert!(matches!(outcome, ChatOutcome::Continue), "{cmd}");
            assert!(
                widget.bottom_pane().view_stack().contains::<ScreenView>(),
                "{cmd} opens a focused view"
            );
            assert!(
                widget.transcript().is_empty(),
                "{cmd} must not dump into scrollback"
            );
            assert!(!widget.turn_running(), "{cmd} starts no turn");
            widget.handle_key(press(KeyCode::Esc));
            assert!(
                widget.bottom_pane().view_stack().is_empty(),
                "{cmd} closes back to the composer"
            );
        }
    }

    #[test]
    fn slash_theme_opens_picker_and_enter_applies_and_reports_for_persistence() {
        let mut widget = widget();
        assert_eq!(
            widget.theme_setting(),
            ThemeSetting::Named(ThemeName::Dark),
            "hermetic default"
        );
        assert!(matches!(
            submit_command(&mut widget, "/theme"),
            ChatOutcome::Continue
        ));
        assert!(widget
            .bottom_pane()
            .view_stack()
            .contains::<ThemePickerView>());
        // Move from "Dark mode" (index 1) to "Light mode" (index 2), commit.
        widget.handle_key(press(KeyCode::Down));
        let outcome = widget.handle_key(press(KeyCode::Enter));
        // The theme is applied live AND surfaced so the app can persist it.
        assert!(matches!(
            outcome,
            ChatOutcome::SetTheme(ThemeSetting::Named(ThemeName::Light))
        ));
        assert_eq!(
            widget.theme_setting(),
            ThemeSetting::Named(ThemeName::Light)
        );
        assert_eq!(widget.theme_name(), ThemeName::Light);
        assert_eq!(widget.theme, tui_core::theme::theme_for(ThemeName::Light));
        assert!(
            widget.bottom_pane().view_stack().is_empty(),
            "picker closes on commit"
        );
        assert!(widget.transcript().is_empty(), "no scrollback dump");

        // Esc cancels without touching the applied theme.
        submit_command(&mut widget, "/theme");
        widget.handle_key(press(KeyCode::Esc));
        assert!(widget.bottom_pane().view_stack().is_empty());
        assert_eq!(widget.theme_name(), ThemeName::Light);
    }

    #[test]
    fn slash_color_sets_resets_lists_and_rejects_with_system_echoes() {
        use crate::history_cell::system::SystemTextCell;
        let mut widget = widget();

        // Set: accent applied to the pane + byte-locked echo.
        submit_command(&mut widget, "/color cyan");
        assert_eq!(
            widget.bottom_pane().accent(),
            Some(crate::color::accent_color("cyan"))
        );
        assert_eq!(
            cell::<SystemTextCell>(&widget, 0).body(),
            "Session color set to: cyan"
        );

        // Invalid: error echo, accent untouched.
        submit_command(&mut widget, "/color chartreuse");
        assert!(widget.bottom_pane().accent().is_some());
        let invalid = cell::<SystemTextCell>(&widget, 1);
        assert!(invalid.body().starts_with("Invalid color \"chartreuse\"."));
        assert!(invalid.is_error(), "invalid color echoes as an error");

        // Reset alias: accent cleared.
        submit_command(&mut widget, "/color default");
        assert_eq!(widget.bottom_pane().accent(), None);
        assert_eq!(
            cell::<SystemTextCell>(&widget, 2).body(),
            "Session color reset to default"
        );

        // Bare /color: lists the available colors (ArgSpec::Optional).
        submit_command(&mut widget, "/color");
        let list = cell::<SystemTextCell>(&widget, 3);
        assert!(
            list.body().starts_with("Please provide a color."),
            "{}",
            list.body()
        );
        assert!(!widget.turn_running(), "no prompt turn for /color");
    }

    #[test]
    fn slash_stats_opens_session_stats_view_with_real_counts() {
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::UserText {
                    body: "hi".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "hello".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::SystemText {
                    body: "note".to_string(),
                    timestamp: 0,
                    is_error: false,
                },
            ],
            SessionInfo::default(),
        );
        assert!(matches!(
            submit_command(&mut widget, "/stats"),
            ChatOutcome::Continue
        ));
        let stats = widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| v.as_any().downcast_ref::<ScreenView>())
            .expect("stats screen open");
        let text = stats.body_text();
        assert!(text.contains("This session"), "{text}");
        // 1 user prompt, 1 assistant reply, 3 committed cells.
        let count_of = |label: &str| {
            text.lines()
                .find(|l| l.contains(label))
                .unwrap_or_else(|| panic!("no {label} row:\n{text}"))
                .rsplit(' ')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(count_of("Prompts sent"), "1", "{text}");
        assert_eq!(count_of("Replies received"), "1", "{text}");
        assert_eq!(count_of("Transcript cells"), "3", "{text}");
        assert!(text.contains("Counts cover this session only."), "{text}");
        assert_eq!(widget.transcript().committed_cells().len(), 3, "no dump");
        widget.handle_key(press(KeyCode::Esc));
        assert!(widget.bottom_pane().view_stack().is_empty());
    }

    #[test]
    fn slash_export_writes_raw_transcript_and_never_clobbers() {
        let dir = std::env::temp_dir().join(format!("tui-rata-cmd-export-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::UserText {
                    body: "hi".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "hello there".to_string(),
                    timestamp: 0,
                },
            ],
            SessionInfo::default(),
        );
        widget.set_export_dir(dir.clone());

        // Named export writes the committed cells' raw (copy-friendly) text.
        assert!(matches!(
            submit_command(&mut widget, "/export conv"),
            ChatOutcome::Continue
        ));
        let body = std::fs::read_to_string(dir.join("conv.txt")).expect("exported file");
        assert!(body.contains("hi"), "{body}");
        assert!(body.contains("hello there"), "{body}");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 2);
        assert!(
            echo.body().starts_with("Conversation exported to: "),
            "{}",
            echo.body()
        );
        assert!(!echo.is_error());

        // Re-exporting to the same name refuses to clobber, as an error echo.
        submit_command(&mut widget, "/export conv");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 3);
        assert!(
            echo.body().starts_with("Failed to export conversation:"),
            "{}",
            echo.body()
        );
        assert!(echo.is_error());

        // Bare /export falls back to the timestamped default filename.
        submit_command(&mut widget, "/export");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 4);
        assert!(
            echo.body().contains("lingxi-transcript-"),
            "{}",
            echo.body()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn slash_copy_echoes_confirmation_and_hands_text_to_the_caller() {
        use crate::history_cell::system::SystemTextCell;
        let mut widget = widget();

        // Empty transcript: byte-locked error, no clipboard handoff.
        assert!(matches!(
            submit_command(&mut widget, "/copy"),
            ChatOutcome::Continue
        ));
        let echo = cell::<SystemTextCell>(&widget, 0);
        assert_eq!(echo.body(), "No assistant message to copy");
        assert!(echo.is_error());

        // Two replies: /copy takes the latest, /copy 2 reaches back.
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::AssistantText {
                    body: "older reply".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "newest".to_string(),
                    timestamp: 0,
                },
            ],
            SessionInfo::default(),
        );
        let outcome = submit_command(&mut widget, "/copy");
        assert!(matches!(outcome, ChatOutcome::CopyToClipboard(ref t) if t == "newest"));
        assert_eq!(
            cell::<SystemTextCell>(&widget, 2).body(),
            "Copied to clipboard (6 characters, 1 lines)"
        );
        let outcome = submit_command(&mut widget, "/copy 2");
        assert!(matches!(outcome, ChatOutcome::CopyToClipboard(ref t) if t == "older reply"));
        // Out-of-range and junk args echo errors without a handoff.
        assert!(matches!(
            submit_command(&mut widget, "/copy 9"),
            ChatOutcome::Continue
        ));
        assert_eq!(
            cell::<SystemTextCell>(&widget, 4).body(),
            "Only 2 assistant messages available to copy"
        );
        assert!(!widget.turn_running(), "/copy starts no turn");
    }

    #[test]
    fn slash_clear_resets_transcript_and_commit_cursor() {
        let mut widget = ChatWidget::new(
            vec![RenderedMessage::SystemText {
                body: "old".to_string(),
                timestamp: 0,
                is_error: false,
            }],
            SessionInfo::default(),
        );
        let mut terminal = test_terminal();
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 1);
        assert!(matches!(
            submit_command(&mut widget, "/clear"),
            ChatOutcome::Continue
        ));
        assert!(widget.transcript().is_empty());
        assert_eq!(widget.transcript().committed_to_terminal(), 0);

        // Criterion 17 (coherence): transcript state AND the native-scrollback
        // commit counter reset TOGETHER — the next flush is a no-op on the
        // emptied transcript, and new content commits cleanly from zero.
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 0);
        widget.transcript.push_message(RenderedMessage::SystemText {
            body: "fresh".to_string(),
            timestamp: 0,
            is_error: false,
        });
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            widget.transcript().committed_to_terminal(),
            1,
            "post-clear content commits from a zeroed cursor"
        );
    }

    /// One string per buffer row.
    fn buffer_rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn render_contract_refreshes_status_and_delegates_cursor_to_the_pane() {
        let mut widget = widget();
        typ(&mut widget, "go");
        widget.handle_key(press(KeyCode::Enter));
        widget.apply_turn_event(TurnEvent::TurnStarted);
        let width = 80;
        // Desired height = live tail (the just-opened empty assistant cell
        // renders its 1-row marker) stacked above the pane's height.
        let tail_height = widget.live_tail_height(width);
        assert_eq!(tail_height, 1, "empty active assistant cell = marker row");
        // `desired_height` uses the FRESHLY computed running flag (Task 5:
        // height is now running-dependent), not `bottom_pane().desired_height`
        // directly — the pane's own flag only updates inside
        // `handle_key`/`render`, and neither has run yet at this point.
        assert_eq!(
            widget.desired_height(width),
            widget.bottom_pane().desired_height_for(width, true) + tail_height
        );
        let area = Rect::new(0, 0, width, widget.desired_height(width).max(4));
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // The render refreshed the pane's task status before drawing: the
        // spinner row (just below the tail) reflects the in-flight turn.
        let status_row = &buffer_rows(&buf)[usize::from(tail_height)];
        assert!(
            status_row.contains("esc to interrupt"),
            "status: {status_row}"
        );
        assert!(
            status_row.contains("Ctrl-C: cancel"),
            "status: {status_row}"
        );
        // Cursor position/style delegate to the pane within its sub-area
        // below the live tail.
        let (_, pane_area) = widget.split_area(area, tail_height);
        assert_eq!(pane_area.y, tail_height);
        assert_eq!(
            widget.cursor_pos(area),
            widget.bottom_pane().cursor_pos(pane_area)
        );
        assert!(widget.cursor_pos(area).is_some());
    }

    #[test]
    fn mid_turn_text_delta_is_visible_in_the_rendered_frame_before_turn_ended() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        let width = 80u16;
        let pane_only = widget.bottom_pane().desired_height(width);
        widget.apply_turn_event(TurnEvent::TextDelta("streamed reply words".to_string()));
        // desired_height accounts for the live tail…
        let height = widget.desired_height(width);
        assert!(
            height > pane_only,
            "tail must grow the widget: {height} <= {pane_only}"
        );
        // …and the mid-turn delta is VISIBLE in the rendered frame — before
        // any TurnEnded — drawn ABOVE the bottom pane (acceptance criterion
        // 12: streaming text stays visible while the turn is active).
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rows = buffer_rows(&buf);
        let text_row = rows
            .iter()
            .position(|row| row.contains("streamed reply words"))
            .unwrap_or_else(|| panic!("streamed text not rendered:\n{}", rows.join("\n")));
        let status_row = rows
            .iter()
            .position(|row| row.contains("esc to interrupt"))
            .expect("running status row rendered");
        assert!(
            text_row < status_row,
            "tail above the pane: text row {text_row}, status row {status_row}"
        );
        // The composer prompt still renders beneath the tail.
        assert!(
            rows.iter().any(|row| row.starts_with('›')),
            "composer visible:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn spinner_text_has_frame_activity_elapsed_seconds_and_interrupt_hint() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        // Default activity: an animation frame glyph, "Working", elapsed
        // seconds, and the esc-to-interrupt hint (claude-code status parity).
        let text = widget.spinner_text();
        let frame = text.chars().next().expect("spinner frame glyph");
        assert!("·✢✳✶✻✽".contains(frame), "unknown frame: {text}");
        assert!(text.contains("Working… ("), "verb + elapsed open: {text}");
        assert!(text.ends_with("s · esc to interrupt)"), "hint: {text}");
        // ToolUseStart swaps the verb for the activity label.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Edit".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.spinner_text().contains("Editing… ("));
    }

    #[test]
    fn activity_label_maps_known_tools_to_gerunds() {
        assert_eq!(activity_label("Bash"), "Running Bash");
        assert_eq!(activity_label("BashOutput"), "Running Bash");
        assert_eq!(activity_label("Read"), "Reading");
        assert_eq!(activity_label("Write"), "Writing");
        assert_eq!(activity_label("Edit"), "Editing");
        assert_eq!(activity_label("MultiEdit"), "Editing");
        assert_eq!(activity_label("Grep"), "Searching");
        assert_eq!(activity_label("Glob"), "Searching");
        assert_eq!(activity_label("WebFetch"), "Browsing");
        assert_eq!(activity_label("WebSearch"), "Browsing");
        assert_eq!(activity_label("Task"), "Delegating");
        assert_eq!(activity_label("SomeMcpTool"), "Running SomeMcpTool");
    }

    #[test]
    fn view_run_command_outcomes_dispatch_to_the_widget() {
        /// A stub view that returns a scripted command on any key.
        struct CommandStub(Option<CommandAction>);

        impl Renderable for CommandStub {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}
            fn desired_height(&self, _width: u16) -> u16 {
                1
            }
        }

        impl BottomPaneView for CommandStub {
            fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
                self.0
                    .take()
                    .map_or(ViewOutcome::Pending, ViewOutcome::RunCommand)
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        // ClearTranscript empties the transcript.
        let mut widget = ChatWidget::new(
            vec![RenderedMessage::SystemText {
                body: "old".to_string(),
                timestamp: 0,
                is_error: false,
            }],
            SessionInfo::default(),
        );
        widget
            .bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::ClearTranscript))));
        assert!(matches!(
            widget.handle_key(press(KeyCode::Enter)),
            ChatOutcome::Continue
        ));
        assert!(widget.transcript().is_empty());
        assert!(
            widget.bottom_pane().view_stack().is_empty(),
            "completed view popped"
        );

        // Quit surfaces as ChatOutcome::Quit.
        widget
            .bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::Quit))));
        assert!(matches!(
            widget.handle_key(press(KeyCode::Enter)),
            ChatOutcome::Quit
        ));
    }

    // ===== Fix round 1: the remaining bridge-emitted TurnEvent variants =====

    /// The rendered widget rows at `width`, sized to the desired height.
    fn rendered_rows(widget: &mut ChatWidget, width: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, widget.desired_height(width).max(4));
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buffer_rows(&buf)
    }

    #[test]
    fn cost_updated_shows_in_the_status_row_idle_and_running() {
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::CostUpdated("$0.0123".to_string()));
        // Idle hints now live in the footer (the LAST row) and carry the dim
        // cost suffix. Width 90 (not 80): the footer's 2-column indent
        // pushes this exact hint+cost combination past 80 columns, clipping
        // the cost suffix — an unrelated width edge case, not what this test
        // checks.
        let rows = rendered_rows(&mut widget, 90);
        let footer = rows.last().expect("footer row");
        assert!(
            footer.contains("Enter: send") && footer.contains("$0.0123"),
            "idle footer row: {footer}"
        );
        // Running spinner row carries it too, and a later event replaces it.
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::CostUpdated("$0.0456".to_string()));
        let rows = rendered_rows(&mut widget, 90);
        let status = rows
            .iter()
            .find(|r| r.contains("esc to interrupt"))
            .expect("running status row");
        assert!(status.contains("$0.0456"), "running status row: {status}");
        assert!(!status.contains("$0.0123"), "stale cost replaced: {status}");
    }

    #[test]
    fn context_pressure_banner_renders_its_own_row_and_clears() {
        let mut widget = widget();
        let width = 80;
        let idle_height = widget.desired_height(width);
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: Some(traits::ContextPressureBanner {
                text: "Context low (12% remaining)".to_string(),
                level: traits::ContextPressureLevel::Warning,
            }),
            used_fraction: 0.88,
        });
        assert_eq!(
            widget.bottom_pane().context_pressure().map(|b| b.level),
            Some(traits::ContextPressureLevel::Warning)
        );
        // The banner adds exactly one pane row. Idle has no leading status
        // row now, so the banner renders FIRST, above the composer.
        assert_eq!(widget.desired_height(width), idle_height + 1);
        let rows = rendered_rows(&mut widget, width);
        assert!(
            rows[0].contains("Context low (12% remaining)"),
            "banner row: {}",
            rows[0]
        );
        assert!(
            rows.iter().any(|r| r.starts_with('›')),
            "composer still visible:\n{}",
            rows.join("\n")
        );
        // `None` clears the banner and its row.
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: None,
            used_fraction: 0.0,
        });
        assert!(widget.bottom_pane().context_pressure().is_none());
        assert_eq!(widget.desired_height(width), idle_height);
    }

    #[test]
    fn context_pressure_feeds_statusline_used_percentage() {
        // The numeric `used_fraction` on a ContextPressure event reaches the
        // statusline payload's `context_window.used_percentage` (× 100).
        let mut widget = widget();
        let slot = crate::status_line::new_slot(
            tui_core::status_line_command::StatusLineConfig::from_settings_value(
                &serde_json::json!({"type": "command", "command": "sl.sh"}),
            ),
        );
        widget.set_status_line(slot.clone());
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: None,
            used_fraction: 0.375,
        });
        let (_, json) = crate::status_line::build_payload(&slot.lock().unwrap()).expect("payload");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // 0.375 fraction → 37.5% used.
        assert!(
            (v["context_window"]["used_percentage"].as_f64().unwrap() - 37.5).abs() < 1e-3,
            "used_percentage: {}",
            v["context_window"]["used_percentage"]
        );
    }

    #[test]
    fn compaction_completed_folds_one_deduped_compact_boundary() {
        use crate::history_cell::system::CompactBoundaryCell;
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 40,
            messages_after: 8,
            bytes_saved: 1024,
        });
        assert_eq!(cells(&widget).len(), 1);
        let _ = cell::<CompactBoundaryCell>(&widget, 0);
        // A duplicate report of the same compaction de-dupes to ONE marker
        // (the old backend's `push_compact_boundary` contract).
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 40,
            messages_after: 8,
            bytes_saved: 1024,
        });
        assert_eq!(cells(&widget).len(), 1, "consecutive boundaries de-dupe");
        // A NON-boundary message in between makes the next boundary render.
        widget.transcript.push_message(RenderedMessage::SystemText {
            body: "between".to_string(),
            timestamp: 0,
            is_error: false,
        });
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 12,
            messages_after: 4,
            bytes_saved: 2048,
        });
        assert_eq!(cells(&widget).len(), 3);
        let _ = cell::<CompactBoundaryCell>(&widget, 2);
    }

    /// A rejected five-hour rate-limit header snapshot.
    fn rate_limit_rejected() -> TurnEvent {
        TurnEvent::RateLimit {
            status: Some("rejected".to_string()),
            rate_limit_type: Some("five_hour".to_string()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        }
    }

    #[test]
    fn rate_limit_composes_a_notice_and_dedupes_identical_text() {
        use crate::history_cell::system::RateLimitCell;
        let mut widget = widget();
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1);
        let notice = cell::<RateLimitCell>(&widget, 0);
        assert_eq!(notice.text(), "You've hit your session limit");
        assert_eq!(notice.upsell(), None, "unknown subscription → no upsell");
        // An identical snapshot composes the same text → suppressed.
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1, "identical notices never stack");
        // `/clear` resets the dedupe slot with the scrollback: the SAME
        // notice can reappear in the now-empty transcript.
        submit_command(&mut widget, "/clear");
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You've hit your session limit"
        );
    }

    #[test]
    fn rate_limit_overage_transition_notice_fires_once_and_rearms_on_leaving() {
        use crate::history_cell::system::RateLimitCell;
        let overage = || TurnEvent::RateLimit {
            status: Some("rejected".to_string()),
            rate_limit_type: Some("five_hour".to_string()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: Some("allowed".to_string()),
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        };
        let mut widget = widget();
        // Entering overage: no composed notice (isUsingOverage + allowed →
        // null), but the ONE-SHOT transition notice fires.
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You're now using extra usage"
        );
        assert!(widget.has_shown_overage_notification);
        // Staying in overage: the flag suppresses a repeat.
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1, "one-shot while in overage");
        // The flag SURVIVES /clear (TS component state) — no repeat after it.
        submit_command(&mut widget, "/clear");
        widget.apply_turn_event(overage());
        assert!(
            widget.has_shown_overage_notification,
            "flag survives /clear"
        );
        assert!(cells(&widget).is_empty(), "no repeat notice after /clear");
        // Leaving overage resets the flag; re-entering fires again.
        widget.apply_turn_event(TurnEvent::RateLimit {
            status: Some("allowed".to_string()),
            rate_limit_type: None,
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        });
        assert!(!widget.has_shown_overage_notification);
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You're now using extra usage"
        );
    }

    #[test]
    fn rate_limit_reads_the_wired_subscription_slot_at_compose_time() {
        use crate::history_cell::system::RateLimitCell;
        let mut widget = widget();
        // A live composition-root slot, filled AFTER wiring (the background
        // fetch landing) — the composer reads it at compose time.
        let slot: traits::subscription::SharedSubscription =
            std::sync::Arc::new(std::sync::RwLock::new(None));
        widget.set_subscription(std::sync::Arc::clone(&slot));
        *slot.write().unwrap() = Some(traits::subscription::SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".to_string()),
            billing_type: Some("stripe_subscription".to_string()),
            ..Default::default()
        });
        widget.apply_turn_event(rate_limit_rejected());
        let notice = cell::<RateLimitCell>(&widget, 0);
        assert_eq!(notice.text(), "You've hit your session limit");
        assert!(
            notice.upsell().is_some(),
            "subscriber snapshot → subscription-granular error upsell"
        );
    }

    #[test]
    fn terminal_sequences_stage_in_order_and_drain_once() {
        let mut widget = widget();
        assert!(widget.take_terminal_sequences().is_empty());
        widget.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]0;title\u{7}".to_string(),
        });
        widget.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]9;notify\u{7}".to_string(),
        });
        assert_eq!(
            widget.take_terminal_sequences(),
            vec![
                "\u{1b}]0;title\u{7}".to_string(),
                "\u{1b}]9;notify\u{7}".to_string()
            ],
            "FIFO order"
        );
        assert!(
            widget.take_terminal_sequences().is_empty(),
            "drain consumes the stage"
        );
        // Staging never touches the transcript.
        assert!(widget.transcript().is_empty());
    }

    #[test]
    fn raw_utilization_and_reserved_permission_variant_are_documented_noops() {
        // RawUtilization's only consumer is the configured statusline
        // *command* input — a surface this backend does not have; the
        // reserved PermissionRequest variant is never emitted by the bridge
        // (live prompts arrive via the permission channel). Both fold without
        // any visible or turn-state change.
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::RawUtilization {
            five_hour_utilization: Some(0.5),
            five_hour_resets_at: Some(1),
            seven_day_utilization: Some(0.2),
            seven_day_resets_at: Some(2),
        });
        widget.apply_turn_event(TurnEvent::PermissionRequest {
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.transcript().is_empty());
        assert!(!widget.turn_running());
        assert!(!widget.has_open_permission());
        assert!(widget.take_terminal_sequences().is_empty());
    }
}
