//! Inbound JSON-RPC request handlers required by the
//! `{roots:{}, elicitation:{}}` capability declaration.
//!
//! Implementations match claude-code's defaults in
//! `services/mcp/client.ts` lines 1009-1018 (roots) and 1188-1197
//! (elicitation default cancel).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use jsonrpc::{InboundHandler, Request, Response};
use serde_json::{json, Value};

use crate::hook_dispatch::{ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher};

/// Handler for inbound `roots/list` requests from the MCP server.
///
/// Returns `{"roots": [{"uri": "file://<cwd>"}]}` where `<cwd>` is the
/// absolute path supplied at [`crate::McpClient`] construction time.
pub struct RootsListHandler {
    /// Absolute current working directory advertised as the single root.
    pub cwd: PathBuf,
}

#[async_trait]
impl InboundHandler for RootsListHandler {
    async fn handle(&self, req: Request) -> Response {
        // claude-code wire shape: {"roots": [{"uri": "file://<absolute-cwd>"}]}.
        // The path is forwarded verbatim — caller is responsible for passing
        // an absolute path (the platform crate that constructs McpClient
        // resolves cwd via `std::env::current_dir()` before handing it in).
        let uri = format!("file://{}", self.cwd.display());
        Response::success(req.id, json!({ "roots": [ { "uri": uri } ] }))
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
                .and_then(|o| o.get("requestedSchema").or_else(|| o.get("requested_schema")))
                .cloned(),
        }
    }
}

#[async_trait]
impl InboundHandler for ElicitationCreateHandler {
    async fn handle(&self, req: Request) -> Response {
        // When a dispatcher is wired, consult the `Elicitation` hook first.
        // Mirrors `runElicitationHooks` (elicitationHandler.ts:91-107):
        //   * hook RESPONDS  -> use {action, content} as the answer.
        //   * hook DENIES     -> {"action":"decline"}.
        //   * hook PASSES     -> fall through to the default below.
        if let Some(dispatcher) = &self.dispatcher {
            let hook_req = self.build_hook_request(req.params.as_ref());
            match dispatcher.dispatch_elicitation(hook_req).await {
                ElicitationHookOutcome::Respond(answer) => {
                    return Response::success(req.id, answer);
                }
                ElicitationHookOutcome::Deny => {
                    return Response::success(req.id, json!({ "action": "decline" }));
                }
                // Pass: no hook intervened — default behavior takes over.
                ElicitationHookOutcome::Pass => {}
            }
        }
        // claude-code default: deny all elicitations until UI layer overrides.
        Response::success(req.id, json!({ "action": "cancel" }))
    }
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
        };
        let resp = handler.handle(req("roots/list")).await;
        let result = resp.result.expect("success result");
        // Wire shape MUST be {"roots": [{"uri": "..."}]}.
        let roots = result["roots"].as_array().expect("roots array");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["uri"], "file:///Users/example/project");
    }

    #[tokio::test]
    async fn roots_list_uri_uses_literal_file_scheme() {
        let handler = RootsListHandler {
            cwd: PathBuf::from("/tmp/x"),
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

    use crate::hook_dispatch::{
        ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher,
    };
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
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick"})))
            .await;
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
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick"})))
            .await;
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
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick"})))
            .await;
        let result = resp.result.expect("success result");
        assert_eq!(result, json!({"action": "cancel"}));
    }

    #[tokio::test]
    async fn elicitation_no_dispatcher_is_unchanged_default() {
        // (d) No dispatcher => current default unchanged (strict no-op): the
        // handler never even looks at the params, returns {"action":"cancel"}.
        let handler = ElicitationCreateHandler::with_dispatcher("github", None);
        let resp = handler
            .handle(elicit_req(json!({"message": "Pick"})))
            .await;
        let result = resp.result.expect("success result");
        let bytes = serde_json::to_vec(&result).unwrap();
        assert_eq!(bytes, br#"{"action":"cancel"}"#);
    }
}
