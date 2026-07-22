//! `AdapterPermissionGate` — the id-keyed, fail-closed `traits::PermissionGate`
//! impl (plan F1-14).
//!
//! This is the hardest mapping in the adapter: it replicates the proven
//! `TuiPermissionGate` (`tui/src/permission_bridge.rs:64-118`, COPIED not
//! imported per plan F1-10) but is **keyed by `request_id`** so a single
//! connection can multiplex concurrent worker + main permission round-trips,
//! with an explicit **fail-closed owner** so a vanished resolving task can never
//! hang the turn future.
//!
//! ## Shape (mirrors `TuiPermissionGate`, id-keyed)
//!
//! `check(name, &Value)` ([`traits::PermissionGate`]):
//! 1. Consult the session rules — short-circuit `Allow` if a rule matches the
//!    bare tool name (same as the TUI gate).
//! 2. Build a [`PermissionKindDto::ToolUseConfirm`] from `permission::tool_default`
//!    (the `PromptDefault` → `default_allow: bool` collapse, reusing
//!    [`crate::lowering::prompt_default_to_allow`]) and the lowered tool input
//!    JSON string ([`crate::lowering::value_to_json_string`]).
//! 3. Assign a fresh `request_id` via an [`AtomicU64`], park a
//!    `oneshot::Sender<PermissionResponse>` in the id-keyed `HashMap`, and emit a
//!    [`PermissionRequest`] DTO through the [`PermissionRequestSink`].
//! 4. `await` the oneshot, **bounded by a per-request timeout**. A dropped sender
//!    (the resolving task vanished / the map was drained) OR a timeout both
//!    resolve to [`PermissionDecision::Deny`] — fail-closed.
//!
//! `resolve(request_id, PermissionResponseDto)` is called by the transport from
//! a DIFFERENT task on an inbound `ApprovePermission`/`DenyPermission` command:
//! it looks up the id, sends on the parked oneshot, and (for `AllowAlways`)
//! appends a session [`PermissionRule`].
//!
//! ## Fail-closed owner
//!
//! The gate owns the id→sender `HashMap`. On transport teardown / app-background
//! the connection drops the gate (or calls [`AdapterPermissionGate::drain`]),
//! which drops every parked sender ⇒ every in-flight `check()` resolves `Deny`.
//! A [`Drop`] impl drains as a backstop. Combined with the per-request timeout,
//! no parked turn future can hang forever.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client_protocol::permission::{
    PermissionKindDto, PermissionRequest as PermissionRequestDto, PermissionResponseDto,
    WorkerInfoDto,
};
use permission::gate::{PermissionDecision, PermissionGate, PermissionResponse, PromptWorker};
use permission::{
    persist_permission_update, PermissionPaths, PermissionRule, PermissionUpdate,
    PermissionUpdateDestination,
};
use tokio::sync::{oneshot, Mutex};

use crate::lowering::{prompt_default_to_allow, value_to_json_string};

/// Default per-request timeout — a `check()` parked longer than this resolves
/// `Deny` (fail-closed). Generous enough that a real user dialog round-trip
/// never trips it, short enough that a vanished resolving task does not hang the
/// turn future indefinitely.
pub const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

/// Transport-supplied destination for the outbound [`PermissionRequestDto`].
///
/// `PermissionRequest` is NOT a [`client_protocol::events::ClientEvent`] variant
/// (the frozen F1-04 contract surfaces it as its own permission DTO), so it does
/// NOT travel over the [`crate::ClientEventSink`]. The gate instead pushes each
/// request through this dedicated seam; the transport wraps it into a
/// `Frame::Event(PermissionRequest)` (bridge-server, F2-06) or hands it to the
/// `ClientEventListener` (mobile, F3). Like the event sink, it is object-safe and
/// always held behind an `Arc<dyn PermissionRequestSink>`.
#[async_trait]
pub trait PermissionRequestSink: Send + Sync {
    /// Forward one fully-lowered permission request to the underlying transport.
    /// Implementations should be cheap / non-blocking on the engine task.
    async fn emit_request(&self, request: PermissionRequestDto);
}

/// One parked permission round-trip: the oneshot the awaiting `check()` blocks
/// on, plus the call's tool input (so `resolve` can narrow an `AllowAlways`).
struct ParkedRequest {
    /// Resolves the awaiting `check()` future.
    sender: oneshot::Sender<PermissionResponse>,
    /// The tool input of the call, captured at `check()` time.
    input: serde_json::Value,
}

