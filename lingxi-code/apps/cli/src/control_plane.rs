//! Bidirectional control-plane state for the stream-json `--input-format
//! stream-json` path (P5 Phase 2+).
//!
//! Mirrors `StructuredIO.pendingRequests` + `sendRequest`/`processLine` from
//! claude-code's `cli/structuredIO.ts`. Holds:
//!  - the shared single-writer outbound channel (so a CLI-originated
//!    `control_request` never overtakes a queued data frame — the
//!    structuredIO.ts:160-162 design lock),
//!  - the `pendingRequests` map keyed by `request_id` (for the `can_use_tool`
//!    permission round-trip — the ONE subtype the CLI originates),
//!  - the resolved-tool-use-id dedup ring (`MAX_RESOLVED_TOOL_USE_IDS`,
//!    oldest-evicted — duplicate-response drop is wired in Phase 4),
//!  - the active turn's `CancellationToken`, so a `deny+interrupt` permission
//!    response can abort the whole turn (§3.4).
//!
//! The plane is created in the CLI BEFORE `build_runtime` (its outbound handle
//! comes from `StreamJsonStream::outbound_tx()`, available pre-build) and shared
//! with the injected [`StdioControlPermissionGate`] and the run loop's resolver.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use permission::gate::{
    PermissionCheckContext, PermissionDecision, PermissionGate, PermissionOutcome, PromptWorker,
};
use permission::{
    persist_permission_update, PermissionBehavior, PermissionPaths, PermissionRule,
    PermissionRuleSource, PermissionRuleValue, PermissionUpdate, PermissionUpdateDestination,
};

use crate::stream_json::{serialize_ndjson_line, OutboundTx};

/// Cap on the resolved-tool-use dedup ring (claude-code
/// `MAX_RESOLVED_TOOL_USE_IDS`, oldest-evicted).
const MAX_RESOLVED_TOOL_USE_IDS: usize = 1000;

/// A registered outbound `control_request` awaiting its `control_response`.
struct PendingControlRequest {
    /// Resolved by [`StdioControlPlane::resolve_response`] with the inner
    /// `response.response` payload (`Ok`) or the `response.error` string (`Err`).
    responder: oneshot::Sender<Result<Value, String>>,
    /// The `tool_use_id` this request carried, tracked on resolve so a late /
    /// duplicate response can be deduped (§1.5).
    tool_use_id: Option<String>,
}

/// Shared bidirectional control-plane state (see module docs).
pub struct StdioControlPlane {
    /// The single-writer outbound NDJSON channel shared with `StreamJsonStream`
    /// (every producer enqueues here; one drain task is the sole stdout writer).
    outbound_tx: Arc<OutboundTx>,
    /// CLI-originated requests awaiting a response, keyed by `request_id`.
    pending: Mutex<HashMap<String, PendingControlRequest>>,
    /// Resolved tool_use_ids, for duplicate-response dedup (Phase 4).
    resolved_tool_use_ids: Mutex<VecDeque<String>>,
    /// The in-flight turn's cancellation token (for `deny+interrupt`).
    active_turn_cancel: Mutex<Option<CancellationToken>>,
}

impl StdioControlPlane {
    /// Build the plane over the stream's shared outbound channel.
    #[must_use]
    pub fn new(outbound_tx: Arc<OutboundTx>) -> Arc<Self> {
        Arc::new(Self {
            outbound_tx,
            pending: Mutex::new(HashMap::new()),
            resolved_tool_use_ids: Mutex::new(VecDeque::new()),
            active_turn_cancel: Mutex::new(None),
        })
    }

    /// Register the current turn's cancellation token so a `deny+interrupt`
    /// permission response can abort the whole turn (§3.4). Called by the turn
    /// loop before each turn.
    pub async fn set_active_turn(&self, token: CancellationToken) {
        *self.active_turn_cancel.lock().await = Some(token);
    }

    /// Clear the active-turn token between turns.
    pub async fn clear_active_turn(&self) {
        *self.active_turn_cancel.lock().await = None;
    }

    /// Cancel the active turn (the `deny+interrupt` path).
    async fn cancel_active_turn(&self) {
        if let Some(tok) = self.active_turn_cancel.lock().await.as_ref() {
            tok.cancel();
        }
    }

    /// A clone of the active turn's cancellation token, if any. The gate selects
    /// on it so a turn abort (interrupt / Ctrl-C) cancels an in-flight
    /// `can_use_tool` request rather than blocking forever.
    async fn active_turn_token(&self) -> Option<CancellationToken> {
        self.active_turn_cancel.lock().await.clone()
    }

    /// Abort an outbound `control_request` whose turn was cancelled / superseded
    /// (§1.4 outbound-emit): enqueue `control_cancel_request` to tell the host to
    /// drop its prompt, remove the pending entry locally (tracking its tool_use_id
    /// so a late response is deduped), and let the awaiting future fall through.
    async fn cancel_request(&self, request_id: &str) {
        let entry = {
            let mut pending = self.pending.lock().await;
            pending.remove(request_id)
        };
        if let Some(tuid) = entry.and_then(|e| e.tool_use_id) {
            self.track_resolved(tuid).await;
        }
        let frame = json!({
            "type": "control_cancel_request",
            "request_id": request_id,
        });
        let _ = self.outbound_tx.send(serialize_ndjson_line(&frame));
    }

