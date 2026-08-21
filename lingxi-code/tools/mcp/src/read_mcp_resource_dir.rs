//! `ReadMcpResourceDirTool` (alias `ReadMcpResourceDir`) — 2.1.238 `Yme`
//! (`cc-238.js @224767196`), name `DSe` (`cc-238.js @222722746`).
//!
//! The third member of the MCP-resource trio. Like its two siblings it is NOT
//! in `getTools`' base list — `iJ` drops all three by name
//! (`let r=new Set([K8.name,Z8.name,Yme.name,Ky])`, `cc-238.js @230759264`) and
//! MCP discovery pushes them into the MCP tool partition only when a connected
//! server declares `capabilities.resources`
//! (`if(![K8,Z8].some((c)=>o.some((u)=>il(u,c.name))))o.push(K8,Z8,Yme)`,
//! `cc-238.js @225994181`). See `build_registered_mcp_tools`.
//!
//! The tool object declares no `isEnabled`, so `es()`'s default
//! `isEnabled:()=>!0` applies — once pushed, it is always advertised.
//!
//! ## Ported scope
//!
//! The oracle's `call` has three legs:
//!
//! 1. resolve the server (`Ami`) — must exist, be connected, and declare
//!    `capabilities.resources`;
//! 2. `if(!eA())` → `{resources:[],error:"Directory listing is not enabled in this build."}`
//!    where `eA(){return it("tengu_mcp_skills",!1)}` (`cc-238.js @224764491`) —
//!    a **default-false** gate, so this is the branch every shipped default
//!    build takes;
//! 3. `if(!serverDeclaresDirectoryRead(i.capabilities))` →
//!    `Server "<name>" does not support directory listing.`, then the paginated
//!    `resources/directory/read` RPC.
//!
//! Legs 1 and 2 are ported byte-exactly. Leg 3's *predicate* is
//! `caps.extensions["io.modelcontextprotocol/skills"].directoryRead === true`
//! (`FSf`, `cc-238.js @226219742`); the port's [`traits::ServerCapabilitiesDto`]
//! decodes only the four presence booleans plus `experimental`, so no server can
//! currently declare `directoryRead` here and the oracle's own
//! "does not support directory listing" branch is the correct answer for every
//! server the port can observe. DEFERRED, and deliberately: the
//! `resources/directory/read` RPC + the capability-`extensions` decode are a
//! separate subsystem (MCP skills) and `traits/` is outside this change's scope.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Model-facing name (2.1.238 `DSe`, `cc-238.js @222722746`).
pub const READ_MCP_RESOURCE_DIR_TOOL_NAME: &str = "ReadMcpResourceDirTool";

/// Legacy alias (`aliases:["ReadMcpResourceDir"]`). Already present in the
/// port's alias-normalisation tables (`permission/src/rule.rs`,
/// `llm-client/src/convert.rs`) — this is the implementation they pointed at.
pub const READ_MCP_RESOURCE_DIR_ALIAS: &str = "ReadMcpResourceDir";

/// The MCP-skills gate (`eA(){return it("tengu_mcp_skills",!1)}`). Default
/// FALSE, so the shipped binary's `call` returns [`NOT_ENABLED_ERROR`].
const MCP_SKILLS_GATE: &str = "tengu_mcp_skills";

/// Oracle `"inode/directory"` (`JBn`, `cc-238.js @222722720`).
pub const DIRECTORY_MIME_TYPE: &str = "inode/directory";

/// Oracle: `{data:{resources:[],error:"Directory listing is not enabled in this build."}}`.
const NOT_ENABLED_ERROR: &str = "Directory listing is not enabled in this build.";

/// Oracle `h_p` (`cc-238.js @222722775`). Note the LEADING newline — the
/// template literal opens with one — and the trailing newline before the
/// closing backtick.
const DESCRIPTION: &str = "
List the direct children of a directory resource on an MCP server.
- server: The name of the MCP server to read from
- uri: The URI of the directory resource

Only usable against a server that has declared support for directory listing. The listing is not recursive.
";

/// Oracle `g_p` (`cc-238.js @222723069`), with `${JBn}` interpolated.
const PROMPT: &str = "
List the direct children of a directory resource on an MCP server (`resources/directory/read`).

Parameters:
- server (required): The name of the MCP server to read from
- uri (required): The URI of the directory resource

The listing is not recursive. Each entry carries its own `uri`; subdirectories appear with mimeType \"inode/directory\" \u{2014} call this tool again on a subdirectory's `uri` to descend.

Only usable against a server that has declared support for directory listing; other servers return an error.
";