/// The id-keyed, fail-closed permission gate the orchestrator binds as its
/// `Arc<dyn PermissionGate>` on a client connection.
///
/// Connection-scoped: one gate per transport connection. Cloneable handles share
/// the same parked-request map and session rules via `Arc`, so the transport read
/// task ([`Self::resolve`]) and the engine turn task ([`PermissionGate::check`])
/// operate on the same state.
pub struct AdapterPermissionGate {
    /// Where outbound [`PermissionRequestDto`]s go (the transport).
    sink: Arc<dyn PermissionRequestSink>,
    /// Session-scoped allow rules. Consulted before a request is emitted;
    /// appended to on `AllowAlways`. Shared with the rest of the connection.
    session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>,
    /// Monotonic `request_id` source. Each `check()` reserves a fresh id so
    /// concurrent worker + main requests never collide.
    next_id: Arc<AtomicU64>,
    /// Parked requests keyed by `request_id`. The fail-closed owner: when this map
    /// is drained (or the gate dropped), every [`ParkedRequest::sender`] drops and
    /// the matching `check()` resolves `Deny`. Each entry also carries the call's
    /// tool input so [`Self::resolve`] can NARROW an `AllowAlways` to the specific
    /// command / path / domain (the input is not echoed back on the wire).
    pending: Arc<Mutex<HashMap<u64, ParkedRequest>>>,
    /// Per-request timeout — a parked `check()` that is not resolved within this
    /// window resolves `Deny`.
    timeout: Duration,
    /// (3c) Filesystem roots for persisting an `AllowAlways` choice to
    /// `settings.local.json`. `None` → session-only (the legacy behavior); when
    /// set, an `AllowAlways` additionally writes a durable allow rule.
    persist_paths: Option<PermissionPaths>,
}

impl AdapterPermissionGate {
    /// Construct a gate with the [`DEFAULT_PERMISSION_TIMEOUT`] and a fresh empty
    /// session-rule list.
    #[must_use]
    pub fn new(sink: Arc<dyn PermissionRequestSink>) -> Self {
        Self::with_rules(sink, Arc::new(Mutex::new(Vec::new())))
    }