    /// Emit a CLI-originated `control_request` and return a receiver resolved
    /// when the matching `control_response` arrives (`sendRequest`,
    /// structuredIO.ts:469). Mints a `request_id`, enqueues the frame on the
    /// shared outbound channel (FIFO — no overtaking data frames), and registers
    /// the pending entry.
    pub async fn send_request(
        &self,
        request: Value,
        tool_use_id: Option<String>,
    ) -> (String, oneshot::Receiver<Result<Value, String>>) {
        let request_id = Uuid::new_v4().to_string();
        let frame = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        });
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(
                request_id.clone(),
                PendingControlRequest {
                    responder: tx,
                    tool_use_id,
                },
            );
        }
        // Enqueue AFTER registering so a (theoretically) instant response can't
        // race the insert. The drain task serializes to stdout.
        let _ = self.outbound_tx.send(serialize_ndjson_line(&frame));
        (request_id, rx)
    }

    /// Resolve an inbound `control_response` against `pendingRequests`
    /// (`processLine`, structuredIO.ts:362). Frame keys are already
    /// snake_case-normalized by the router (`normalize_control_message_keys`),
    /// so the join key is the inner `response.request_id` and the payload is the
    /// inner `response.response` (the load-bearing double nesting).
    pub async fn resolve_response(&self, frame: &Value) {
        let Some(response) = frame.get("response") else {
            return;
        };
        let request_id = response
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let entry = {
            let mut pending = self.pending.lock().await;
            pending.remove(&request_id)
        };
        let Some(entry) = entry else {
            // Orphan / duplicate `control_response`: no matching pending entry.
            // §1.5 dedup: if the payload's `toolUseID` was already resolved (a
            // websocket-reconnect double-delivery), log + drop so it can't
            // double-resolve a tool_use (which would 400 on a non-unique id).
            let tool_use_id = response
                .get("response")
                .and_then(|p| p.get("toolUseID"))
                .and_then(Value::as_str);
            if let Some(tuid) = tool_use_id {
                if self.is_resolved(tuid).await {
                    tracing::debug!(
                        "Ignoring duplicate control_response for already-resolved toolUseID={} request_id={}",
                        tuid,
                        request_id
                    );
                }
            }
            return;
        };
        if let Some(tuid) = entry.tool_use_id.clone() {
            self.track_resolved(tuid).await;
        }
        let subtype = response.get("subtype").and_then(Value::as_str).unwrap_or("");
        let result = if subtype == "error" {
            Err(response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("control_response error")
                .to_string())
        } else {
            // SUCCESS: payload at `response.response`; absent ⇒ `{}` (the
            // `rn(gt)` no-payload case omits the key, parsed as empty).
            Ok(response.get("response").cloned().unwrap_or_else(|| json!({})))
        };
        let _ = entry.responder.send(result);
    }

    /// Mark a tool_use_id resolved (oldest-evicted ring, cap
    /// `MAX_RESOLVED_TOOL_USE_IDS`). The duplicate-drop check that consults this
    /// ring lands in Phase 4.
    async fn track_resolved(&self, tool_use_id: String) {
        let mut ring = self.resolved_tool_use_ids.lock().await;
        if ring.iter().any(|id| id == &tool_use_id) {
            return;
        }
        ring.push_back(tool_use_id);
        while ring.len() > MAX_RESOLVED_TOOL_USE_IDS {
            ring.pop_front();
        }
    }

    /// Whether `tool_use_id` is in the resolved ring (§1.5 dedup check).
    async fn is_resolved(&self, tool_use_id: &str) -> bool {
        self.resolved_tool_use_ids
            .lock()
            .await
            .iter()
            .any(|id| id == tool_use_id)
    }

    /// Reject EVERY in-flight pending `control_request` with `reason`.
    ///
    /// Called when stdin reaches EOF (the host closed the input stream): without
    /// this, a gate awaiting a `can_use_tool` `control_response` would block
    /// forever, hanging the tool call / turn. Mirrors claude-code's
    /// `StructuredIO.read()` close path, which sets `inputClosed` and rejects
    /// every `pendingRequests` entry with *"Tool permission stream closed before
    /// response received"*. Each drained entry's `tool_use_id` is tracked as
    /// resolved so a late re-delivery is still deduped.
    pub async fn fail_all_pending(&self, reason: &str) {
        let drained: Vec<PendingControlRequest> = {
            let mut pending = self.pending.lock().await;
            pending.drain().map(|(_, e)| e).collect()
        };
        for entry in drained {
            if let Some(tuid) = entry.tool_use_id.clone() {
                self.track_resolved(tuid).await;
            }
            let _ = entry.responder.send(Err(reason.to_string()));
        }
    }
}

/// Inner-transport `PermissionGate` that resolves an unresolved `Ask` by
/// emitting a `can_use_tool` control_request and awaiting the host's
/// `control_response` (§3).
///
/// Sits BEHIND `PolicyPermissionGate` (the outer local pre-check), substituted
/// via `cfg.injected_permission_gate`. Mirrors `StructuredIO.createCanUseTool`,
/// but the local allow/deny pre-gate already ran in the outer policy gate, so
/// only an `ask` reaches here. Deferred (per spec §3.5): the local
/// PermissionRequest-hook race, the `updatedInput` rewrite, and the sandbox-ask
/// piggyback.
pub struct StdioControlPermissionGate {
    plane: Arc<StdioControlPlane>,
    /// Settings-file roots used to PERSIST the host's `updatedPermissions` rule
    /// updates carried by a `can_use_tool` ALLOW response (claude-code
    /// `persistPermissionUpdates`). `None` ⇒ persistence is skipped (the gate
    /// still maps the allow; only the settings write is suppressed — e.g. in
    /// unit tests with no real settings tree). Wired via [`Self::with_persist`]
    /// from the CLI config (`cfg.claude_home` / `cfg.cwd`).
    persist_paths: Option<PermissionPaths>,
}