/// Oracle `$6S` (`cc-238.js @224766806`):
/// `be({server:H().describe("The MCP server name"),uri:H().describe("The directory resource URI to list")})`.
///
/// NOTE the key is `server`, not the `server_name` the port's two sibling
/// resource tools use — that divergence is pre-existing on those two and is not
/// replicated onto a tool being added fresh against the oracle.
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "server": { "type": "string", "description": "The MCP server name" },
            "uri":    { "type": "string", "description": "The directory resource URI to list" }
        },
        "required": ["server", "uri"]
    })
});

/// Oracle `B6S` (`cc-238.js @224766934`).
static OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "resources": {
                "type": "array",
                "description": "Direct children of the directory resource. Subdirectories appear with mimeType \"inode/directory\".",
                "items": {
                    "type": "object",
                    "properties": {
                        "uri":      { "type": "string", "description": "Child resource URI" },
                        "name":     { "type": "string", "description": "Child resource name" },
                        "mimeType": { "type": "string", "description": "Child MIME type" }
                    },
                    "required": ["uri", "name"]
                }
            },
            "error": {
                "type": "string",
                "description": "Human-readable error when the server could not list the directory"
            }
        },
        "required": ["resources"]
    })
});

/// `ReadMcpResourceDirTool` tool.
pub struct ReadMcpResourceDirTool {
    ctx: BuiltinToolContext,
}

impl ReadMcpResourceDirTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

/// Oracle `Ami(clients, name)` (`cc-238.js @224079500`) — resolve a server by
/// name, erroring with the oracle's three messages in the oracle's order.
/// Returns the server's raw display name on success.
async fn resolve_server(
    registry: &mcp::McpRegistry,
    requested: &str,
) -> Result<String, ToolError> {
    use mcp::McpConnectionState;
    let conns = registry.connections.read().await;
    let mut names: Vec<String> = conns.values().map(|s| s.name().to_string()).collect();
    names.sort();

    let matched = conns.values().find(|s| {
        s.name() == requested
            || mcp::normalization::normalize_name_for_mcp(s.name())
                == mcp::normalization::normalize_name_for_mcp(requested)
    });
    let Some(state) = matched else {
        return Err(ToolError::InvalidInput(format!(
            "Server \"{requested}\" not found. Available servers: {}",
            names.join(", ")
        )));
    };
    let name = state.name().to_string();
    let McpConnectionState::Connected { capabilities, .. } = state else {
        return Err(ToolError::InvalidInput(format!(
            "Server \"{name}\" is not connected"
        )));
    };
    if !capabilities.resources {
        return Err(ToolError::InvalidInput(format!(
            "Server \"{name}\" does not support resources"
        )));
    }
    Ok(name)
}

#[async_trait]
impl Tool for ReadMcpResourceDirTool {
    fn name(&self) -> &str {
        READ_MCP_RESOURCE_DIR_TOOL_NAME
    }

    fn aliases(&self) -> &[&str] {
        &[READ_MCP_RESOURCE_DIR_ALIAS]
    }

    /// Oracle `searchHint:"list the children of an MCP directory resource"`.
    fn search_hint(&self) -> Option<&str> {
        Some("list the children of an MCP directory resource")
    }

    fn user_facing_name(&self) -> Option<&str> {
        Some("readMcpResourceDir")
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&OUTPUT_SCHEMA)
    }

    /// Oracle `maxResultSizeChars:1e5`.
    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    /// Oracle `shouldDefer:!0`.
    fn should_defer(&self) -> bool {
        true
    }

    /// The object declares NO `isEnabled`, so `es()`'s default
    /// (`MFb = {isEnabled:()=>!0, …}`, `bin @284321674`) applies. Reachability
    /// is controlled entirely by whether MCP discovery pushed it into the tool
    /// partition, exactly as in the oracle.
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }

    /// Oracle `isReadOnly(){return!0}`.
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    /// Oracle `isConcurrencySafe(){return!0}`.
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "list MCP directory resource".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let server = input
            .get("server")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidInput(
                    "ReadMcpResourceDirTool: missing or non-string server".into(),
                )
            })?
            .to_string();
        // `uri` is destructured alongside `server` in the oracle; validated here
        // so a malformed call fails before the gate branch.
        input.get("uri").and_then(Value::as_str).ok_or_else(|| {
            ToolError::InvalidInput("ReadMcpResourceDirTool: missing or non-string uri".into())
        })?;

        let Some(registry) = self.ctx.mcp_registry.as_ref() else {
            return Err(ToolError::Io(
                "ReadMcpResourceDirTool: MCP registry not configured".into(),
            ));
        };

        // Leg 1 — oracle `let i=Ami(t,n)`, which THROWS before the gate check.
        let name = resolve_server(registry, &server).await?;

        // Leg 2 — oracle `if(!eA())return{data:{resources:[],error:…}}`.
        // `tengu_mcp_skills` is default-false, so this is the shipped default.
        if !telemetry::flag_bool(MCP_SKILLS_GATE, false) {
            return Ok(dir_error(NOT_ENABLED_ERROR.to_string()));
        }

        // Leg 3 — oracle
        // `if(!serverDeclaresDirectoryRead(i.capabilities))return{data:{resources:[],error:…}}`.
        // The port cannot observe `capabilities.extensions`, so no server
        // declares directoryRead and this branch always applies. See the module
        // doc: the RPC itself is a deliberate deferral.
        Ok(dir_error(format!(
            "Server \"{name}\" does not support directory listing."
        )))
    }
}

