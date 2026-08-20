//! First-party local-app host operations, registered as ORDINARY BUILTIN tools.
//!
//! These were previously reachable only as `mcp__local_apps__<op>`. That name
//! shape carried third-party-MCP permission semantics they were never meant to
//! have:
//!
//! - `permission::tool_default` is a flat table keyed by literal tool name with
//!   `.unwrap_or(DenyByDefault)`. No `mcp__…` name is in it, so EVERY call —
//!   `build`, `read_logs`, `manage_runtime` — raised a permission prompt. In
//!   the create flow that is a prompt per step.
//! - MCP rule matching is tool-wide: content matching is gated on Deny/Ask, so
//!   no allow rule could ever scope one of these to a single app. A grant
//!   written into one app's workspace settings meant "any app on this device,
//!   forever".
//!
//! They are first-party host operations that take `app_id` as a parameter —
//! the shape of a builtin tool, not of a user-configured MCP server. As
//! builtins they get ordinary permission semantics: a real entry in the
//! defaults table, and content matching, so `LocalAppBuild(<app_id>)` becomes
//! expressible.
//!
//! ## What deliberately stays on the MCP transport
//!
//! The DYNAMIC per-app tools (`mcp__local_apps__<app_id>__data_query`, …) are
//! NOT moved, for two independent reasons:
//!
//! 1. Their namespace is load-bearing: the provider binds `app_id` host-side
//!    and REJECTS a model-supplied one ("app_id is host-bound by the app MCP
//!    namespace"). That is the correct scoping mechanism and it already works.
//! 2. They must be registered at RUNTIME, when an app is created mid-session.
//!    `ToolRegistry::register_builtin` takes `&mut self` (construction only);
//!    only `register_mcp_tools(&self, …)` can add tools to a shared registry.

use std::sync::Arc;

use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::Value;
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

use crate::local_apps_mcp::LocalAppsMcpTransport;

/// `(builtin name, provider operation, read-only)` for every host operation.
///
/// `read_only` drives [`Tool::is_read_only`]/[`Tool::is_concurrency_safe`]; the
/// PROMPT default for each name lives in `permission::defaults_per_tool` — one
/// table for every tool in the product, rather than a second policy here.
pub const LOCAL_APP_TOOLS: &[(&str, &str, bool)] = &[
    // Read-only.
    ("LocalAppList", "list", true),
    ("LocalAppGet", "get", true),
    ("LocalAppLogs", "read_logs", true),
    ("LocalAppQueryData", "query_data", true),
    ("LocalAppCheckpointList", "list_checkpoints", true),
    // NOT read-only: `read_app_events` DRAINS the unread queue and advances a
    // persisted cursor unless `peek=true`. `is_read_only` reads the input to
    // honour `peek`; this flag is the DEFAULT for a call that omits it.
    ("LocalAppEvents", "read_app_events", false),
    ("LocalAppBackgroundList", "background_list", true),
    ("LocalAppBackgroundStatus", "background_status", true),
    ("LocalAppInspectUi", "inspect_ui", true),
    // Mutating.
    ("LocalAppBuild", "build", false),
    ("LocalAppInstallDeps", "install_dependencies", false),
    ("LocalAppRuntime", "manage_runtime", false),
    ("LocalAppCreate", "create", false),
    ("LocalAppManifest", "update_manifest", false),
    ("LocalAppMutateData", "mutate_data", false),
    ("LocalAppActOnUi", "act_on_ui", false),
    ("LocalAppCheckpointCreate", "create_checkpoint", false),
    ("LocalAppCheckpointRestore", "restore_checkpoint", false),
    ("LocalAppBackgroundSchedule", "background_schedule", false),
    ("LocalAppBackgroundCancel", "background_cancel", false),
    ("LocalAppBackgroundRetry", "background_retry", false),
];

