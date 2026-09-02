//! Inbound JSON-RPC request handlers required by the
//! `{roots:{listChanged:true}, elicitation:{}}` capability declaration.
//!
//! Implementations match claude-code's defaults in
//! `services/mcp/client.ts` lines 1009-1018 (roots) and 1188-1197
//! (elicitation default cancel).

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use jsonrpc::{InboundHandler, Request, Response};
use serde_json::{json, Value};
use telemetry::pii::Verified;
use telemetry::tengu::mcp::{ElicitationMode, ElicitationResponsePayload, ElicitationShownPayload};
#[cfg(test)]
use telemetry::tengu::mcp::{ELICITATION_RESPONSE, ELICITATION_SHOWN};

use crate::hook_dispatch::{ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher};

/// A LIVE, shared set of additional working directories advertised as MCP
/// roots after the session cwd. Held behind an `Arc<RwLock<…>>` so a runtime
/// `/add-dir` (parity 2.1.207 P1-08) can push a directory into the SAME cell
/// the registered [`RootsListHandler`] reads — the very next `roots/list` the
/// server issues then reflects the addition, matching claude-code's live
/// `additionalWorkingDirectories`. Mutating it does NOT itself notify servers;
/// the caller pairs a push with [`crate::McpRegistry::notify_roots_list_changed_all`].
pub type SharedRoots = Arc<RwLock<Vec<PathBuf>>>;

/// Build a [`SharedRoots`] cell seeded with `dirs`. An empty seed advertises a
/// cwd-only `roots/list` until something is pushed in.
#[must_use]
pub fn new_shared_roots(dirs: Vec<PathBuf>) -> SharedRoots {
    Arc::new(RwLock::new(dirs))
}

/// Handler for inbound `roots/list` requests from the MCP server.
///
/// Returns `{"roots": [{"uri": "file://<dir>"}, ...]}` — the session's
/// current working directory FIRST, followed by every additional working
/// directory (settings `additionalDirectories` + CLI `--add-dir`), matching
/// claude-code 2.1.207 `r1d()` which builds the list from
/// `[sn(), ...qzn()]` (cwd + `additionalWorkingDirectories`) and dedupes by
/// `pathToFileURL(...).href`. With no additional dirs this collapses to the
/// single-root `{"roots": [{"uri": "file://<cwd>"}]}` shape.
pub struct RootsListHandler {
    /// Absolute current working directory — always the FIRST advertised root.
    pub cwd: PathBuf,
    /// LIVE additional working directories advertised as roots after `cwd`
    /// (settings `additionalDirectories` union CLI `--add-dir`, plus any
    /// runtime `/add-dir`). Read fresh on every `roots/list` so a runtime add
    /// is reflected without a reboot. Deduplicated against `cwd` and each other
    /// by file URL, preserving discovery order.
    pub additional: SharedRoots,
}

impl RootsListHandler {
    /// Build the `{"roots": [...]}` result value: cwd-first, then each
    /// additional dir (read LIVE off the shared cell), deduplicated by
    /// `file://` URL (claude-code `r1d()`).
    fn roots_value(&self) -> Value {
        // Snapshot the live additional-dirs set under a brief read lock (no
        // await is held across it), so a concurrent `/add-dir` push never
        // tears this response.
        let additional = self
            .additional
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        let mut roots = Vec::new();
        for dir in std::iter::once(&self.cwd).chain(additional.iter()) {
            let uri = format!("file://{}", dir.display());
            if seen.insert(uri.clone()) {
                roots.push(json!({ "uri": uri }));
            }
        }
        json!({ "roots": roots })
    }
}

#[async_trait]
impl InboundHandler for RootsListHandler {
    async fn handle(&self, req: Request) -> Response {
        // claude-code wire shape: {"roots": [{"uri": "file://<absolute-dir>"}]}.
        // Paths are forwarded verbatim — the caller is responsible for passing
        // absolute paths (the platform crate that constructs McpClient resolves
        // cwd via `std::env::current_dir()` and expands the additional dirs).
        Response::success(req.id, self.roots_value())
    }
}