/// Oracle `mapToolResultToToolResultBlockParam`: when `error` is set the model
/// sees the error text alone.
fn dir_error(error: String) -> ToolCallResult {
    ToolCallResult {
        data: json!({ "resources": [], "error": error }),
        model_content: Some(error),
        new_messages: vec![],
        context_modifier: None,
        is_error: false,
        mcp_meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_byte_exact() {
        assert_eq!(READ_MCP_RESOURCE_DIR_TOOL_NAME, "ReadMcpResourceDirTool");
        assert_eq!(READ_MCP_RESOURCE_DIR_ALIAS, "ReadMcpResourceDir");
        assert_eq!(DIRECTORY_MIME_TYPE, "inode/directory");
    }

    /// The alias tables in `permission/src/rule.rs` and `llm-client/src/convert.rs`
    /// already normalise `ReadMcpResourceDir` onto `ReadMcpResourceDirTool`;
    /// this is the implementation they were pointing at.
    #[test]
    fn alias_matches_the_permission_normalisation_table() {
        assert_eq!(
            permission::rule::normalize_legacy_tool_name(READ_MCP_RESOURCE_DIR_ALIAS),
            READ_MCP_RESOURCE_DIR_TOOL_NAME
        );
    }

    /// 2.1.238 `h_p` (`cc-238.js @222722775`) — a template literal that opens
    /// AND closes with a newline.
    #[test]
    fn description_matches_the_oracle_template() {
        assert_eq!(
            DESCRIPTION,
            "\nList the direct children of a directory resource on an MCP server.\n\
             - server: The name of the MCP server to read from\n\
             - uri: The URI of the directory resource\n\
             \n\
             Only usable against a server that has declared support for directory listing. The listing is not recursive.\n"
        );
    }

    /// 2.1.238 `g_p` (`cc-238.js @222723069`) with `${JBn}` interpolated, em
    /// dash included.
    #[test]
    fn prompt_interpolates_the_directory_mime_type_and_keeps_the_em_dash() {
        assert!(PROMPT.starts_with(
            "\nList the direct children of a directory resource on an MCP server (`resources/directory/read`).\n"
        ));
        assert!(PROMPT.contains(&format!(
            "subdirectories appear with mimeType \"{DIRECTORY_MIME_TYPE}\" \u{2014} call this tool again on a subdirectory's `uri` to descend."
        )));
        assert!(PROMPT.ends_with(
            "Only usable against a server that has declared support for directory listing; other servers return an error.\n"
        ));
    }

    /// The default-false `tengu_mcp_skills` gate's error text.
    #[test]
    fn not_enabled_error_is_byte_exact() {
        assert_eq!(
            NOT_ENABLED_ERROR,
            "Directory listing is not enabled in this build."
        );
        let r = dir_error(NOT_ENABLED_ERROR.to_string());
        assert_eq!(
            r.model_content.as_deref(),
            Some("Directory listing is not enabled in this build.")
        );
        assert_eq!(r.data["resources"], json!([]));
        assert!(!r.is_error);
    }

    /// The input schema uses the ORACLE's `server`/`uri` keys.
    #[test]
    fn input_schema_uses_the_oracle_key_names() {
        let props = &INPUT_SCHEMA["properties"];
        assert!(props.get("server").is_some(), "oracle key is `server`");
        assert!(props.get("uri").is_some());
        assert!(
            props.get("server_name").is_none(),
            "`server_name` is the port's pre-existing divergence on the two sibling tools; \
             a freshly ported tool matches the oracle"
        );
    }
}
