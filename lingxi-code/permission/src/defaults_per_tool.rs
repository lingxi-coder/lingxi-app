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
//! LINGXI DIVERGENCE: 35 further rows with no oracle counterpart, reported by
//! [`is_divergence_tool`]. 34 are the `LocalApp*` first-party local-app host
//! operations (`engine_mobile::local_apps_tools`) — claude-code has no
//! host-owned local-app surface. The 35th is `Workflow`: claude-code gates it
//! behind the `WORKFLOW_SCRIPTS` feature and it is absent from external builds
//! (see `mode_policy`'s module doc), so the M5-05 table has no row for it and
//! its default here is a LingXi decision, NOT oracle parity.
//!
//! The divergence rows are split by REVERSIBILITY: 14 `AllowByDefault`
//! (read-only, plus the network-disabled build, the restartable local preview
//! runtime, the shell-scaffolding commit, template-selection staging, and the
//! `Workflow` hand-off), 21 `DenyByDefault` (user data, UI actuation, view
//! capture, checkpoint restore, network, template validation, and the MCP
//! proposal / dependency-review lifecycle).
//!
//! 🚨 `AllowByDefault` is not merely a prompt default: it also short-circuits
//! the Plan-mode mutation backstop and the `DontAsk` ask→deny transform. Every
//! divergence row is therefore EXCLUDED from the Plan-mode auto-allow unless it
//! is plan-safe — see `policy_gate::read_only_default_auto_allows`, which keys
//! that carve-out on [`is_divergence_tool`].
//!
//! Both splits are asserted in
//! `tests::the_counts_in_this_module_doc_are_the_counts_in_the_table` — this
//! paragraph's four counts are asserted so additions cannot silently desync
//! the documentation from the table.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::gate::PromptDefault;

static TOOL_DEFAULTS: OnceLock<HashMap<&'static str, PromptDefault>> = OnceLock::new();

fn init_defaults() -> HashMap<&'static str, PromptDefault> {
    use PromptDefault::{AllowByDefault, DenyByDefault};
    let mut m: HashMap<&'static str, PromptDefault> = HashMap::with_capacity(77);

    // Allow-by-default tools ([Y/n]) — 21 entries. (It said 20 while there
    // were 21, from before `ListAgents` was added; the count is asserted in
    // `tests::the_counts_in_this_module_doc_are_the_counts_in_the_table` now.)
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

    // Deny-by-default tools ([y/N]) — 23 entries.
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

    // ---- LINGXI DIVERGENCE: the workflow-script launcher -------------------
    // NOT oracle parity: claude-code gates `Workflow` behind `WORKFLOW_SCRIPTS`
    // and external builds never advertise it (`mode_policy` module doc), so the
    // M5-05 table has no row to copy. Without a row `tool_default` fell through
    // to the fail-closed `DenyByDefault`, which prompted [y/N] on the single
    // hand-off of the whole create-app flow — unlike its siblings `Agent` and
    // `Skill`.
    //
    // Two disjoint launch shapes, both already gated below this row
    // (`WorkflowTool::check_permissions`, tools/workflow/src/lib.rs:1115):
    //   - a `scriptPath` launch is re-asked as the canonical `Read` tool on the
    //     resolved file, so a `Read(...)` rule and the symlink checks apply;
    //   - an inline / named launch reads no caller-selected file and self-allows
    //     with the reason "Workflow launch — spawned agents are individually
    //     permissioned".
    // They are the `else` and the `Some` arms of one `let`, so exactly one runs
    // per call; neither leaves the launch unchecked.
    //
    // `Workflow` is NOT plan-safe (`mode_policy::PLAN_SAFE_TOOLS`), so
    // [`is_divergence_tool`] keeps Plan mode prompting for it.
    m.insert("Workflow", AllowByDefault);

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
    // Read-only: the tool table (`local_apps_tools::LOCAL_APP_TOOLS`) marks
    // its `read_only` column `true`.
    m.insert("LocalAppRuntimeProfiles", AllowByDefault);
    m.insert("LocalAppTemplateCatalog", AllowByDefault);
    m.insert("LocalAppResolveTemplateSelection", AllowByDefault);
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
    // The ONE way out of an app the "+" button created as an empty shell:
    // until it succeeds, every build/dependency/runtime/UI operation on that
    // app refuses. Allowed by default because the user has just confirmed the
    // name, brief and shape IN THE CONVERSATION — a permission sheet on top of
    // that confirmation asks the same question twice. The scope check is not
    // waived, only the policy prompt: `LocalAppTool::check_permissions` still
    // refines this to Ask in a session that is not bound to an app workspace,
    // which is the case where the target id comes from the model rather than
    // from the user's own workspace.
    m.insert("LocalAppScaffold", AllowByDefault);
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
    m.insert("LocalAppValidateTemplateSelection", DenyByDefault);
    m.insert("LocalAppStageCreate", AllowByDefault);
    // MCP proposal lifecycle — the ONLY path by which network-reaching MCP
    // server configuration gets authored onto an app and promoted into its
    // live catalog, so every step of it asks. `approve_mcp_proposal` asks even
    // in its `create_without_mcp=true` branch, which authors no tools: that
    // branch still seals and persists a signed candidate + journal for the
    // app and drives the same approval surface, and the row is per-NAME, not
    // per-input, so it cannot be split by that flag.
    m.insert("LocalAppValidateMcpProposal", DenyByDefault);
    m.insert("LocalAppApproveMcpProposal", DenyByDefault);
    m.insert("LocalAppQaMcpCandidate", DenyByDefault);
    m.insert("LocalAppPromoteMcpCandidate", DenyByDefault);
    // r2-never-wired-02: newly wired into `local_apps_tools::LOCAL_APP_TOOLS`.
    // Same posture as the MCP proposal lifecycle above: a native confirmation
    // sheet already gates the actual dependency change/apply inside the
    // handler, but the row here is the POLICY prompt in front of that sheet,
    // and network-reaching dependency resolution is not trivially undone.
    m.insert("LocalAppConfirmDependencyChange", DenyByDefault);
    m.insert("LocalAppUpdateDependencies", DenyByDefault);
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

    // 44 oracle-parity tools + 35 LingXi divergence rows (34 local-app
    // builtins + `Workflow`).
    debug_assert_eq!(m.len(), 79, "tool defaults table must list all 79 tools");
    m
}