/// Handler for inbound `elicitation/create` requests from the MCP server.
///
/// Default response is `{"action": "cancel"}` (matches claude-code
/// `services/mcp/client.ts:1196`).
///
/// When a [`HookDispatcher`] is wired (via [`Self::with_dispatcher`]), the
/// handler first fires the `Elicitation` hook — reproducing claude-code's
/// `runElicitationHooks` (`services/mcp/elicitationHandler.ts:91-107`,
/// `214-257`): a hook may PROVIDE the elicitation answer
/// ([`ElicitationHookOutcome::Respond`]) or DENY it
/// ([`ElicitationHookOutcome::Deny`] => `{"action":"decline"}`). With no hook
/// intervention (or no dispatcher) it falls through to the default
/// `{"action":"cancel"}`. The dispatcher path is strictly best-effort: a hook
/// that does not resolve the request leaves the default behavior intact.
pub struct ElicitationCreateHandler {
    /// Logical MCP server name forwarded into the `Elicitation` hook payload
    /// (wire `mcp_server_name`). Empty string in the legacy default
    /// constructor — only consulted when a dispatcher is wired.
    server_name: String,
    /// Optional hook-dispatch seam. `None` => current behavior (no hook is
    /// fired, default `{"action":"cancel"}`), matching the
    /// `RawConnectionProvider`/auth-provider injection pattern.
    dispatcher: Option<Arc<dyn HookDispatcher>>,
}

impl Default for ElicitationCreateHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ElicitationCreateHandler {
    /// Construct the handler with NO hook dispatcher wired — byte-identical to
    /// the historical unit-struct behavior (`{"action":"cancel"}`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            server_name: String::new(),
            dispatcher: None,
        }
    }

    /// Construct the handler bound to a logical `server_name` and an optional
    /// [`HookDispatcher`]. `dispatcher == None` is equivalent to [`Self::new`]
    /// (current default behavior); `Some(_)` enables the `Elicitation` hook
    /// fire-and-resolve path.
    #[must_use]
    pub fn with_dispatcher(
        server_name: impl Into<String>,
        dispatcher: Option<Arc<dyn HookDispatcher>>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            dispatcher,
        }
    }

    /// Build the byte-faithful [`ElicitationHookRequest`] from the inbound
    /// request params, mirroring how claude-code's `runElicitationHooks`
    /// reads `params.message` / `params.mode` / `params.url` /
    /// `params.elicitationId` / `params.requestedSchema`
    /// (`elicitationHandler.ts:220-239`). Missing fields default to empty /
    /// `None` so a sparse server payload still produces a valid hook input.
    fn build_hook_request(&self, params: Option<&Value>) -> ElicitationHookRequest {
        let p = params.and_then(Value::as_object);
        let str_field = |k: &str| {
            p.and_then(|o| o.get(k))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        };
        ElicitationHookRequest {
            server_name: self.server_name.clone(),
            message: str_field("message").unwrap_or_default(),
            // claude-code normalizes mode to "form" unless it is exactly "url".
            mode: str_field("mode"),
            url: str_field("url"),
            // claude-code reads `params.elicitationId`; the wire payload key is
            // `elicitation_id`. Accept either so the seam is robust to both.
            elicitation_id: str_field("elicitationId").or_else(|| str_field("elicitation_id")),
            requested_schema: p
                .and_then(|o| {
                    o.get("requestedSchema")
                        .or_else(|| o.get("requested_schema"))
                })
                .cloned(),
        }
    }

    /// Provider-neutral MCP elicitation telemetry normalizes the wire mode the
    /// same way the oracle's hook path does: only the exact `"url"` literal
    /// stays URL mode; everything else is treated as form.
    fn telemetry_mode(params: Option<&Value>) -> ElicitationMode {
        match params
            .and_then(Value::as_object)
            .and_then(|o| o.get("mode"))
            .and_then(Value::as_str)
        {
            Some("url") => ElicitationMode::Url,
            Some(_) | None => ElicitationMode::Form,
        }
    }
}

