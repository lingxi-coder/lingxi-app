//! Per-tool default Y/N decisions for the interactive permission prompt.
//!
//! Source-of-truth table — see M5-05 plan §"Tool default Y/N table" for the
//! claude-code references that justify each row. Unknown tool names default
//! to [`PromptDefault::DenyByDefault`] (fail-closed).
//!
//! Aggregate, oracle-parity set: 23 `DenyByDefault` (destructive / external
//! side-effects), 20 `AllowByDefault` (read-only or agent-local) = 43 tools,
//! plus one synthetic `<unknown>` fallback.
//!
//! MOBILE DIVERGENCE: 21 further `LocalApp*` rows for the first-party
//! local-app host operations (`engine_mobile::local_apps_tools`). They have no
//! oracle counterpart — claude-code has no host-owned local-app surface — and
//! are split by REVERSIBILITY: 11 `AllowByDefault` (read-only, plus the
//! network-disabled build and the restartable local preview runtime),
//! 10 `DenyByDefault` (user data, UI actuation, checkpoint restore, network).
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::gate::PromptDefault;

static TOOL_DEFAULTS: OnceLock<HashMap<&'static str, PromptDefault>> = OnceLock::new();

fn init_defaults() -> HashMap<&'static str, PromptDefault> {
    use PromptDefault::{AllowByDefault, DenyByDefault};
    let mut m: HashMap<&'static str, PromptDefault> = HashMap::with_capacity(41);

    // Allow-by-default tools ([Y/n]) — 19 entries.
    m.insert("Agent", AllowByDefault);
    m.insert("AskUserQuestion", AllowByDefault);
    m.insert("Config", AllowByDefault);
    m.insert("CronList", AllowByDefault); // read-only: lists local scheduled jobs
    m.insert("EnterPlanMode", AllowByDefault);
    m.insert("ExitPlanMode", AllowByDefault);
    m.insert("Glob", AllowByDefault);
    m.insert("Grep", AllowByDefault);
    m.insert("LSP", AllowByDefault);
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
    // Split by REVERSIBILITY, not read/write.
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
    // Effects the user cannot trivially undo, or that reach the network.
    // These two expose an app's CONTENT — user records and the live WebView
    // DOM. Binding scopes them inside an app workspace, but a GLOBAL
    // conversation has no binding and can name any app, so they ask.
    m.insert("LocalAppQueryData", DenyByDefault);
    m.insert("LocalAppInspectUi", DenyByDefault);
    m.insert("LocalAppCreate", DenyByDefault);
    m.insert("LocalAppManifest", DenyByDefault);
    m.insert("LocalAppMutateData", DenyByDefault);
    m.insert("LocalAppActOnUi", DenyByDefault);
    m.insert("LocalAppCheckpointCreate", DenyByDefault);
    m.insert("LocalAppCheckpointRestore", DenyByDefault);
    m.insert("LocalAppInstallDeps", DenyByDefault);
    m.insert("LocalAppBackgroundSchedule", DenyByDefault);
    m.insert("LocalAppBackgroundCancel", DenyByDefault);
    m.insert("LocalAppBackgroundRetry", DenyByDefault);

    // 43 oracle-parity tools + 21 mobile local-app builtins.
    debug_assert_eq!(m.len(), 64, "tool defaults table must list all 64 tools");
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
    /// user-configured MCP server. While they were spelled
    /// `mcp__local_apps__*` they matched no table entry and fell through to
    /// `DenyByDefault`, so the create flow raised a prompt on every build,
    /// log read and runtime start.
    ///
    /// The split is by REVERSIBILITY, not by read/write: an operation whose
    /// effect the user can trivially undo (rebuild, restart) auto-allows;
    /// anything that touches user data, drives the UI on the user's behalf,
    /// discards uncommitted work, or opens the network still asks.
    #[test]
    fn local_app_tools_split_by_reversibility() {
        for name in [
            "LocalAppList",
            "LocalAppGet",
            "LocalAppLogs",
            "LocalAppCheckpointList",
            "LocalAppBackgroundList",
            "LocalAppBackgroundStatus",
            // Writes, but app-local and trivially reversible: the build runs
            // with the network DISABLED and only writes the app's own `dist/`,
            // and the runtime is a local preview server that can be restarted.
            "LocalAppBuild",
            "LocalAppRuntime",
        ] {
            assert_eq!(
                tool_default(name),
                PromptDefault::AllowByDefault,
                "{name} should not prompt"
            );
        }

        for name in [
            "LocalAppCreate",
            // Expose app CONTENT; a global chat can name any app.
            "LocalAppQueryData",
            "LocalAppInspectUi",
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
        assert_eq!(oracle, 43, "oracle-parity tool count changed");
        assert_eq!(mobile, 21, "local-app builtin count changed");
        assert_eq!(m.len(), oracle + mobile);
    }
}