/// Does this row have NO claude-code counterpart?
///
/// True for the mobile `LocalApp*` family and for `Workflow` (gated behind
/// `WORKFLOW_SCRIPTS` upstream, absent from external builds). The oracle-parity
/// split in `tests::table_splits_into_the_parity_set_and_the_mobile_divergence`
/// keys on this, and so does the Plan-mode carve-out in
/// `policy_gate::read_only_default_auto_allows`: a divergence row that is not
/// plan-safe must not be auto-allowed while the user believes they are only
/// planning. Deliberately NOT extended to the oracle rows — several of them are
/// `AllowByDefault` without being plan-safe, and changing that would be a parity
/// change rather than a fix.
#[must_use]
pub fn is_divergence_tool(name: &str) -> bool {
    name.starts_with("LocalApp") || name == "Workflow"
}

/// Look up the row for a tool name, distinguishing "no row" from "a row that
/// happens to say Deny". `tool_default` collapses both to `DenyByDefault`,
/// which makes a missing row indistinguishable from a deliberate deny at that
/// call site — callers that need to catch a missing row (e.g. a cross-crate
/// guard test) must use this instead.
#[must_use]
pub fn tool_default_row(name: &str) -> Option<PromptDefault> {
    TOOL_DEFAULTS.get_or_init(init_defaults).get(name).copied()
}