#[async_trait]
impl InboundHandler for ElicitationCreateHandler {
    async fn handle(&self, req: Request) -> Response {
        let mode = Self::telemetry_mode(req.params.as_ref());
        emit_elicitation_shown(mode);
        // When a dispatcher is wired, consult the `Elicitation` hook first.
        // Mirrors `runElicitationHooks` (elicitationHandler.ts:91-107):
        //   * hook RESPONDS  -> use {action, content} as the answer.
        //   * hook DENIES     -> {"action":"decline"}.
        //   * hook PASSES     -> fall through to the default below.
        if let Some(dispatcher) = &self.dispatcher {
            let hook_req = self.build_hook_request(req.params.as_ref());
            match dispatcher.dispatch_elicitation(hook_req).await {
                ElicitationHookOutcome::Respond(answer) => {
                    if let Some(action) = answer.get("action").and_then(Value::as_str) {
                        emit_elicitation_response(mode, action);
                    }
                    return Response::success(req.id, answer);
                }
                ElicitationHookOutcome::Deny => {
                    emit_elicitation_response(mode, "decline");
                    return Response::success(req.id, json!({ "action": "decline" }));
                }
                // Pass: no hook intervened — default behavior takes over.
                ElicitationHookOutcome::Pass => {}
            }
        }
        // claude-code default: deny all elicitations until UI layer overrides.
        emit_elicitation_response(mode, "cancel");
        Response::success(req.id, json!({ "action": "cancel" }))
    }
}

fn emit_elicitation_shown(mode: ElicitationMode) {
    let payload = ElicitationShownPayload { mode };
    telemetry::emit_mcp_elicitation_shown(&payload);
    #[cfg(test)]
    record_test_elicitation_telemetry_event(ELICITATION_SHOWN, &payload);
}