impl StdioControlPermissionGate {
    /// Build the decider over the shared control plane.
    #[must_use]
    pub fn new(plane: Arc<StdioControlPlane>) -> Self {
        Self {
            plane,
            persist_paths: None,
        }
    }

    /// Attach the settings-file roots so an ALLOW response's `updatedPermissions`
    /// rule updates are PERSISTED (mirrors `TuiPermissionGate::with_persist`):
    /// `claude_home` resolves `userSettings`, `cwd` resolves
    /// project/local settings. Without this the host's rule updates are parsed +
    /// dropped (no settings write).
    #[must_use]
    pub fn with_persist(mut self, paths: PermissionPaths) -> Self {
        self.persist_paths = Some(paths);
        self
    }

    /// Collapse a [`PermissionOutcome`] to the 2-valued [`PermissionDecision`]
    /// (dropping any `updated_input`) for the `check`/`check_with_worker` callers
    /// that cannot apply a rewrite.
    fn outcome_to_decision(outcome: PermissionOutcome) -> PermissionDecision {
        match outcome {
            PermissionOutcome::Allow { .. } => PermissionDecision::Allow,
            PermissionOutcome::Deny { reason } => PermissionDecision::Deny { reason },
        }
    }

    /// Run the `can_use_tool` round-trip and map the host's decision to a
    /// [`PermissionOutcome`] (carrying any `updatedInput` rewrite).
    async fn decide_outcome(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        // §3.2 outbound request: emit `tool_name, input, tool_use_id` (+ agent_id
        // when a worker is present, + decision_reason / permission_suggestions /
        // blocked_path when supplied — claude-code `createCanUseTool` sends
        // `decision_reason: serializeDecisionReason(...)`,
        // `permission_suggestions: mainPermissionResult.suggestions`,
        // `blocked_path: mainPermissionResult.blockedPath`). Each is OMITTED when
        // absent (the oracle's `.optional()` fields). Use the REAL assistant
        // tool_use id when the dispatcher provides it (so the host can correlate +
        // dedup), else mint one.
        let tool_use_id = ctx
            .tool_use_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut request = json!({
            "subtype": "can_use_tool",
            "tool_name": name,
            "input": input,
            "tool_use_id": tool_use_id,
        });
        if let Some(w) = &ctx.worker {
            request["agent_id"] = json!(w.name);
        }
        if let Some(reason) = &ctx.decision_reason {
            request["decision_reason"] = json!(reason);
        }
        // permission_suggestions / blocked_path: forwarded only when the policy
        // Ask supplies them. PARTIAL (stream-json P5 finding #9): LingXi's policy
        // `PermissionResult::Ask` does not model claude-code's
        // `PermissionAskDecision.suggestions` / `.blockedPath` (the per-tool
        // ask-suggestion builders — e.g. `ruleSuggestionsForCommand` — and the
        // path-block reason are unported), so `ctx.permission_suggestions` /
        // `ctx.blocked_path` are always `None` today and these keys are OMITTED,
        // byte-faithful to the oracle's `.optional()` shape. Emitting them is wired
        // here so the request becomes byte-complete the moment those builders land
        // on the policy Ask, with NO further control-plane change.
        if let Some(suggestions) = &ctx.permission_suggestions {
            request["permission_suggestions"] = suggestions.clone();
        }
        if let Some(path) = &ctx.blocked_path {
            request["blocked_path"] = json!(path);
        }
        let (request_id, rx) = self.plane.send_request(request, Some(tool_use_id)).await;

        // There is NO timeout on the can_use_tool request (§3.1): block until a
        // control_response arrives, the channel drops, or the turn is aborted.
        // On a turn abort (interrupt / Ctrl-C), emit `control_cancel_request` so
        // the host drops its prompt, and deny (§1.4 / §3.1 AbortError → deny).
        let result = match self.plane.active_turn_token().await {
            Some(token) => {
                tokio::select! {
                    r = rx => r,
                    () = token.cancelled() => {
                        self.plane.cancel_request(&request_id).await;
                        return PermissionOutcome::Deny {
                            reason: "Tool permission request failed: aborted".to_string(),
                        };
                    }
                }
            }
            None => rx.await,
        };
        match result {
            Ok(Ok(payload)) => {
                let outcome = self.map_payload(payload).await;
                // §2b: when the allow carried `updatedPermissions`, parse + persist
                // them here (claude-code `persistPermissionUpdates`, fire-and-forget
                // — a persist error never turns the allow into a deny). The in-memory
                // `applyPermissionUpdates` is a documented PARTIAL (see below).
                if let PermissionOutcome::Allow {
                    permission_updates, ..
                } = &outcome
                {
                    if !permission_updates.is_empty() {
                        self.persist_permission_updates(permission_updates).await;
                    }
                }
                outcome
            }
            Ok(Err(err)) => PermissionOutcome::Deny {
                reason: format!("Tool permission request failed: {err}"),
            },
            Err(_) => PermissionOutcome::Deny {
                reason: "Tool permission request failed: control channel closed".to_string(),
            },
        }
    }

