//! Per-tool default Y/N decisions for the interactive permission prompt.
//!
//! Source-of-truth table — see M5-05 plan §"Tool default Y/N table" for the
//! claude-code references that justify each row. Unknown tool names default
//! to [`PromptDefault::DenyByDefault`] (fail-closed).
//!
//! Aggregate, oracle-parity set: 23 `DenyByDefault` (destructive / external
//! side-effects), 21 `AllowByDefault` (read-only or agent-local) = 44 tools,
//! plus one synthetic `<unknown>` fallback.
//!
//! MOBILE DIVERGENCE: 22 further `LocalApp*` rows for the first-party
//! local-app host operations (`engine_mobile::local_apps_tools`). They have no
//! oracle counterpart — claude-code has no host-owned local-app surface — and
//! are split by REVERSIBILITY: 11 `AllowByDefault` (read-only, plus the
//! network-disabled build and the restartable local preview runtime),
//! 11 `DenyByDefault` (user data, UI actuation, view capture, checkpoint
//! restore, network).
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::gate::PromptDefault;

static TOOL_DEFAULTS: OnceLock<HashMap<&'static str, PromptDefault>> = OnceLock::new();

fn init_defaults() -> HashMap<&'static str, PromptDefault> {
    use PromptDefault::{AllowByDefault, DenyByDefault};
    let mut m: HashMap<&'static str, PromptDefault> = HashMap::with_capacity(41);

    // Allow-by-default tools ([Y/n]) — 20 entries.
    m.insert("Agent", AllowByDefault);
    m.insert("AskUserQuestion", AllowByDefault);
    m.insert("Config", AllowByDefault);
    m.insert("CronList", AllowByDefault); // read-only: lists local scheduled jobs
    m.insert("EnterPlanMode", AllowByDefault);
    m.insert("ExitPlanMode", AllowByDefault);
    m.insert("Glob", AllowByDefault);
    m.insert("Grep", AllowByDefault);
    m.insert("LSP", AllowByDefault);
    // 2.1.232 `zy`. Read-only discovery, and its own `check_permissions`
    // answers Allow — but a tool ABSENT from this table falls to the
    // fail-closed `DenyByDefault`, which makes `read_only_default_auto_allows`
    // false and raises a prompt on every call. It was missed when the registry
    // count went 42 -> 43: the oracle side of that count still balanced only
    // because the legacy `Task` alias below occupies a slot the registry does
    // not have.
    m.insert("ListAgents", AllowByDefault);
    m.insert("Read", AllowByDefault);
    m.insert("SendUserMessage", AllowByDefault); // wire name of BriefTool (Brief alias)
    m.insert("Skill", AllowByDefault);
    m.insert("Sleep", AllowByDefault);
    m.insert("StructuredOutput", AllowByDefault);
    m.insert("Task", AllowByDefault); // legacy alias of Agent
    m.insert("TaskGet", AllowByDefault);
    m.insert("TaskList", AllowByDefault);
    m.insert("TaskOutput", AllowByDefault);
    m.insert("TodoWrite", AllowByDefault);
    m.insert("ToolSearch", AllowByDefault);

    // Deny-by-default tools ([y/N]) — 22 entries.
    m.insert("Bash", DenyByDefault);
    m.insert("Edit", DenyByDefault);
    m.insert("EnterWorktree", DenyByDefault);
    m.insert("ExitWorktree", DenyByDefault);
    m.insert("ListMcpResourcesTool", DenyByDefault);
    m.insert("MCP", DenyByDefault);
    m.insert("McpAuth", DenyByDefault);
    m.insert("NotebookEdit", DenyByDefault);
    m.insert("PowerShell", DenyByDefault);
    m.insert("REPL", DenyByDefault);
    m.insert("ReadMcpResourceTool", DenyByDefault);
    m.insert("RemoteTrigger", DenyByDefault);
    m.insert("CronCreate", DenyByDefault);
    m.insert("CronDelete", DenyByDefault);
    m.insert("SendMessage", DenyByDefault);
    m.insert("TaskCreate", DenyByDefault);
    m.insert("TaskStop", DenyByDefault);
    m.insert("TaskUpdate", DenyByDefault);
    m.insert("TeamCreate", DenyByDefault);
    m.insert("TeamDelete", DenyByDefault);
    m.insert("WebFetch", DenyByDefault);
    m.insert("WebSearch", DenyByDefault);
    m.insert("Write", DenyByDefault);
    // ---- MOBILE DIVERGENCE: first-party local-app host operations ----------
    // No oracle counterpart — claude-code has no host-owned local-app surface.
    // These are BUILTIN tools (see `engine_mobile::local_apps_tools`), not a
    // user-configured MCP server; while they were spelled `mcp__local_apps__*`
    // they matched nothing here and fell through to `DenyByDefault`, so the
    // create flow prompted on every step.
    //
    // Split by REVERSIBILITY, not read/write. The allow-by-default operations
    // still refine to Ask in `LocalAppTool::check_permissions` when a session
    // is not bound to an app workspace.
    m.insert("LocalAppList", AllowByDefault);
    m.insert("LocalAppGet", AllowByDefault);
    m.insert("LocalAppLogs", AllowByDefault);
    m.insert("LocalAppCheckpointList", AllowByDefault);
    // NOT auto-allowed: `read_app_events` DRAINS the unread queue and advances
    // a persisted cursor by default, so a speculative call permanently
    // consumes what the user's running app posted. `peek=true` is the
    // non-destructive form, but the defaults table is per-NAME, not per-input.
    m.insert("LocalAppEvents", DenyByDefault);
    m.insert("LocalAppBackgroundList", AllowByDefault);
    m.insert("LocalAppBackgroundStatus", AllowByDefault);
    // Writes, but app-local and trivially reversible: the build runs with the
    // network DISABLED into the app's own `dist/`, and the runtime is a local
    // preview server. These two are the hot loop of the create flow.
    m.insert("LocalAppBuild", AllowByDefault);
    m.insert("LocalAppRuntime", AllowByDefault);
    // The library's create sheet does NOT come through this gate: it sends
    // `ClientCommand::CreateApp` and creates the app outright, before any
    // conversation exists (see `client_protocol::version`). So the ONLY caller
    // this row governs is an agent reaching `LocalAppCreate` from a global or
    // project chat — which is exactly the case the deny exists for, and the
    // user is right there in that chat to answer.
    //
    // This was briefly `AllowByDefault` while the create sheet ran an intake
    // conversation and needed the agent to commit the create without a prompt.
    // That flow is gone; the exemption went with it.
    m.insert("LocalAppCreate", DenyByDefault);
    // Effects the user cannot trivially undo, or that reach the network.
    // These two expose an app's CONTENT — user records and the live WebView
    // DOM. Binding scopes them inside an app workspace, but a GLOBAL
    // conversation has no binding and can name any app, so they ask.
    m.insert("LocalAppQueryData", DenyByDefault);
    m.insert("LocalAppInspectUi", DenyByDefault);
    // Strictly more revealing than `LocalAppInspectUi`, which is already
    // DenyByDefault: the DOM snapshot nulls out `password`/`hidden` input
    // values and a pixel capture cannot redact anything it renders.
    m.insert("LocalAppCaptureUi", DenyByDefault);
    m.insert("LocalAppManifest", DenyByDefault);
    m.insert("LocalAppMutateData", DenyByDefault);
    m.insert("LocalAppActOnUi", DenyByDefault);
    m.insert("LocalAppCheckpointCreate", DenyByDefault);
    m.insert("LocalAppCheckpointRestore", DenyByDefault);
    m.insert("LocalAppInstallDeps", DenyByDefault);
    m.insert("LocalAppBackgroundSchedule", DenyByDefault);
    m.insert("LocalAppBackgroundCancel", DenyByDefault);
    m.insert("LocalAppBackgroundRetry", DenyByDefault);

    // 44 oracle-parity tools + 22 mobile local-app builtins.
    debug_assert_eq!(m.len(), 66, "tool defaults table must list all 66 tools");
    m
}

