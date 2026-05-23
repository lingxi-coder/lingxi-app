//! Inbound JSON-RPC request handlers required by the
//! `{roots:{}, elicitation:{}}` capability declaration.
//!
//! Implementations match claude-code's defaults in
//! `services/mcp/client.ts` lines 1009-1018 (roots) and 1188-1197
//! (elicitation default cancel).

use std::path::PathBuf;

use async_trait::async_trait;
use lingxi_jsonrpc::{InboundHandler, Request, Response};
use serde_json::json;

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
pub struct ElicitationCreateHandler;

#[async_trait]
impl InboundHandler for ElicitationCreateHandler {
    async fn handle(&self, req: Request) -> Response {
        // claude-code default: deny all elicitations until UI layer overrides.
        Response::success(req.id, json!({ "action": "cancel" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_jsonrpc::Id;

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
        let handler = ElicitationCreateHandler;
        let resp = handler.handle(req("elicitation/create")).await;
        let result = resp.result.expect("success result");
        // EXACT shape {"action": "cancel"} — no extra fields.
        let obj = result.as_object().expect("object");
        assert_eq!(obj.len(), 1, "exactly one field");
        assert_eq!(obj["action"], "cancel");
    }

    #[tokio::test]
    async fn elicitation_create_raw_bytes_match() {
        let resp = ElicitationCreateHandler
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

        let r2 = ElicitationCreateHandler
            .handle(Request::new(
                "elicitation/create",
                None,
                Id::String("abc".into()),
            ))
            .await;
        assert_eq!(r2.id, Some(Id::String("abc".into())));
    }
}