    /// Parse + PERSIST the host's `updatedPermissions` rule updates from a
    /// `can_use_tool` ALLOW response (claude-code `persistPermissionUpdates`).
    ///
    /// Best-effort and fire-and-forget — 1:1 with the oracle, where
    /// `persistPermissionUpdates` is called without awaiting / surfacing errors:
    /// a parse skip or a [`permission::PersistError`] is logged and the allow
    /// still stands. Only the `addRules` update type is persisted (the single
    /// shape LingXi's [`PermissionUpdate`] models, per `persist.rs`); a
    /// `replaceRules` / `removeRules` / `setMode` / `add|removeDirectories` entry
    /// — or a malformed one — is skipped with a debug log, never an error
    /// (mirroring the oracle's tolerant `.catch(undefined)`).
    ///
    /// IN-MEMORY APPLY is a documented PARTIAL: claude-code's
    /// `applyPermissionUpdates` mutates the live `toolPermissionContext` so the
    /// SAME session's subsequent calls auto-allow. LingXi's enforcing
    /// `PolicyPermissionGate` wraps an IMMUTABLE `Arc<PermissionPolicy>` (rule
    /// buckets are baked at `from_rules`; only `mode_override` is interior-mutable),
    /// so there is no runtime rule-injection seam reachable from this gate. The
    /// PERSISTED rules take effect on the NEXT session load. Reaching live-apply
    /// would require adding a session-rule `RwLock<Vec<PermissionRule>>` to
    /// `PolicyPermissionGate` consulted in `effective_authorize` — a larger change
    /// deferred with this exact reason.
    async fn persist_permission_updates(&self, updates: &[Value]) {
        let Some(paths) = &self.persist_paths else {
            // No settings tree wired (e.g. unit tests) — parse-only, no write.
            return;
        };
        for raw in updates {
            for update in parse_add_rules_update(raw) {
                match persist_permission_update(&update, paths).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(
                            "Failed to persist permission update from can_use_tool response: {e}"
                        );
                    }
                }
            }
        }
    }

    /// Map a `PermissionToolOutput` payload onto a [`PermissionOutcome`] (§3.3).
    async fn map_payload(&self, payload: Value) -> PermissionOutcome {
        match payload.get("behavior").and_then(Value::as_str) {
            Some("allow") => {
                // §3.3 / strict schema: an allow result REQUIRES an `updatedInput`
                // object key (claude-code `PermissionAllowResultSchema`); a missing
                // (or non-object) key is a malformed allow → deny. The value may be
                // `{}` (no rewrite); only a NON-EMPTY object substitutes the tool
                // input (claude-code applies `updatedInput` "when it has keys").
                match payload.get("updatedInput") {
                    Some(Value::Object(map)) => {
                        let updated_input = if map.is_empty() {
                            None
                        } else {
                            Some(Value::Object(map.clone()))
                        };
                        // §2b / `applyPermissionUpdates` + `persistPermissionUpdates`:
                        // an allow may also carry an `updatedPermissions` array of
                        // permission-rule updates the host wants applied + persisted.
                        // It is OPTIONAL and independent of `updatedInput`: a missing
                        // or non-array value is simply no updates (mirroring the
                        // oracle's `.catch(undefined)` — a malformed array is IGNORED,
                        // never a deny). The raw wire array is carried through and
                        // parsed + persisted by the consumer ([`Self::decide_outcome`]).
                        let permission_updates = match payload.get("updatedPermissions") {
                            Some(Value::Array(arr)) => arr.clone(),
                            _ => Vec::new(),
                        };
                        PermissionOutcome::Allow {
                            updated_input,
                            permission_updates,
                        }
                    }
                    _ => PermissionOutcome::Deny {
                        reason: "Tool permission request failed: malformed allow result (missing updatedInput)"
                            .to_string(),
                    },
                }
            }
            Some("deny") => {
                let message = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Permission denied")
                    .to_string();
                if payload
                    .get("interrupt")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    // deny+interrupt: abort the whole turn, not just this tool
                    // (§3.4 — the binary calls `ctx.abortController.abort()`).
                    self.plane.cancel_active_turn().await;
                }
                PermissionOutcome::Deny { reason: message }
            }
            _ => PermissionOutcome::Deny {
                reason: "Tool permission request returned an unknown behavior".to_string(),
            },
        }
    }
}