fn emit_elicitation_response(mode: ElicitationMode, action: &str) {
    // MCP's ElicitResult action is a closed, low-cardinality enum. A malformed
    // hook response is returned unchanged by the pre-existing hook seam, but
    // must not turn arbitrary hook text into an analytics dimension.
    if !matches!(action, "accept" | "decline" | "cancel") {
        return;
    }
    let payload = ElicitationResponsePayload {
        mode,
        action: Verified::assert_safe(action.to_string()),
    };
    telemetry::emit_mcp_elicitation_response(&payload);
    #[cfg(test)]
    record_test_elicitation_telemetry_event(ELICITATION_RESPONSE, &payload);
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedElicitationTelemetryEvent {
    name: &'static str,
    payload: Value,
}

#[cfg(test)]
std::thread_local! {
    static TEST_ELICITATION_TELEMETRY_EVENTS:
        std::cell::RefCell<Vec<CapturedElicitationTelemetryEvent>> =
        std::cell::RefCell::new(Vec::new());
}

#[cfg(test)]
fn clear_test_elicitation_telemetry_events() {
    TEST_ELICITATION_TELEMETRY_EVENTS.with(|events| events.borrow_mut().clear());
}

#[cfg(test)]
fn take_test_elicitation_telemetry_events() -> Vec<CapturedElicitationTelemetryEvent> {
    TEST_ELICITATION_TELEMETRY_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

#[cfg(test)]
fn record_test_elicitation_telemetry_event<T: serde::Serialize>(name: &'static str, payload: &T) {
    TEST_ELICITATION_TELEMETRY_EVENTS.with(|events| {
        events.borrow_mut().push(CapturedElicitationTelemetryEvent {
            name,
            payload: serde_json::to_value(payload).expect("serialize elicitation telemetry"),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpc::Id;

    fn req(method: &str) -> Request {
        Request::new(method, None, Id::Number(1))
    }

    #[tokio::test]
    async fn roots_list_returns_file_uri_with_absolute_cwd() {
        let handler = RootsListHandler {
            cwd: PathBuf::from("/Users/example/project"),
            additional: new_shared_roots(Vec::new()),
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        // Wire shape MUST be {"roots": [{"uri": "..."}]}.
        let roots = result["roots"].as_array().expect("roots array");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["uri"], "file:///Users/example/project");
    }

    #[tokio::test]
    async fn roots_list_includes_additional_dirs_cwd_first() {
        // claude-code r1d(): cwd FIRST, then each additionalWorkingDirectory,
        // each advertised as a `file://` root.
        let handler = RootsListHandler {
            cwd: PathBuf::from("/proj"),
            additional: new_shared_roots(vec![
                PathBuf::from("/tmp/extra"),
                PathBuf::from("/opt/data"),
            ]),
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        let roots = result["roots"].as_array().expect("roots array");
        assert_eq!(roots.len(), 3, "cwd + 2 extras");
        assert_eq!(roots[0]["uri"], "file:///proj", "cwd is first");
        assert_eq!(roots[1]["uri"], "file:///tmp/extra");
        assert_eq!(roots[2]["uri"], "file:///opt/data");
    }

    #[tokio::test]
    async fn roots_list_dedupes_additional_dir_matching_cwd() {
        // A duplicate dir (equal to cwd, or repeated) is deduped by file URL —
        // r1d() builds the list through a URL-keyed set.
        let handler = RootsListHandler {
            cwd: PathBuf::from("/proj"),
            additional: new_shared_roots(vec![
                PathBuf::from("/proj"), // dup of cwd
                PathBuf::from("/tmp/extra"),
                PathBuf::from("/tmp/extra"), // dup of an extra
            ]),
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        let roots = result["roots"].as_array().expect("roots array");
        assert_eq!(roots.len(), 2, "cwd + one unique extra (dups dropped)");
        assert_eq!(roots[0]["uri"], "file:///proj");
        assert_eq!(roots[1]["uri"], "file:///tmp/extra");
    }

    #[tokio::test]
    async fn roots_list_reflects_live_mutation_of_shared_cell() {
        // The handler reads the additional-dirs set LIVE off the shared cell:
        // a directory pushed AFTER construction (a runtime `/add-dir`) shows up
        // on the very next `roots/list` — no rebuild, no reboot (parity P1-08).
        let cell = new_shared_roots(Vec::new());
        let handler = RootsListHandler {
            cwd: PathBuf::from("/proj"),
            additional: cell.clone(),
        };

        // Before any add: cwd-only.
        let before = handler.handle(req("roots/list")).await.result.unwrap();
        assert_eq!(before["roots"].as_array().unwrap().len(), 1);

        // Runtime add pushes into the shared cell.
        cell.write().unwrap().push(PathBuf::from("/extra"));

        // The next roots/list reflects it immediately, cwd still first.
        let after = handler.handle(req("roots/list")).await.result.unwrap();
        let roots = after["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 2, "cwd + the live-added dir");
        assert_eq!(roots[0]["uri"], "file:///proj");
        assert_eq!(roots[1]["uri"], "file:///extra");
    }

    #[tokio::test]
    async fn roots_list_uri_uses_literal_file_scheme() {
        let handler = RootsListHandler {
            cwd: PathBuf::from("/tmp/x"),
            additional: new_shared_roots(Vec::new()),
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        let bytes = serde_json::to_vec(&result).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            s.contains(r#""uri":"file:///tmp/x""#),
            "wire bytes must include literal file:// scheme + absolute path, got: {s}",
        );
    }

    #[tokio::test]
    async fn roots_list_outer_envelope_is_object_not_bare_array() {
        // claude-code expects {"roots": [...]} — a bare array would be
        // protocol-incompatible. Lock the outer-envelope shape here.
        let handler = RootsListHandler {
            cwd: PathBuf::from("/x"),
            additional: new_shared_roots(Vec::new()),
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        assert!(
            result.is_object(),
            "roots/list result MUST be an object, not a bare array: {result}",
        );
        let obj = result.as_object().unwrap();
        assert_eq!(obj.len(), 1, "exactly one top-level field");
        assert!(obj.contains_key("roots"));
    }

    #[tokio::test]
    async fn elicitation_create_returns_cancel_literal() {
        let handler = ElicitationCreateHandler::new();
        let resp = handler.handle(req("elicitation/create")).await;
        let result = resp.result.expect("success result");
        // EXACT shape {"action": "cancel"} — no extra fields.
        let obj = result.as_object().expect("object");
        assert_eq!(obj.len(), 1, "exactly one field");
        assert_eq!(obj["action"], "cancel");
    }

    #[tokio::test]
    async fn elicitation_create_raw_bytes_match() {
        let resp = ElicitationCreateHandler::new()
            .handle(req("elicitation/create"))
            .await;
        let result = resp.result.expect("success result");
        let bytes = serde_json::to_vec(&result).unwrap();
        assert_eq!(bytes, br#"{"action":"cancel"}"#);
    }

    #[tokio::test]
    async fn handlers_echo_request_id_in_response() {
        // Per JSON-RPC 2.0: response.id MUST match request.id.
        let r1 = RootsListHandler {
            cwd: PathBuf::from("/x"),
            additional: new_shared_roots(Vec::new()),
        }
        .handle(Request::new("roots/list", None, Id::Number(7)))
        .await;
        assert_eq!(r1.id, Some(Id::Number(7)));

        let r2 = ElicitationCreateHandler::new()
            .handle(Request::new(
                "elicitation/create",
                None,
                Id::String("abc".into()),
            ))
            .await;
        assert_eq!(r2.id, Some(Id::String("abc".into())));
    }

    // --- Hook-dispatch seam (Elicitation hook fire + resolve) --------------

    use crate::hook_dispatch::{ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher};
    use std::sync::Mutex;

    /// Records the request it received and returns a canned outcome. Used to
    /// assert (a) the right payload is forwarded and (b)/(c) the outcome maps
    /// to the correct response.
    struct MockDispatcher {
        outcome: ElicitationHookOutcome,
        seen: Mutex<Option<ElicitationHookRequest>>,
    }

    impl MockDispatcher {
        fn new(outcome: ElicitationHookOutcome) -> Self {
            Self {
                outcome,
                seen: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl HookDispatcher for MockDispatcher {
        async fn dispatch_elicitation(
            &self,
            request: ElicitationHookRequest,
        ) -> ElicitationHookOutcome {
            *self.seen.lock().unwrap() = Some(request);
            self.outcome.clone()
        }
    }

    fn elicit_req(params: Value) -> Request {
        Request::new("elicitation/create", Some(params), Id::Number(9))
    }

    #[tokio::test]
    async fn elicitation_fires_hook_with_byte_faithful_payload() {
        // (a) An elicitation fires `Elicitation` with the right payload —
        // server name from the handler, plus message/mode/url/elicitationId/
        // requestedSchema lifted verbatim from the request params.
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Pass));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "github",
            Some(mock.clone() as Arc<dyn HookDispatcher>),
        );
        let params = json!({
            "message": "Authorize?",
            "mode": "url",
            "url": "https://example.test",
            "elicitationId": "e-1",
            "requestedSchema": {"type": "object"},
        });
        let _ = handler.handle(elicit_req(params)).await;

        let seen = mock.seen.lock().unwrap().clone().expect("hook fired");
        assert_eq!(seen.server_name, "github");
        assert_eq!(seen.message, "Authorize?");
        assert_eq!(seen.mode.as_deref(), Some("url"));
        assert_eq!(seen.url.as_deref(), Some("https://example.test"));
        assert_eq!(seen.elicitation_id.as_deref(), Some("e-1"));
        assert_eq!(seen.requested_schema, Some(json!({"type": "object"})));
    }

    #[tokio::test]
    async fn elicitation_uses_hook_provided_response() {
        // (b) A hook-provided response is used verbatim as the answer.
        let answer = json!({"action": "accept", "content": {"token": "xyz"}});
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Respond(
            answer.clone(),
        )));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "linear",
            Some(mock as Arc<dyn HookDispatcher>),
        );
        let resp = handler.handle(elicit_req(json!({"message": "Pick"}))).await;
        let result = resp.result.expect("success result");
        assert_eq!(result, answer);
    }

    #[tokio::test]
    async fn elicitation_hook_denial_declines() {
        // (c) A hook denial maps to {"action":"decline"} (NOT the default
        // "cancel"), matching `runElicitationHooks` blockingError -> decline.
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Deny));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "linear",
            Some(mock as Arc<dyn HookDispatcher>),
        );
        let resp = handler.handle(elicit_req(json!({"message": "Pick"}))).await;
        let result = resp.result.expect("success result");
        let obj = result.as_object().expect("object");
        assert_eq!(obj.len(), 1);
        assert_eq!(obj["action"], "decline");
    }

    #[tokio::test]
    async fn elicitation_hook_pass_falls_through_to_default() {
        // Pass => default {"action":"cancel"} (current behavior preserved even
        // when a dispatcher is wired but no hook intervenes).
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Pass));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "linear",
            Some(mock as Arc<dyn HookDispatcher>),
        );
        let resp = handler.handle(elicit_req(json!({"message": "Pick"}))).await;
        let result = resp.result.expect("success result");
        assert_eq!(result, json!({"action": "cancel"}));
    }

    #[tokio::test]
    async fn elicitation_no_dispatcher_is_unchanged_default() {
        // (d) No dispatcher => current default unchanged (strict no-op): the
        // handler never even looks at the params, returns {"action":"cancel"}.
        let handler = ElicitationCreateHandler::with_dispatcher("github", None);
        let resp = handler.handle(elicit_req(json!({"message": "Pick"}))).await;
        let result = resp.result.expect("success result");
        let bytes = serde_json::to_vec(&result).unwrap();
        assert_eq!(bytes, br#"{"action":"cancel"}"#);
    }

    #[tokio::test]
    async fn elicitation_default_cancel_emits_form_shown_and_response_once() {
        clear_test_elicitation_telemetry_events();
        let handler = ElicitationCreateHandler::new();
        let resp = handler.handle(elicit_req(json!({"message": "Pick"}))).await;
        assert_eq!(resp.result.unwrap(), json!({"action": "cancel"}));
        assert_eq!(
            take_test_elicitation_telemetry_events(),
            vec![
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_SHOWN,
                    payload: serde_json::to_value(ElicitationShownPayload {
                        mode: ElicitationMode::Form,
                    })
                    .unwrap(),
                },
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_RESPONSE,
                    payload: serde_json::to_value(ElicitationResponsePayload {
                        mode: ElicitationMode::Form,
                        action: Verified::assert_safe("cancel".to_string()),
                    })
                    .unwrap(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn elicitation_url_cancel_emits_url_mode() {
        clear_test_elicitation_telemetry_events();
        let handler = ElicitationCreateHandler::new();
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick", "mode": "url"})))
            .await;
        assert_eq!(resp.result.unwrap(), json!({"action": "cancel"}));
        assert_eq!(
            take_test_elicitation_telemetry_events(),
            vec![
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_SHOWN,
                    payload: serde_json::to_value(ElicitationShownPayload {
                        mode: ElicitationMode::Url,
                    })
                    .unwrap(),
                },
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_RESPONSE,
                    payload: serde_json::to_value(ElicitationResponsePayload {
                        mode: ElicitationMode::Url,
                        action: Verified::assert_safe("cancel".to_string()),
                    })
                    .unwrap(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn elicitation_hook_deny_emits_form_decline_response() {
        clear_test_elicitation_telemetry_events();
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Deny));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "linear",
            Some(mock as Arc<dyn HookDispatcher>),
        );
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick", "mode": "other"})))
            .await;
        assert_eq!(resp.result.unwrap(), json!({"action": "decline"}));
        assert_eq!(
            take_test_elicitation_telemetry_events(),
            vec![
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_SHOWN,
                    payload: serde_json::to_value(ElicitationShownPayload {
                        mode: ElicitationMode::Form,
                    })
                    .unwrap(),
                },
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_RESPONSE,
                    payload: serde_json::to_value(ElicitationResponsePayload {
                        mode: ElicitationMode::Form,
                        action: Verified::assert_safe("decline".to_string()),
                    })
                    .unwrap(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn elicitation_hook_response_emits_actual_url_action() {
        clear_test_elicitation_telemetry_events();
        let mock = Arc::new(MockDispatcher::new(ElicitationHookOutcome::Respond(
            json!({"action": "accept", "content": {"token": "xyz"}}),
        )));
        let handler = ElicitationCreateHandler::with_dispatcher(
            "linear",
            Some(mock as Arc<dyn HookDispatcher>),
        );
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick", "mode": "url"})))
            .await;
        assert_eq!(
            resp.result.unwrap(),
            json!({"action": "accept", "content": {"token": "xyz"}})
        );
        assert_eq!(
            take_test_elicitation_telemetry_events(),
            vec![
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_SHOWN,
                    payload: serde_json::to_value(ElicitationShownPayload {
                        mode: ElicitationMode::Url,
                    })
                    .unwrap(),
                },
                CapturedElicitationTelemetryEvent {
                    name: ELICITATION_RESPONSE,
                    payload: serde_json::to_value(ElicitationResponsePayload {
                        mode: ElicitationMode::Url,
                        action: Verified::assert_safe("accept".to_string()),
                    })
                    .unwrap(),
                },
            ]
        );
    }
}