/// Every tool name that has a row in this table, sorted.
///
/// r2-tests-honesty-012: this is the REVERSE direction of
/// [`tool_default_row`]. That answers "does THIS tool have a row"; nothing
/// could answer "does this ROW name a tool that still exists", because
/// `TOOL_DEFAULTS` is a private `static` and only single-key lookups were
/// exported. THREE guards do constrain this table's composition, not one:
/// `init_defaults`'s own `debug_assert_eq!(m.len(), 79, "tool defaults table
/// must list all 79 tools")`, the test
/// `table_splits_into_the_parity_set_and_the_mobile_divergence`'s
/// `oracle == 44` / `divergence == 35`, and the test
/// `the_counts_in_this_module_doc_are_the_counts_in_the_table`'s four
/// hand-bumped bucket counts (23/21/14/21). NONE of those seven numbers moves
/// for the orphan this function exists for, because every one of them counts
/// `TOOL_DEFAULTS` alone: delete a tool from a CONSUMER crate's table, leave
/// its row here, and all seven still hold. They also fail by naming a NUMBER
/// rather than the orphaned row, and they cannot see a consumer crate's tool
/// table in any case, since `permission` depends on none of them.
///
/// The consumer-side guard this exists for is
/// `engine_mobile::local_apps_tools::tests::every_local_app_permission_row_names_a_real_tool`.
#[must_use]
pub fn tool_default_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = TOOL_DEFAULTS
        .get_or_init(init_defaults)
        .keys()
        .copied()
        .collect();
    names.sort_unstable();
    names
}

/// Look up the default Y/N decision for a tool name. Unknown tools → Deny.
#[must_use]
pub fn tool_default(name: &str) -> PromptDefault {
    tool_default_row(name).unwrap_or(PromptDefault::DenyByDefault)
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
            "LocalAppTemplateCatalog",
            "LocalAppResolveTemplateSelection",
            "LocalAppStageCreate",
            "LocalAppLogs",
            "LocalAppCheckpointList",
            "LocalAppBackgroundList",
            "LocalAppBackgroundStatus",
            "LocalAppBuild",
            "LocalAppRuntime",
            // The shell's only way out; the user confirmed it in the chat.
            "LocalAppScaffold",
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
            "LocalAppValidateTemplateSelection",
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

    /// `Workflow` is the single hand-off of the create-app flow (and of any
    /// scripted multi-agent launch). `WorkflowTool::check_permissions` gates it
    /// below this row in EITHER of two mutually exclusive shapes — a
    /// `scriptPath` launch is re-asked as the canonical `Read` tool on the
    /// resolved file, an inline / named launch reads no caller-selected file and
    /// self-allows because the agents it spawns are individually permissioned —
    /// so a missing row here would double-prompt with no extra safety. A tool
    /// absent from this table falls through `tool_default` to the fail-closed
    /// `DenyByDefault`, unlike its siblings `Agent` and `Skill`.
    ///
    /// The Plan-mode consequence is covered by
    /// `policy_gate::plan_mode_divergence_test`.
    #[test]
    fn workflow_is_allow_by_default() {
        assert_eq!(tool_default("Workflow"), PromptDefault::AllowByDefault);
        assert!(
            is_divergence_tool("Workflow"),
            "Workflow is a divergence row, not oracle parity"
        );
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
        let oracle = m.keys().filter(|k| !is_divergence_tool(k)).count();
        let divergence = m.keys().filter(|k| is_divergence_tool(k)).count();
        assert_eq!(oracle, 44, "oracle-parity tool count changed");
        assert_eq!(divergence, 35, "divergence row count changed");
        assert_eq!(m.len(), oracle + divergence);
        // `Workflow` must be booked as a divergence, never as oracle parity:
        // the M5-05 table has no row for it.
        assert!(is_divergence_tool("Workflow"));
        assert!(!is_divergence_tool("Agent"));
    }

    /// The module doc states four counts. Assert each one so the documentation
    /// and table stay synchronized as local-app tools evolve.
    #[test]
    fn the_counts_in_this_module_doc_are_the_counts_in_the_table() {
        let m = init_defaults();
        let count = |divergence: bool, want: PromptDefault| {
            m.iter()
                .filter(|(name, value)| is_divergence_tool(name) == divergence && **value == want)
                .count()
        };
        assert_eq!(
            count(false, PromptDefault::DenyByDefault),
            23,
            "oracle deny"
        );
        assert_eq!(
            count(false, PromptDefault::AllowByDefault),
            21,
            "oracle allow"
        );
        assert_eq!(
            count(true, PromptDefault::AllowByDefault),
            14,
            "divergence allow"
        );
        assert_eq!(
            count(true, PromptDefault::DenyByDefault),
            21,
            "divergence deny"
        );
    }
}