/// Parse one raw `updatedPermissions` wire entry (a `permissionUpdateSchema`
/// discriminated union, keyed by `type`) into zero or more [`PermissionUpdate`]s.
///
/// Only the `addRules` variant is modeled — `{type:"addRules", rules:[{toolName,
/// ruleContent?}], behavior, destination}` — because that is the single shape
/// LingXi's [`PermissionUpdate`] persists (`persist.rs` notes `replaceRules` /
/// `removeRules` / `setMode` / directory updates are unmodeled). Each element of
/// `rules` becomes one [`PermissionUpdate`] (claude-code stores rules per behavior
/// + destination). A non-`addRules` type, a missing/ill-typed field, or an
/// unknown behavior/destination yields an EMPTY vec (the caller skips it with a
/// debug log) — never an error, mirroring the oracle's tolerant validation.
fn parse_add_rules_update(raw: &Value) -> Vec<PermissionUpdate> {
    let Some(obj) = raw.as_object() else {
        return Vec::new();
    };
    if obj.get("type").and_then(Value::as_str) != Some("addRules") {
        tracing::debug!(
            "Skipping non-addRules permission update from can_use_tool response: type={:?}",
            obj.get("type")
        );
        return Vec::new();
    }
    let Some(behavior) = obj
        .get("behavior")
        .and_then(Value::as_str)
        .and_then(parse_behavior)
    else {
        tracing::debug!("Skipping addRules update with missing/unknown behavior");
        return Vec::new();
    };
    let Some(destination) = obj
        .get("destination")
        .and_then(Value::as_str)
        .and_then(parse_destination)
    else {
        tracing::debug!("Skipping addRules update with missing/unknown destination");
        return Vec::new();
    };
    // The settings-file rule SOURCE mirrors the destination (claude-code stores a
    // persisted rule under the file its destination resolves to). Non-persistable
    // destinations (session/cliArg) map to their nearest source; `persist` no-ops
    // them anyway (`destination_path` returns `None`).
    let source = source_for_destination(destination);
    let Some(rules) = obj.get("rules").and_then(Value::as_array) else {
        tracing::debug!("Skipping addRules update with missing rules array");
        return Vec::new();
    };
    rules
        .iter()
        .filter_map(|rule_value| {
            let tool_name = rule_value.get("toolName").and_then(Value::as_str)?;
            // `ruleContent` is optional; a tool-wide rule omits it.
            let rule_content = rule_value
                .get("ruleContent")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(PermissionUpdate {
                rule: PermissionRule {
                    value: PermissionRuleValue {
                        tool_name: permission::rule::normalize_legacy_tool_name(tool_name),
                        rule_content,
                    },
                    behavior,
                    source,
                },
                destination,
            })
        })
        .collect()
}

/// Map the wire behavior string (`allow`/`deny`/`ask`) to a [`PermissionBehavior`].
fn parse_behavior(s: &str) -> Option<PermissionBehavior> {
    match s {
        "allow" => Some(PermissionBehavior::Allow),
        "deny" => Some(PermissionBehavior::Deny),
        "ask" => Some(PermissionBehavior::Ask),
        _ => None,
    }
}

/// Map the wire destination string to a [`PermissionUpdateDestination`]
/// (`permissionUpdateDestinationSchema`).
fn parse_destination(s: &str) -> Option<PermissionUpdateDestination> {
    match s {
        "userSettings" => Some(PermissionUpdateDestination::UserSettings),
        "projectSettings" => Some(PermissionUpdateDestination::ProjectSettings),
        "localSettings" => Some(PermissionUpdateDestination::LocalSettings),
        "session" => Some(PermissionUpdateDestination::Session),
        "cliArg" => Some(PermissionUpdateDestination::CliArg),
        _ => None,
    }
}

/// The [`PermissionRuleSource`] a rule persisted to `destination` should carry
/// (the file it lives in determines its precedence on the next load).
fn source_for_destination(dest: PermissionUpdateDestination) -> PermissionRuleSource {
    match dest {
        PermissionUpdateDestination::UserSettings => PermissionRuleSource::UserSettings,
        PermissionUpdateDestination::ProjectSettings => PermissionRuleSource::ProjectSettings,
        PermissionUpdateDestination::LocalSettings => PermissionRuleSource::LocalSettings,
        PermissionUpdateDestination::CliArg => PermissionRuleSource::CliArg,
        PermissionUpdateDestination::Session => PermissionRuleSource::Session,
    }
}

#[async_trait]
impl PermissionGate for StdioControlPermissionGate {
    async fn check(&self, name: &str, input: &Value) -> PermissionDecision {
        Self::outcome_to_decision(
            self.decide_outcome(name, input, &PermissionCheckContext::default())
                .await,
        )
    }

    async fn check_with_worker(
        &self,
        name: &str,
        input: &Value,
        worker: Option<PromptWorker>,
    ) -> PermissionDecision {
        let ctx = PermissionCheckContext {
            worker,
            ..PermissionCheckContext::default()
        };
        Self::outcome_to_decision(self.decide_outcome(name, input, &ctx).await)
    }