    /// Construct a gate sharing an existing session-rule list (the connection's
    /// canonical list, also surfaced to `/permissions`).
    #[must_use]
    pub fn with_rules(
        sink: Arc<dyn PermissionRequestSink>,
        session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>,
    ) -> Self {
        Self {
            sink,
            session_allow_rules,
            next_id: Arc::new(AtomicU64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            timeout: DEFAULT_PERMISSION_TIMEOUT,
            persist_paths: None,
        }
    }

    /// Override the per-request timeout (tests use a short window for
    /// `timeout_resolves_deny`).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// (3c) Enable persisting an `AllowAlways` choice to `settings.local.json`
    /// (under `paths.cwd/.lingxi/`). Without this, `AllowAlways` is session-only.
    #[must_use]
    pub fn with_persist(mut self, paths: PermissionPaths) -> Self {
        self.persist_paths = Some(paths);
        self
    }

    /// Shared handle to the session-allow-rule list (so the connection can also
    /// read/surface it).
    #[must_use]
    pub fn session_allow_rules(&self) -> Arc<Mutex<Vec<PermissionRule>>> {
        self.session_allow_rules.clone()
    }

    /// Resolve a parked request by `request_id` from a DIFFERENT task (the
    /// transport read loop on an inbound `ApprovePermission`/`DenyPermission`).
    ///
    /// Looks up and removes the parked sender, then sends the mapped
    /// [`PermissionResponse`]. `AllowAlways` additionally appends a session
    /// [`PermissionRule`] for `tool_name` so subsequent same-tool calls skip the
    /// dialog. Returns `true` if a matching request was found and resolved,
    /// `false` if the id was unknown / already resolved.
    pub async fn resolve(
        &self,
        request_id: u64,
        response: PermissionResponseDto,
        tool_name: &str,
    ) -> bool {
        let parked = { self.pending.lock().await.remove(&request_id) };
        let Some(ParkedRequest { sender, input }) = parked else {
            return false;
        };

        if matches!(response, PermissionResponseDto::AllowAlways) {
            // NARROW the persisted grant to the specific command / path / domain
            // the call used (claude-code `ruleSuggestions`), not a tool-wide allow.
            let rule = permission::allow_suggestion(tool_name, &input);
            self.session_allow_rules.lock().await.push(rule.clone());
            // (3c) Durably record the choice to settings.local.json when a
            // persist target is wired. Best-effort: a write failure must not
            // fail the resolve (the session rule above still skips re-prompts).
            // Skip a degenerate empty tool name (a missing request id resolves
            // `tool_name = ""`) so we never persist `allow: [""]`.
            if let Some(paths) = self
                .persist_paths
                .as_ref()
                .filter(|_| !tool_name.is_empty())
            {
                let update = PermissionUpdate {
                    rule,
                    destination: PermissionUpdateDestination::LocalSettings,
                };
                if let Err(e) = persist_permission_update(&update, paths).await {
                    tracing::warn!(error = %e, tool = tool_name, "failed to persist AllowAlways permission rule");
                }
            }
        }

        let mapped = match response {
            PermissionResponseDto::AllowOnce => PermissionResponse::AllowOnce,
            PermissionResponseDto::AllowAlways => PermissionResponse::AllowAlways,
            // `#[non_exhaustive]` — any future/`Deny` response fails closed.
            _ => PermissionResponse::Deny,
        };
        // If the receiver vanished (timed out first), the send simply fails;
        // that path already resolved `Deny`, so it is safe to ignore.
        sender.send(mapped).is_ok()
    }

    /// Fail-closed drain: drop every parked sender so all in-flight `check()`
    /// calls resolve `Deny`. Called on transport teardown / app-background.
    /// Returns the number of requests that were drained.
    pub async fn drain(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let n = pending.len();
        pending.clear();
        n
    }

    /// Number of requests currently parked (test/inspection helper).
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

impl Drop for AdapterPermissionGate {
    fn drop(&mut self) {
        // Backstop drain: drop every parked sender so any still-awaiting
        // `check()` future resolves `Deny`. `get_mut` avoids needing an async
        // context in `drop` — we hold the only `&mut self` here.
        if let Ok(mut pending) = self.pending.try_lock() {
            pending.clear();
        }
        // If the lock is contended (another clone is mid-`resolve`/`check`), the
        // remaining `Arc`s keep the map alive; the LAST holder's drop (or an
        // explicit `drain`) clears it. The per-request timeout is the final
        // backstop regardless.
    }
}

#[async_trait]
impl PermissionGate for AdapterPermissionGate {
    async fn check(&self, name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Main-thread call — no worker attribution on the wire.
        self.check_with_worker(name, input, None).await
    }

    async fn check_with_worker(
        &self,
        name: &str,
        input: &serde_json::Value,
        worker: Option<PromptWorker>,
    ) -> PermissionDecision {
        // Step 1: consult session rules (identical to the TUI gate; content-aware
        // so a narrowed AllowAlways rule only short-circuits a matching call).
        {
            let rules = self.session_allow_rules.lock().await;
            if rules
                .iter()
                .any(|r| permission::call_matches_rule(r, name, input))
            {
                return PermissionDecision::Allow;
            }
        }

        // Step 2: build the request DTO. Collapse `PromptDefault` → bool and
        // lower the tool input `Value` → JSON string (reusing the F1-11 fns).
        let default_allow = prompt_default_to_allow(permission::tool_default(name));
        let kind = PermissionKindDto::ToolUseConfirm {
            tool_name: name.to_string(),
            tool_input_json: value_to_json_string(input),
            default_allow,
        };

        // Step 3: reserve a fresh id, park the oneshot, emit the request.
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            self.pending.lock().await.insert(
                request_id,
                ParkedRequest {
                    sender: tx,
                    input: input.clone(),
                },
            );
        }
        let request = PermissionRequestDto {
            request_id,
            kind,
            // Worker attribution (claude-code 2.1.186): a subagent/teammate's
            // permission prompt carries the asking worker so the remote client
            // renders the `● @name` badge. `color` seeds the multiagent color
            // (the worker's display name); `None` for a main-thread call.
            worker: worker.map(|w| WorkerInfoDto {
                color: w.name.clone(),
                name: w.name,
                team: w.team,
            }),
        };
        self.sink.emit_request(request).await;

        // Step 4: await the resolution, bounded by the per-request timeout.
        // A dropped sender (drain / vanished resolver) OR a timeout both fail
        // closed to `Deny`. On timeout we also evict the now-stale parked entry
        // so a late `resolve()` is a no-op (and the map does not leak).
        let response = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_dropped)) => {
                return PermissionDecision::Deny {
                    reason: "permission request dropped (connection closed)".to_string(),
                };
            }
            Err(_elapsed) => {
                self.pending.lock().await.remove(&request_id);
                return PermissionDecision::Deny {
                    reason: "permission request timed out".to_string(),
                };
            }
        };

        // Step 5: map the resolved response → decision (the AllowAlways rule was
        // already persisted in `resolve`).
        match response {
            PermissionResponse::AllowOnce | PermissionResponse::AllowAlways => {
                PermissionDecision::Allow
            }
            PermissionResponse::Deny => PermissionDecision::Deny {
                reason: "user denied via permission dialog".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::Mutex as TokioMutex;

    /// A [`PermissionRequestSink`] that captures every emitted request so a test
    /// can read back the assigned `request_id` and assert the lowered fields.
    #[derive(Default)]
    struct MockRequestSink {
        requests: TokioMutex<Vec<PermissionRequestDto>>,
    }

    impl MockRequestSink {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        async fn requests(&self) -> Vec<PermissionRequestDto> {
            self.requests.lock().await.clone()
        }
        async fn last(&self) -> PermissionRequestDto {
            self.requests
                .lock()
                .await
                .last()
                .cloned()
                .expect("a request was emitted")
        }
    }

    #[async_trait]
    impl PermissionRequestSink for MockRequestSink {
        async fn emit_request(&self, request: PermissionRequestDto) {
            self.requests.lock().await.push(request);
        }
    }

    /// Spin until the gate has parked exactly `n` requests (the emit + park
    /// happens on the spawned `check()` task). Bounded so a bug can't hang.
    async fn wait_for_pending(gate: &AdapterPermissionGate, n: usize) {
        for _ in 0..2000 {
            if gate.pending_count().await == n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("gate never reached {n} pending requests");
    }

    // ── Mirror of the 4 `permission_bridge.rs` gate tests ──────────────────

    /// `gate_emits_request_and_resolves_allow_once` — emit id N, `resolve(N,
    /// AllowOnce)`, assert `check()` returns `PermissionDecision::Allow`.
    #[tokio::test]
    async fn gate_emits_request_and_resolves_allow_once() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g = gate.clone();
        let task = tokio::spawn(async move { g.check("Bash", &json!({"command": "ls"})).await });

        wait_for_pending(&gate, 1).await;
        let req = sink.last().await;
        match &req.kind {
            PermissionKindDto::ToolUseConfirm { tool_name, .. } => assert_eq!(tool_name, "Bash"),
            other => panic!("unexpected kind: {other:?}"),
        }
        assert!(
            gate.resolve(req.request_id, PermissionResponseDto::AllowOnce, "Bash")
                .await
        );

        let decision = task.await.unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
        // No rule persisted on AllowOnce.
        assert!(gate.session_allow_rules().lock().await.is_empty());
    }

    /// `gate_skips_dialog_when_session_rule_matches` — a pre-seeded rule
    /// short-circuits `Allow` with NO request emitted.
    #[tokio::test]
    async fn gate_skips_dialog_when_session_rule_matches() {
        let sink = MockRequestSink::arc();
        let rules = Arc::new(Mutex::new(vec![PermissionRule::allow_tool_session("Bash")]));
        let gate = AdapterPermissionGate::with_rules(sink.clone(), rules);

        let decision = gate.check("Bash", &json!({"command": "ls"})).await;
        assert_eq!(decision, PermissionDecision::Allow);
        // No request was emitted to the transport.
        assert!(sink.requests().await.is_empty());
        assert_eq!(gate.pending_count().await, 0);
    }

    /// `gate_persists_allow_always` — `AllowAlways` appends a session rule.
    #[tokio::test]
    async fn gate_persists_allow_always() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g = gate.clone();
        let task = tokio::spawn(async move { g.check("Bash", &json!({})).await });

        wait_for_pending(&gate, 1).await;
        let req = sink.last().await;
        assert!(
            gate.resolve(req.request_id, PermissionResponseDto::AllowAlways, "Bash")
                .await
        );

        let decision = task.await.unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        let stored = gate.session_allow_rules();
        let stored = stored.lock().await;
        assert_eq!(stored.len(), 1);
        assert!(stored[0].matches_tool("Bash"));
    }

    /// `AllowAlways` NARROWS the persisted rule to the call's command (not a bare
    /// tool-wide allow), and the narrowed rule still short-circuits a matching
    /// later call within the session.
    #[tokio::test]
    async fn gate_persists_allow_always_narrows_to_command() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g = gate.clone();
        let input = json!({ "command": "git commit -m \"x\"" });
        let task = tokio::spawn(async move { g.check("Bash", &input).await });

        wait_for_pending(&gate, 1).await;
        let req = sink.last().await;
        assert!(
            gate.resolve(req.request_id, PermissionResponseDto::AllowAlways, "Bash")
                .await
        );
        assert_eq!(task.await.unwrap(), PermissionDecision::Allow);

        // The stored rule is narrowed to the command prefix, NOT tool-wide.
        {
            let stored = gate.session_allow_rules();
            let stored = stored.lock().await;
            assert_eq!(stored.len(), 1);
            assert_eq!(
                stored[0].value.rule_content.as_deref(),
                // `PermissionRule` keeps the shell matcher in its canonical
                // internal space-star form. The settings serializer is the
                // boundary that renders this as `Bash(git commit:*)`.
                Some("git commit *")
            );
            assert!(
                !stored[0].matches_tool("Bash"),
                "narrowed rule is not tool-wide"
            );
        }

        // A matching later command short-circuits (no new request emitted)...
        assert_eq!(
            gate.check("Bash", &json!({ "command": "git commit -m \"y\"" }))
                .await,
            PermissionDecision::Allow
        );
        // ...but a DIFFERENT command still prompts (would park a new request).
        let g2 = gate.clone();
        let other =
            tokio::spawn(async move { g2.check("Bash", &json!({ "command": "rm -rf /" })).await });
        wait_for_pending(&gate, 1).await;
        let req2 = sink.last().await;
        assert!(
            gate.resolve(req2.request_id, PermissionResponseDto::Deny, "Bash")
                .await
        );
        assert!(matches!(
            other.await.unwrap(),
            PermissionDecision::Deny { .. }
        ));
    }

    /// (3c) `gate_persists_allow_always_writes_local_settings` — with a persist
    /// target wired, `AllowAlways` ALSO writes a durable rule to
    /// `<cwd>/.lingxi/settings.local.json` (in addition to the session rule).
    #[tokio::test]
    async fn gate_persists_allow_always_writes_local_settings() {
        let tmp = std::env::temp_dir().join(format!("lx-3c-adapter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()).with_persist(
            permission::PermissionPaths {
                lingxi_home: tmp.join("home/.lingxi"),
                cwd: tmp.join("proj"),
            },
        ));

        let g = gate.clone();
        let task = tokio::spawn(async move { g.check("Bash", &json!({})).await });
        wait_for_pending(&gate, 1).await;
        let req = sink.last().await;
        assert!(
            gate.resolve(req.request_id, PermissionResponseDto::AllowAlways, "Bash")
                .await
        );
        assert_eq!(task.await.unwrap(), PermissionDecision::Allow);

        // The choice was persisted to settings.local.json.
        let path = tmp.join("proj/.lingxi/settings.local.json");
        let written = std::fs::read_to_string(&path).expect("settings.local.json written");
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Bash"]));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `gate_denies_on_user_deny` — `Deny` resolves to `PermissionDecision::Deny`.
    #[tokio::test]
    async fn gate_denies_on_user_deny() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g = gate.clone();
        let task = tokio::spawn(async move { g.check("Bash", &json!({})).await });

        wait_for_pending(&gate, 1).await;
        let req = sink.last().await;
        assert!(
            gate.resolve(req.request_id, PermissionResponseDto::Deny, "Bash")
                .await
        );

        match task.await.unwrap() {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("user denied"), "got: {reason}");
            }
            PermissionDecision::Allow => panic!("expected Deny"),
        }
        // Resolved request is no longer parked.
        assert_eq!(gate.pending_count().await, 0);
    }

    // ── NEW (plan F1-14): id-keyed concurrency + fail-closed paths ──────────

    /// `concurrent_worker_and_main_ids_resolve_independently` — two parked
    /// requests get distinct ids and resolve to independent decisions.
    #[tokio::test]
    async fn concurrent_worker_and_main_ids_resolve_independently() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g1 = gate.clone();
        let main = tokio::spawn(async move { g1.check("Bash", &json!({"who": "main"})).await });
        let g2 = gate.clone();
        let worker =
            tokio::spawn(async move { g2.check("Write", &json!({"who": "worker"})).await });

        wait_for_pending(&gate, 2).await;
        let reqs = sink.requests().await;
        assert_eq!(reqs.len(), 2);
        // Distinct ids.
        assert_ne!(reqs[0].request_id, reqs[1].request_id);

        // Find each by tool name, resolve them to OPPOSITE decisions.
        let bash = reqs.iter().find(|r| {
            matches!(&r.kind, PermissionKindDto::ToolUseConfirm { tool_name, .. } if tool_name == "Bash")
        }).unwrap();
        let write = reqs.iter().find(|r| {
            matches!(&r.kind, PermissionKindDto::ToolUseConfirm { tool_name, .. } if tool_name == "Write")
        }).unwrap();

        assert!(
            gate.resolve(bash.request_id, PermissionResponseDto::AllowOnce, "Bash")
                .await
        );
        assert!(
            gate.resolve(write.request_id, PermissionResponseDto::Deny, "Write")
                .await
        );

        assert_eq!(main.await.unwrap(), PermissionDecision::Allow);
        match worker.await.unwrap() {
            PermissionDecision::Deny { .. } => {}
            PermissionDecision::Allow => panic!("worker expected Deny"),
        }
    }

    /// `drop_resolves_deny` — draining the parked map drops the sender ⇒ the
    /// in-flight `check()` resolves `Deny` (fail-closed on transport teardown).
    #[tokio::test]
    async fn drop_resolves_deny() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        let g = gate.clone();
        let task = tokio::spawn(async move { g.check("Bash", &json!({})).await });

        wait_for_pending(&gate, 1).await;
        let drained = gate.drain().await;
        assert_eq!(drained, 1);

        match task.await.unwrap() {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("dropped"), "got: {reason}");
            }
            PermissionDecision::Allow => panic!("expected Deny after drain"),
        }
    }

    /// `timeout_resolves_deny` — a `check()` parked past the per-request timeout
    /// resolves `Deny` even if nobody ever resolves it.
    #[tokio::test]
    async fn timeout_resolves_deny() {
        let sink = MockRequestSink::arc();
        let gate = AdapterPermissionGate::new(sink.clone()).with_timeout(Duration::from_millis(20));

        let decision = gate.check("Bash", &json!({})).await;
        match decision {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("timed out"), "got: {reason}");
            }
            PermissionDecision::Allow => panic!("expected Deny on timeout"),
        }
        // The stale parked entry was evicted on timeout (no leak).
        assert_eq!(gate.pending_count().await, 0);
    }

    /// `tool_use_confirm_constructed_with_default_allow` — the `tool_default` →
    /// `default_allow: bool` collapse: `Read` is allow-by-default, `Bash` is not.
    #[tokio::test]
    async fn tool_use_confirm_constructed_with_default_allow() {
        let sink = MockRequestSink::arc();
        let gate = Arc::new(AdapterPermissionGate::new(sink.clone()));

        // Read → AllowByDefault → default_allow: true.
        let g = gate.clone();
        let t = tokio::spawn(async move { g.check("Read", &json!({"path": "/tmp/x"})).await });
        wait_for_pending(&gate, 1).await;
        let read_req = sink.last().await;
        match &read_req.kind {
            PermissionKindDto::ToolUseConfirm {
                tool_name,
                tool_input_json,
                default_allow,
            } => {
                assert_eq!(tool_name, "Read");
                assert!(*default_allow, "Read is allow-by-default");
                // Input was lowered to a JSON string (decision §0.4).
                assert_eq!(tool_input_json, &json!({"path": "/tmp/x"}).to_string());
            }
            other => panic!("unexpected kind: {other:?}"),
        }
        assert!(
            gate.resolve(
                read_req.request_id,
                PermissionResponseDto::AllowOnce,
                "Read"
            )
            .await
        );
        t.await.unwrap();

        // Bash → DenyByDefault → default_allow: false.
        let g = gate.clone();
        let t = tokio::spawn(async move { g.check("Bash", &json!({})).await });
        wait_for_pending(&gate, 1).await;
        let bash_req = sink.last().await;
        match &bash_req.kind {
            PermissionKindDto::ToolUseConfirm { default_allow, .. } => {
                assert!(!*default_allow, "Bash is deny-by-default");
            }
            other => panic!("unexpected kind: {other:?}"),
        }
        assert!(
            gate.resolve(bash_req.request_id, PermissionResponseDto::Deny, "Bash")
                .await
        );
        t.await.unwrap();
    }

    /// Resolving an unknown / already-resolved id is a safe no-op.
    #[tokio::test]
    async fn resolve_unknown_id_is_noop() {
        let sink = MockRequestSink::arc();
        let gate = AdapterPermissionGate::new(sink);
        assert!(
            !gate
                .resolve(999, PermissionResponseDto::AllowOnce, "Bash")
                .await
        );
    }
}