/// Look up the default Y/N decision for a tool name. Unknown tools → Deny.
#[must_use]
pub fn tool_default(name: &str) -> PromptDefault {
    TOOL_DEFAULTS
        .get_or_init(init_defaults)
        .get(name)
        .copied()
        .unwrap_or(PromptDefault::DenyByDefault)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local-app host operations are FIRST-PARTY builtin tools, not a
    /// user-configured MCP server. The split stays based on reversibility; the
    /// tool-level permission refinement adds the missing session-scope check
    /// for every allow-by-default operation that targets an app.
    #[test]
    fn local_app_tools_split_by_reversibility() {
        for name in [
            "LocalAppList",
            "LocalAppGet",
            "LocalAppLogs",
            "LocalAppCheckpointList",
            "LocalAppBackgroundList",
            "LocalAppBackgroundStatus",
            "LocalAppBuild",
            "LocalAppRuntime",
        ] {
            assert_eq!(
                tool_default(name),
                PromptDefault::AllowByDefault,
                "{name} should reach its tool-level scope refinement without a policy prompt"
            );
        }

        for name in [
            // The library's create sheet bypasses tools entirely; the only
            // caller here is an agent creating an app from a global or project
            // chat, with the user present to answer.
            "LocalAppCreate",
            // Expose app CONTENT; a global chat can name any app.
            "LocalAppQueryData",
            "LocalAppInspectUi",
            // Renders what inspect_ui redacts.
            "LocalAppCaptureUi",
            // Drains the unread queue and advances a persisted cursor.
            "LocalAppEvents",
            // Mutates the user's own records.
            "LocalAppMutateData",
            // Drives the app UI on the user's behalf.
            "LocalAppActOnUi",
            // Can discard uncommitted work.
            "LocalAppCheckpointRestore",
            "LocalAppCheckpointCreate",
            "LocalAppManifest",
            // Opens the network.
            "LocalAppInstallDeps",
            "LocalAppBackgroundSchedule",
            "LocalAppBackgroundCancel",
            "LocalAppBackgroundRetry",
        ] {
            assert_eq!(
                tool_default(name),
                PromptDefault::DenyByDefault,
                "{name} must still ask"
            );
        }
    }

    #[test]
    fn read_is_allow_by_default() {
        assert_eq!(tool_default("Read"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn bash_is_deny_by_default() {
        assert_eq!(tool_default("Bash"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn agent_is_allow_by_default() {
        assert_eq!(tool_default("Agent"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn write_edit_notebook_are_deny() {
        assert_eq!(tool_default("Write"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("Edit"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("NotebookEdit"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn web_tools_are_deny() {
        assert_eq!(tool_default("WebFetch"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("WebSearch"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn mcp_tools_are_deny() {
        assert_eq!(tool_default("MCP"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("McpAuth"), PromptDefault::DenyByDefault);
        assert_eq!(
            tool_default("ListMcpResourcesTool"),
            PromptDefault::DenyByDefault
        );
        assert_eq!(
            tool_default("ReadMcpResourceTool"),
            PromptDefault::DenyByDefault
        );
    }

    #[test]
    fn read_only_tools_are_allow() {
        assert_eq!(tool_default("Glob"), PromptDefault::AllowByDefault);
        assert_eq!(tool_default("Grep"), PromptDefault::AllowByDefault);
        assert_eq!(tool_default("LSP"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn unknown_tool_defaults_to_deny() {
        assert_eq!(tool_default("DoesNotExist"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default(""), PromptDefault::DenyByDefault);
    }

    #[test]
    fn table_splits_into_the_parity_set_and_the_mobile_divergence() {
        let m = init_defaults();
        // Splitting the count is strictly stronger than asserting the total:
        // it catches BOTH a dropped oracle tool and a mobile tool added
        // without being recorded as a divergence, which a single total hides.
        let oracle = m.keys().filter(|k| !k.starts_with("LocalApp")).count();
        let mobile = m.keys().filter(|k| k.starts_with("LocalApp")).count();
        assert_eq!(oracle, 44, "oracle-parity tool count changed");
        assert_eq!(mobile, 22, "local-app builtin count changed");
        assert_eq!(m.len(), oracle + mobile);
    }
}