    async fn check_with_context(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        self.decide_outcome(name, input, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Build a plane wired to an in-memory outbound channel; the receiver lets a
    /// test read the frames the plane emits (simulating the host on stdin).
    fn plane_with_channel() -> (Arc<StdioControlPlane>, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        (StdioControlPlane::new(Arc::new(tx)), rx)
    }

    fn success_response(request_id: &str, payload: Value) -> Value {
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": payload,
            }
        })
    }

    #[tokio::test]
    async fn send_request_emits_frame_and_resolves_success() {
        let (plane, mut rx) = plane_with_channel();
        let (req_id, fut) = plane
            .send_request(
                json!({"subtype": "can_use_tool", "tool_name": "Bash"}),
                Some("tu1".to_string()),
            )
            .await;

        let line = rx.recv().await.expect("frame emitted");
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["type"], "control_request");
        assert_eq!(frame["request"]["tool_name"], "Bash");
        assert_eq!(frame["request_id"].as_str().unwrap(), req_id);

        plane
            .resolve_response(&success_response(&req_id, json!({"behavior": "allow"})))
            .await;
        let payload = fut.await.unwrap().unwrap();
        assert_eq!(payload["behavior"], "allow");
    }

    #[tokio::test]
    async fn resolve_response_error_subtype_rejects() {
        let (plane, _rx) = plane_with_channel();
        let (req_id, fut) = plane
            .send_request(json!({"subtype": "can_use_tool"}), None)
            .await;

        let resp = json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": req_id, "error": "boom"}
        });
        plane.resolve_response(&resp).await;
        assert_eq!(fut.await.unwrap().unwrap_err(), "boom");
    }

    #[tokio::test]
    async fn success_with_no_inner_payload_resolves_empty_object() {
        let (plane, _rx) = plane_with_channel();
        let (req_id, fut) = plane.send_request(json!({"subtype": "x"}), None).await;
        // `response` key absent (the `rn(gt)` no-payload case).
        let resp = json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": req_id}
        });
        plane.resolve_response(&resp).await;
        assert_eq!(fut.await.unwrap().unwrap(), json!({}));
    }

    #[tokio::test]
    async fn turn_abort_emits_control_cancel_request_and_denies() {
        // §1.4 / §3.1: a turn abort while the gate awaits cancels the outbound
        // request and denies.
        let (plane, mut rx) = plane_with_channel();
        let token = CancellationToken::new();
        plane.set_active_turn(token.clone()).await;
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });

        // Drain the can_use_tool request, then abort the turn.
        let line = rx.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["request"]["subtype"],
            "can_use_tool"
        );
        token.cancel();

        assert_eq!(
            check.await.unwrap(),
            PermissionDecision::Deny {
                reason: "Tool permission request failed: aborted".to_string()
            }
        );
        // A control_cancel_request frame is emitted to the host.
        let cancel_line = rx.recv().await.expect("cancel frame emitted");
        let cancel: Value = serde_json::from_str(&cancel_line).unwrap();
        assert_eq!(cancel["type"], "control_cancel_request");
        assert!(cancel["request_id"].is_string());
    }

    #[tokio::test]
    async fn duplicate_control_response_for_resolved_tool_use_is_dropped() {
        // §1.5: resolve once (tracking the tool_use_id), then a duplicate
        // response (orphan, same toolUseID) is dropped without re-resolving.
        let (plane, _rx) = plane_with_channel();
        let (req_id, fut) = plane
            .send_request(json!({"subtype": "can_use_tool"}), Some("tu-dup".to_string()))
            .await;
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "toolUseID": "tu-dup"}),
            ))
            .await;
        assert_eq!(fut.await.unwrap().unwrap()["behavior"], "allow");

        // The duplicate has a NEW request_id (orphan) but the same toolUseID.
        let dup = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "late-redelivery",
                "response": {"behavior": "allow", "toolUseID": "tu-dup"}
            }
        });
        // Must not panic / double-resolve; the tool_use_id is in the resolved ring.
        plane.resolve_response(&dup).await;
        assert!(plane.is_resolved("tu-dup").await);
    }

    #[tokio::test]
    async fn fail_all_pending_rejects_in_flight_requests() {
        // §EOF: stdin close must reject every pending control_request so an
        // awaiting gate does not hang forever.
        let (plane, _rx) = plane_with_channel();
        let (_id1, fut1) = plane
            .send_request(json!({"subtype": "can_use_tool"}), Some("a".into()))
            .await;
        let (_id2, fut2) = plane
            .send_request(json!({"subtype": "can_use_tool"}), Some("b".into()))
            .await;
        plane
            .fail_all_pending("Tool permission stream closed before response received")
            .await;
        assert_eq!(
            fut1.await.unwrap().unwrap_err(),
            "Tool permission stream closed before response received"
        );
        assert_eq!(
            fut2.await.unwrap().unwrap_err(),
            "Tool permission stream closed before response received"
        );
        // Drained tool_use_ids are tracked so a late re-delivery is still deduped.
        assert!(plane.is_resolved("a").await);
        assert!(plane.is_resolved("b").await);
    }

    #[tokio::test]
    async fn gate_hang_is_broken_by_fail_all_pending() {
        // The end-to-end #3 scenario: a gate awaiting can_use_tool is unblocked
        // (denied) when fail_all_pending fires on EOF.
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });
        // Drain the emitted can_use_tool request, then simulate stdin EOF.
        let _ = rx.recv().await.unwrap();
        plane
            .fail_all_pending("Tool permission stream closed before response received")
            .await;
        match check.await.unwrap() {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("Tool permission stream closed before response received"));
            }
            other => panic!("expected Deny on EOF, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn orphan_response_is_dropped_without_panic() {
        let (plane, _rx) = plane_with_channel();
        plane
            .resolve_response(&success_response("does-not-exist", json!({})))
            .await;
        // No pending entry to resolve — silently ignored.
    }

    #[tokio::test]
    async fn gate_allow_maps_to_allow() {
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({"command": "ls"});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });

        let line = rx.recv().await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["request"]["subtype"], "can_use_tool");
        assert_eq!(frame["request"]["tool_name"], "Bash");
        assert!(frame["request"]["tool_use_id"].is_string());
        let req_id = frame["request_id"].as_str().unwrap().to_string();

        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {}}),
            ))
            .await;
        assert_eq!(check.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn gate_deny_maps_to_deny_with_message() {
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Write", &input).await });

        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "deny", "message": "not allowed"}),
            ))
            .await;
        assert_eq!(
            check.await.unwrap(),
            PermissionDecision::Deny {
                reason: "not allowed".to_string()
            }
        );
    }

    #[tokio::test]
    async fn gate_deny_interrupt_cancels_active_turn() {
        let (plane, mut rx) = plane_with_channel();
        let token = CancellationToken::new();
        plane.set_active_turn(token.clone()).await;
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });

        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "deny", "message": "stop", "interrupt": true}),
            ))
            .await;
        assert_eq!(
            check.await.unwrap(),
            PermissionDecision::Deny {
                reason: "stop".to_string()
            }
        );
        assert!(token.is_cancelled(), "deny+interrupt must cancel the turn");
    }

    #[tokio::test]
    async fn gate_error_response_maps_to_deny() {
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });

        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": req_id, "error": "host failure"}
        });
        plane.resolve_response(&resp).await;
        assert_eq!(
            check.await.unwrap(),
            PermissionDecision::Deny {
                reason: "Tool permission request failed: host failure".to_string()
            }
        );
    }

    #[tokio::test]
    async fn gate_check_with_worker_sets_agent_id() {
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let worker = PromptWorker {
            name: "researcher".to_string(),
            team: None,
            is_async: false,
        };
        let check =
            tokio::spawn(async move { gate.check_with_worker("Bash", &input, Some(worker)).await });

        let line = rx.recv().await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["request"]["agent_id"], "researcher");
        let req_id = frame["request_id"].as_str().unwrap().to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {}}),
            ))
            .await;
        assert_eq!(check.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn context_uses_real_tool_use_id_and_returns_updated_input() {
        // #10: the request carries the dispatcher-supplied tool_use_id (not a
        // minted uuid). #2a: a non-empty updatedInput flows back as the rewrite.
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({"command": "ls"});
        let ctx = PermissionCheckContext {
            tool_use_id: Some("toolu_real_42".to_string()),
            decision_reason: Some("needs review".to_string()),
            ..PermissionCheckContext::default()
        };
        let check =
            tokio::spawn(async move { gate.check_with_context("Bash", &input, &ctx).await });

        let line = rx.recv().await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["request"]["tool_use_id"], "toolu_real_42", "real id used");
        assert_eq!(frame["request"]["decision_reason"], "needs review");
        let req_id = frame["request_id"].as_str().unwrap().to_string();

        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {"command": "ls -la"}}),
            ))
            .await;
        match check.await.unwrap() {
            PermissionOutcome::Allow { updated_input, .. } => {
                assert_eq!(updated_input, Some(json!({"command": "ls -la"})));
            }
            other => panic!("expected Allow with updated_input, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn context_emits_suggestions_and_blocked_path_when_present() {
        // #9: when the policy Ask supplies permission_suggestions / blocked_path,
        // the can_use_tool request carries them (claude-code
        // `permission_suggestions` / `blocked_path`). The seam is forward-compatible
        // even though the policy gate does not populate them yet (always None).
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let ctx = PermissionCheckContext {
            tool_use_id: Some("toolu_9".to_string()),
            permission_suggestions: Some(json!([
                {"type": "addRules", "rules": ["Bash(ls *)"], "behavior": "allow", "destination": "session"}
            ])),
            blocked_path: Some("/etc/secret".to_string()),
            ..PermissionCheckContext::default()
        };
        let check =
            tokio::spawn(async move { gate.check_with_context("Bash", &input, &ctx).await });

        let line = rx.recv().await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            frame["request"]["permission_suggestions"][0]["type"],
            "addRules"
        );
        assert_eq!(frame["request"]["blocked_path"], "/etc/secret");
        let req_id = frame["request_id"].as_str().unwrap().to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {}}),
            ))
            .await;
        assert!(matches!(
            check.await.unwrap(),
            PermissionOutcome::Allow { .. }
        ));
    }

    #[tokio::test]
    async fn context_omits_suggestions_and_blocked_path_when_absent() {
        // The oracle's permission_suggestions / blocked_path are `.optional()`; with
        // nothing in the ctx (the common ask) the keys must be ABSENT, not null.
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });

        let line = rx.recv().await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert!(
            frame["request"].get("permission_suggestions").is_none(),
            "absent suggestions ⇒ key omitted"
        );
        assert!(
            frame["request"].get("blocked_path").is_none(),
            "absent blocked_path ⇒ key omitted"
        );
        assert!(
            frame["request"].get("decision_reason").is_none(),
            "no decision reason ⇒ key omitted"
        );
        let req_id = frame["request_id"].as_str().unwrap().to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {}}),
            ))
            .await;
        assert_eq!(check.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn allow_without_updated_input_is_denied() {
        // #7: the strict schema requires an `updatedInput` key on an allow; a
        // bare allow is malformed → deny.
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });
        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(&req_id, json!({"behavior": "allow"})))
            .await;
        match check.await.unwrap() {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("malformed allow result"), "got {reason}");
            }
            other => panic!("expected Deny for malformed allow, got {other:?}"),
        }
    }

    // ── §2b: updatedPermissions parse + persist ──────────────────────────────

    #[test]
    fn parse_add_rules_update_models_each_rule() {
        // The host's addRules wire entry → one PermissionUpdate per rule, carrying
        // the parsed behavior + destination and the destination-derived source.
        let raw = json!({
            "type": "addRules",
            "rules": [
                { "toolName": "Bash", "ruleContent": "npm install" },
                { "toolName": "Read" }, // tool-wide (no ruleContent)
            ],
            "behavior": "allow",
            "destination": "localSettings",
        });
        let updates = parse_add_rules_update(&raw);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].rule.value.tool_name, "Bash");
        assert_eq!(updates[0].rule.value.rule_content.as_deref(), Some("npm install"));
        assert_eq!(updates[0].rule.behavior, PermissionBehavior::Allow);
        assert_eq!(updates[0].destination, PermissionUpdateDestination::LocalSettings);
        assert_eq!(updates[0].rule.source, PermissionRuleSource::LocalSettings);
        // Tool-wide rule keeps ruleContent None.
        assert_eq!(updates[1].rule.value.tool_name, "Read");
        assert!(updates[1].rule.value.rule_content.is_none());
    }

    #[test]
    fn parse_add_rules_update_normalizes_legacy_tool_name() {
        let raw = json!({
            "type": "addRules",
            "rules": [{ "toolName": "Task", "ruleContent": "general-purpose" }],
            "behavior": "deny",
            "destination": "userSettings",
        });
        let updates = parse_add_rules_update(&raw);
        assert_eq!(updates.len(), 1);
        // "Task" is the legacy alias for "Agent".
        assert_eq!(updates[0].rule.value.tool_name, "Agent");
        assert_eq!(updates[0].rule.behavior, PermissionBehavior::Deny);
        assert_eq!(updates[0].rule.source, PermissionRuleSource::UserSettings);
    }

    #[test]
    fn parse_add_rules_update_skips_unmodeled_and_malformed() {
        // Non-addRules types are skipped (replaceRules/removeRules/setMode/dirs).
        assert!(parse_add_rules_update(&json!({
            "type": "replaceRules", "rules": [{ "toolName": "Bash" }],
            "behavior": "allow", "destination": "localSettings"
        }))
        .is_empty());
        assert!(parse_add_rules_update(&json!({
            "type": "setMode", "mode": "plan", "destination": "localSettings"
        }))
        .is_empty());
        // Unknown behavior / destination / missing rules → empty (never error).
        assert!(parse_add_rules_update(&json!({
            "type": "addRules", "rules": [{ "toolName": "Bash" }],
            "behavior": "bogus", "destination": "localSettings"
        }))
        .is_empty());
        assert!(parse_add_rules_update(&json!({
            "type": "addRules", "rules": [{ "toolName": "Bash" }],
            "behavior": "allow", "destination": "bogus"
        }))
        .is_empty());
        assert!(parse_add_rules_update(&json!({
            "type": "addRules", "behavior": "allow", "destination": "localSettings"
        }))
        .is_empty());
        // A non-object entry is skipped.
        assert!(parse_add_rules_update(&json!("nonsense")).is_empty());
    }

    #[tokio::test]
    async fn allow_carries_updated_permissions_through_outcome() {
        // §2b: an allow with `updatedPermissions` surfaces the raw wire array on the
        // outcome; a missing array is simply empty (not a deny).
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check_with_context("Bash", &input, &PermissionCheckContext::default()).await });
        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({
                    "behavior": "allow",
                    "updatedInput": {},
                    "updatedPermissions": [{
                        "type": "addRules",
                        "rules": [{ "toolName": "Bash" }],
                        "behavior": "allow",
                        "destination": "localSettings"
                    }],
                }),
            ))
            .await;
        match check.await.unwrap() {
            PermissionOutcome::Allow {
                permission_updates, ..
            } => {
                assert_eq!(permission_updates.len(), 1);
                assert_eq!(permission_updates[0]["type"], "addRules");
            }
            other => panic!("expected Allow with permission_updates, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_updated_permissions_is_not_a_deny() {
        // The oracle ignores a malformed `updatedPermissions` (`.catch(undefined)`)
        // — it must NOT turn the allow into a deny. A non-array value → empty.
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone());
        let input = json!({});
        let check = tokio::spawn(async move { gate.check("Bash", &input).await });
        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({"behavior": "allow", "updatedInput": {}, "updatedPermissions": "garbage"}),
            ))
            .await;
        assert_eq!(check.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn allow_with_persist_writes_rule_to_settings() {
        // End-to-end: a gate WITH persist paths writes the host's addRules rule to
        // the resolved settings file (claude-code `persistPermissionUpdates`).
        let tmp = std::env::temp_dir().join(format!("lx-p5-2b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = PermissionPaths {
            claude_home: tmp.join("home/.claude"),
            cwd: tmp.join("proj"),
        };
        let (plane, mut rx) = plane_with_channel();
        let gate = StdioControlPermissionGate::new(plane.clone()).with_persist(paths);
        let input = json!({});
        let check = tokio::spawn(async move {
            gate.check_with_context("Bash", &input, &PermissionCheckContext::default())
                .await
        });
        let line = rx.recv().await.unwrap();
        let req_id = serde_json::from_str::<Value>(&line).unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        plane
            .resolve_response(&success_response(
                &req_id,
                json!({
                    "behavior": "allow",
                    "updatedInput": {},
                    "updatedPermissions": [{
                        "type": "addRules",
                        "rules": [{ "toolName": "Bash", "ruleContent": "npm install" }],
                        "behavior": "allow",
                        "destination": "localSettings"
                    }],
                }),
            ))
            .await;
        let outcome = check.await.unwrap();
        assert!(matches!(outcome, PermissionOutcome::Allow { .. }));
        // The rule landed in <cwd>/.claude/settings.local.json.
        let path = tmp.join("proj/.claude/settings.local.json");
        let written = std::fs::read_to_string(&path).expect("settings.local.json written");
        let v: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Bash(npm install)"]));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