/// One host operation exposed as a builtin tool.
///
/// The catalog entry (description + input schema) and the dispatch both come
/// from [`LocalAppsMcpTransport`], so a builtin and the provider can never
/// disagree about a schema or an argument.
pub struct LocalAppTool {
    /// Model-facing name, e.g. `LocalAppBuild`.
    name: &'static str,
    /// Provider operation this dispatches to, e.g. `build`.
    operation: &'static str,
    description: String,
    input_schema: Value,
    read_only: bool,
    /// The local app this SESSION is rooted in, resolved once at construction
    /// from the engine cwd.
    ///
    /// NOT from `ToolUseContext::cwd`: the main turn loop hard-codes that to
    /// `None` (`turn_loop.rs:2866` — "only an isolated subagent sets this"), so
    /// a call-time binding is inert in exactly the session that needs it. The
    /// engine cwd IS the app workspace for a `.localApp` conversation.
    session_app_id: Option<String>,
    /// Catalog `always_load == Some(false)` → this tool is DEFERRED, i.e. the
    /// model finds it through ToolSearch instead of carrying its description
    /// and schema in every request.
    defer: bool,
    search_hint: Option<String>,
    transport: Arc<LocalAppsMcpTransport>,
}

impl LocalAppTool {
    #[must_use]
    pub fn new(
        name: &'static str,
        operation: &'static str,
        description: String,
        input_schema: Value,
        read_only: bool,
        session_app_id: Option<String>,
        defer: bool,
        search_hint: Option<String>,
        transport: Arc<LocalAppsMcpTransport>,
    ) -> Self {
        Self {
            name,
            operation,
            description,
            input_schema,
            read_only,
            session_app_id,
            defer,
            search_hint,
            transport,
        }
    }
}

/// Build every host operation as a builtin tool, taking each description and
/// input schema from the provider's own catalog.
#[must_use]
pub fn local_app_builtin_tools(
    transport: &Arc<LocalAppsMcpTransport>,
    session_cwd: &std::path::Path,
) -> Vec<Arc<dyn Tool>> {
    let session_app_id = permission::local_app_id_for_root(session_cwd);
    let catalog = LocalAppsMcpTransport::host_tool_catalog();
    LOCAL_APP_TOOLS
        .iter()
        .filter_map(|&(name, operation, read_only)| {
            let entry = catalog.iter().find(|tool| tool.tool_name == operation)?;
            Some(Arc::new(LocalAppTool::new(
                name,
                operation,
                entry.description.clone(),
                entry.input_schema.clone(),
                read_only,
                session_app_id.clone(),
                // Carry the catalog's OWN loading decision. Dropping it made
                // every one of these ride eagerly in every mobile conversation
                // — including ones with no local app — and silently inverted
                // `LocalAppList`'s explicit "keep catalog discovery out of the
                // eager tool set".
                entry.always_load == Some(false),
                entry.search_hint.clone(),
                Arc::clone(transport),
            )) as Arc<dyn Tool>)
        })
        .collect()
}

impl LocalAppTool {
    /// The local app this session is rooted in, if any.
    fn bound_app_id(&self, ctx: &ToolUseContext) -> Option<String> {
        // Construction-time binding first (the main turn loop). An isolated
        // subagent DOES set `ctx.cwd`, so honour that when present.
        ctx.cwd
            .as_deref()
            .and_then(permission::local_app_id_for_root)
            .or_else(|| self.session_app_id.clone())
    }
}

#[async_trait::async_trait]
impl Tool for LocalAppTool {
    fn name(&self) -> &str {
        self.name
    }

    fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }

    /// NOT `is_mcp`. These are builtins now; reporting MCP here would put them
    /// back on the tool-wide-only matching path this move exists to leave.
    fn is_mcp(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        self.defer
    }

    fn search_hint(&self) -> Option<&str> {
        self.search_hint.as_deref()
    }

    fn max_result_size_chars(&self) -> usize {
        30_000
    }

    /// Same value the MCP adapter these replaced used — a large result is
    /// persisted rather than inlined.
    fn persistence_threshold(&self) -> Option<usize> {
        Some(100_000)
    }

    /// A long `LocalAppBuild` (30-minute budget) or `LocalAppInstallDeps` must
    /// be interruptible. The trait default is `Block`, under which the turn
    /// loop does NOT race the cancel token — ESC would leave the session
    /// parked until the budget expired. The MCP adapter these replaced
    /// returned `Cancel`; keep that.
    fn interrupt_behavior(&self, _input: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // TRUE for all of them, matching the MCP adapter these replaced.
        //
        // This answers "may this overlap other tools?", NOT "is this a
        // mutation". A single app's build/runtime mutations are already
        // serialized inside the service, so reporting the mutating ones unsafe
        // buys no safety and turns each into a full execution BARRIER: a turn
        // emitting [LocalAppBuild, Read, LocalAppLogs] would stall both cheap
        // reads behind the build's 30-minute budget.
        true
    }

    /// Input-aware, like `ConfigTool`/`ArtifactTool`: a per-tool flag cannot
    /// express `read_app_events`, which only leaves the cursor alone when the
    /// caller passes `peek=true`.
    fn is_read_only(&self, input: &Value) -> bool {
        if self.operation == "read_app_events" {
            return input
                .get("peek")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }
        self.read_only
    }

    /// The tool has NO opinion — the policy layer decides from the defaults
    /// table plus any matching rule. Same "allow-all-gate" convention the
    /// native file tools use; the real gate is `PolicyPermissionGate`.
    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-01 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        self.description.clone()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        self.description.clone()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // A `LocalAppBuild` runs on a 30-minute budget. Without a `started`
        // frame the row renders frozen for the whole of it — the MCP adapter
        // these replaced emitted one, so dropping it was a visible regression.
        // Best-effort `try_send`, like that adapter.
        if let Some(tool_use_id) = _ctx.tool_use_id.clone() {
            let _ = progress_tx.try_send(tool_api::progress::ToolProgress {
                tool_use_id,
                data: serde_json::json!({
                    "type": "local_app_progress",
                    "status": "started",
                    "tool": self.name,
                    "operation": self.operation,
                }),
            });
        }
        // HOST-BIND `app_id` to the session's own app.
        //
        // These tools are AUTO-ALLOWED by the defaults table, and the static
        // dispatch takes `app_id` from model input verbatim. Without this, a
        // session working on app A could read app B's records, logs, events or
        // live WebView with no prompt at all — strictly worse than the
        // `mcp__local_apps__*` spelling, which at least asked. The dynamic MCP
        // tools solve this by binding the id to their namespace; a builtin has
        // no namespace, so it binds from the session cwd instead.
        //
        // Only binds when the session IS a local-app workspace. A global
        // conversation legitimately addresses apps by id (that is how one gets
        // created), and there the ordinary permission gate is the control.
        let input = match self.bound_app_id(&_ctx) {
            None => input,
            Some(session_app) => match input.get("app_id") {
                // Absent → fill it in, so a workspace session never has to name
                // its own app.
                None => {
                    let mut bound = input;
                    if let Some(object) = bound.as_object_mut() {
                        object.insert("app_id".into(), Value::String(session_app));
                    }
                    bound
                }
                Some(Value::String(requested)) if *requested == session_app => input,
                // A mismatch OR a non-string (`null`, a number, an array) fails
                // CLOSED. Letting a non-string through would slip past both the
                // comparison and the fill-in and reach the provider unbound.
                other => {
                    return Err(ToolError::InvalidInput(format!(
                        "{}: app_id {other:?} does not belong to this workspace \
                         (bound to {session_app:?})",
                        self.name
                    )));
                }
            },
        };
        let result = self
            .transport
            .call_host_operation(self.operation, input)
            .await
            .map_err(|error| ToolError::Internal(error.to_string()))?;
        // The provider builds an MCP ENVELOPE: `content` is
        // `[{"type":"text","text":"<json>"}]` and `structured_content` carries
        // the real payload. Handing the envelope to `from_data` would make the
        // dispatch JSON-dump the array, so the model would receive
        // `[{"type":"text","text":"{\"records\":…}"}]` — a doubly-escaped
        // string — instead of `{"records":…}`. Every prompt that says "the
        // returned `records[].document` must contain the value" would then be
        // asking the model to read through two layers of escaping.
        //
        // Unwrap it: the structured payload is the data, and the envelope's
        // text is the model-facing rendering.
        let text = result
            .content
            .as_array()
            .and_then(|blocks| {
                let joined: Vec<&str> = blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                (!joined.is_empty()).then(|| joined.join("\n"))
            })
            .or_else(|| result.content.as_str().map(str::to_owned));
        let data = result
            .structured_content
            .clone()
            .or_else(|| text.clone().map(Value::String))
            .unwrap_or(Value::Null);
        let mut out = ToolCallResult::from_data(data);
        out.model_content = text;
        // Mirror the MCP contract: a logical failure is a flagged RESULT, not a
        // transport error, so the model sees it 1:1 with the former spelling.
        out.is_error = result.is_error;
        Ok(out)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry in the rename table must resolve to a real provider
    /// operation. A typo here would silently drop a tool: `local_app_builtin_tools`
    /// uses `filter_map`, so an unmatched operation yields FEWER tools rather
    /// than an error.
    #[test]
    fn every_declared_tool_maps_to_a_real_provider_operation() {
        let catalog = LocalAppsMcpTransport::host_tool_catalog();
        let ops: std::collections::BTreeSet<&str> =
            catalog.iter().map(|t| t.tool_name.as_str()).collect();
        for &(name, operation, _) in LOCAL_APP_TOOLS {
            assert!(
                ops.contains(operation),
                "{name} maps to unknown provider operation {operation:?}; \
                 known: {ops:?}"
            );
        }
    }

    /// The builder must produce one tool per declared entry. `filter_map`
    /// turns a bad mapping into a MISSING tool, which no other assertion here
    /// would notice.
    #[test]
    fn the_builder_produces_every_declared_tool() {
        let transport = Arc::new(LocalAppsMcpTransport::new(std::path::PathBuf::from("/tmp")));
        let built = local_app_builtin_tools(&transport, std::path::Path::new("/tmp"));
        assert_eq!(
            built.len(),
            LOCAL_APP_TOOLS.len(),
            "a declared tool was dropped by the catalog lookup"
        );
        let names: std::collections::BTreeSet<&str> =
            built.iter().map(|t| t.name()).collect();
        for &(name, _, _) in LOCAL_APP_TOOLS {
            assert!(names.contains(name), "{name} was not built");
        }
    }

    /// The catalog's own loading decision must survive the move. Dropping it
    /// made all 21 descriptions plus the very large `update_manifest` /
    /// `mutate_data` schemas ride in EVERY mobile conversation, and inverted
    /// `LocalAppList`'s explicit "keep catalog discovery out of the eager
    /// tool set".
    #[test]
    fn the_catalogs_loading_decision_survives_the_move() {
        let transport = Arc::new(LocalAppsMcpTransport::new(std::path::PathBuf::from("/tmp")));
        let built = local_app_builtin_tools(&transport, std::path::Path::new("/tmp"));
        let list = built
            .iter()
            .find(|t| t.name() == "LocalAppList")
            .expect("LocalAppList");
        assert!(list.should_defer(), "LocalAppList must stay deferred");
        assert_eq!(list.search_hint(), Some("discover existing local apps"));
    }

    /// The name table here and the rows in `permission::defaults_per_tool` are
    /// separate hand-written lists in different crates. Nothing else compares
    /// them, so renaming one side alone leaves every other guard green while
    /// `tool_default` silently falls back to `DenyByDefault` and the
    /// create-flow prompt storm returns.
    #[test]
    fn every_tool_has_a_permission_default_row() {
        for &(name, _, _) in LOCAL_APP_TOOLS {
            // A missing row is indistinguishable from a deliberate deny at the
            // lookup, so assert the row EXISTS by its documented value rather
            // than trusting the fallback.
            let actual = permission::tool_default(name);
            let expected_allow = matches!(
                name,
                "LocalAppList"
                    | "LocalAppGet"
                    | "LocalAppLogs"
                    | "LocalAppCheckpointList"
                    | "LocalAppBackgroundList"
                    | "LocalAppBackgroundStatus"
                    | "LocalAppBuild"
                    | "LocalAppRuntime"
            );
            let expected = if expected_allow {
                permission::PromptDefault::AllowByDefault
            } else {
                permission::PromptDefault::DenyByDefault
            };
            assert_eq!(actual, expected, "{name} has the wrong permission default");
        }
    }

    /// The lease allow-list is a THIRD hand-written copy of a subset of these
    /// names. A rename that misses it silently drops the build loop's lease
    /// authorization.
    #[test]
    fn the_lease_allowlist_names_are_real_tools() {
        let known: std::collections::BTreeSet<&str> =
            LOCAL_APP_TOOLS.iter().map(|&(name, _, _)| name).collect();
        for name in [
            "LocalAppBuild",
            "LocalAppLogs",
            "LocalAppRuntime",
            "LocalAppManifest",
            "LocalAppQueryData",
        ] {
            assert!(known.contains(name), "lease allow-list names unknown tool {name}");
        }
    }

    /// These are builtins, not MCP. Reporting `is_mcp` would put them back on
    /// the tool-wide-only matching path the move exists to leave, and would
    /// make `LocalAppBuild(<app_id>)` inexpressible again.
    #[test]
    fn the_tools_do_not_report_themselves_as_mcp() {
        let transport = Arc::new(LocalAppsMcpTransport::new(std::path::PathBuf::from("/tmp")));
        for tool in local_app_builtin_tools(&transport, std::path::Path::new("/tmp")) {
            assert!(!tool.is_mcp(), "{} must not report as MCP", tool.name());
        }
    }

    /// A workspace session must not reach a SIBLING app.
    ///
    /// These tools are auto-allowed by the defaults table and the static
    /// dispatch takes `app_id` from model input verbatim, so without host
    /// binding a session working on app-a could read app-b's records, logs,
    /// events or live WebView with no prompt — strictly worse than the
    /// `mcp__local_apps__*` spelling this replaced, which at least asked.
    #[tokio::test]
    async fn a_workspace_session_cannot_address_a_sibling_app() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("apps/app-a/workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let transport = Arc::new(LocalAppsMcpTransport::new(root.path().to_path_buf()));
        // Bind the way PRODUCTION does: from the engine cwd at construction.
        // The main turn loop leaves `ToolUseContext::cwd` as `None`, so a test
        // that hand-sets it would verify a path that never runs.
        let tools = local_app_builtin_tools(&transport, &workspace);
        let query = tools
            .iter()
            .find(|t| t.name() == "LocalAppQueryData")
            .expect("LocalAppQueryData");

        // Deliberately left as the main loop leaves it.
        let ctx = tool_api::test_support::fresh_ctx();
        assert!(ctx.cwd.is_none(), "the main turn loop supplies no cwd");
        let refused = query
            .call(
                serde_json::json!({"app_id": "app-b", "collection": "journal"}),
                ctx.clone(),
                tool_api::test_support::fresh_tx(),
            )
            .await;
        match refused {
            Err(ToolError::InvalidInput(message)) => {
                assert!(
                    message.contains("app-b") && message.contains("app-a"),
                    "the refusal must name both ids: {message}"
                );
            }
            other => panic!("a sibling app must be refused, got {other:?}"),
        }

        // Its OWN app is not refused by the binding (it fails later, on the
        // absent service — which proves the guard let it through).
        let own = query
            .call(
                serde_json::json!({"app_id": "app-a", "collection": "journal"}),
                ctx,
                tool_api::test_support::fresh_tx(),
            )
            .await;
        assert!(
            !matches!(&own, Err(ToolError::InvalidInput(m)) if m.contains("does not belong")),
            "the session's own app must pass the binding, got {own:?}"
        );
    }

    /// Read-only classification drives `is_read_only`/`is_concurrency_safe`,
    /// which the dispatcher uses to decide what may overlap.
    #[test]
    fn read_only_classification_reaches_the_tool() {
        let transport = Arc::new(LocalAppsMcpTransport::new(std::path::PathBuf::from("/tmp")));
        let built = local_app_builtin_tools(&transport, std::path::Path::new("/tmp"));
        let by_name: std::collections::BTreeMap<&str, &Arc<dyn Tool>> =
            built.iter().map(|t| (t.name(), t)).collect();
        let empty = serde_json::json!({});
        assert!(by_name["LocalAppLogs"].is_read_only(&empty));
        assert!(by_name["LocalAppQueryData"].is_read_only(&empty));
        assert!(!by_name["LocalAppBuild"].is_read_only(&empty));
        assert!(!by_name["LocalAppMutateData"].is_read_only(&empty));
    }
}
