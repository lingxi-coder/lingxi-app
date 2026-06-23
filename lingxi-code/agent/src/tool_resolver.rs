//! Resolve the tool set exposed to a subagent.
//!
//! [`AgentToolResolver`] projects the parent agent's tool set onto the child
//! according to the child's [`AgentToolPolicy`], appends the per-agent MCP
//! tools, drops the always-disallowed agent-tool set + the per-definition
//! denylist, and then applies the read-only filter when the child runs in
//! [`AgentPermissionMode::Plan`]. See spec §10.8.
//!
//! ## Always-disallowed default drop (claude `ALL_AGENT_DISALLOWED_TOOLS`)
//!
//! Mirroring claude-code `constants/tools.ts:36-46` + `filterToolsForAgent`
//! (`AgentTool/agentToolUtils.ts:70-116`), every subagent pool has the
//! agent-management / plan-mode / recursion tools stripped by default:
//! `Agent`, `TaskOutput`, `ExitPlanMode`, `EnterPlanMode`, `AskUserQuestion`,
//! `TaskStop`. The `Agent` AND `Workflow` entries are OMITTED when `USER_TYPE
//! === "ant"` so nested agents may spawn further agents / workflows (claude
//! v2.1.186 `HDd`: `...(USER_TYPE !== 'ant' ? [WORKFLOW_TOOL_NAME] : [])`,
//! mirroring the `Agent` gate). `Workflow` is now a registered LingXi tool, so
//! it is dropped for non-ant subagents to preserve the explicit-opt-in contract
//! and block recursive workflow fan-out.
//!
//! ## Per-definition `disallowedTools` subtraction (claude `resolveAgentTools`)
//!
//! After the always-disallowed drop, the agent definition's own
//! [`AgentDefinition::disallowed_tools`] (claude `disallowedTools` frontmatter,
//! `loadAgentsDir.ts:676-681`) is subtracted from the pool — see
//! `agentToolUtils.ts:149-160`. Each spec's trailing `(rule content)` is
//! stripped before comparison (claude `permissionRuleValueFromString`); LingXi
//! extracts the base tool name as the prefix before the first `(`.
//!
//! ## `Agent(x)` deny semantics are NOT handled here
//!
//! claude's `Agent(x)` rule content carries `allowedAgentTypes` metadata which
//! restricts WHICH agent TYPES may be launched — it operates on the spawnable
//! agent-type LIST (claude `filterDeniedAgents` / `getDenyRuleForAgent`,
//! `permissions.ts:308-343`), NOT on the tool pool. It never adds the `Agent`
//! tool to a child. LingXi's resolver does not model `allowedAgentTypes` at all
//! and so structurally cannot wrongly add `Agent` to a child based on it.

use crate::definition::{AgentDefinition, AgentPermissionMode, AgentToolPolicy};
use std::collections::HashSet;
use std::sync::Arc;
use tool_api::Tool;

/// Stateless utility that computes the effective tool set for an agent
/// spawn from the agent definition plus the surrounding tool sets.
pub struct AgentToolResolver;

impl AgentToolResolver {
    /// Tools every subagent has stripped by default, mirroring claude-code's
    /// `ALL_AGENT_DISALLOWED_TOOLS` (`constants/tools.ts:36-46`). `is_ant`
    /// gates the `Agent` entry: when `true` (claude `USER_TYPE === 'ant'`) the
    /// `Agent` tool is KEPT so nested agents may spawn further agents.
    ///
    /// The tool names are hardcoded string literals because the `agent` crate
    /// has no path-dep on `tools/*` (verified via `agent/Cargo.toml`), so the
    /// `*_TOOL_NAME` consts are unreachable — `builtins.rs` already hardcodes
    /// the same names. `Workflow` is dropped for non-ant subagents (claude
    /// v2.1.186 `HDd` ant-gates it exactly like `Agent`).
    #[must_use]
    pub fn all_agent_disallowed_tools(is_ant: bool) -> Vec<&'static str> {
        let mut names = vec![
            "TaskOutput",
            "ExitPlanMode",
            "EnterPlanMode",
            "AskUserQuestion",
            "TaskStop",
        ];
        // claude: `...(process.env.USER_TYPE === 'ant' ? [] : [AGENT_TOOL_NAME])`
        // and, in the same agent-disallowed set (binary v2.1.186 `HDd`/`nke`),
        // `...(USER_TYPE !== 'ant' ? [WORKFLOW_TOOL_NAME] : [])`. Both gate on the
        // EXACT `USER_TYPE === 'ant'` flag, so non-ant subagents may not recurse
        // into either `Agent` or `Workflow`; ant subagents keep both. The
        // `Workflow` tool is now registered in the LingXi registry (it was not
        // when this list was first written), so it MUST be dropped here to honour
        // the explicit-opt-in contract and prevent recursive workflow fan-out.
        if !is_ant {
            names.push("Agent");
            names.push("Workflow");
        }
        names
    }

    /// `true` when `USER_TYPE == "ant"` (claude `process.env.USER_TYPE ===
    /// 'ant'`, EXACT match — not the truthy allowlist). Matches the existing
    /// repo convention (`tools/task/src/task.rs`, `tools/meta/src/repl_gate.rs`).
    fn is_user_ant() -> bool {
        std::env::var("USER_TYPE").is_ok_and(|v| v == "ant")
    }

    /// Compute the effective tool list for a subagent.
    ///
    /// * `agent_def` — the spawning agent's definition (drives the policy +
    ///   the per-definition `disallowed_tools` denylist).
    /// * `parent_tools` — tools the parent agent had access to.
    /// * `agent_mcp_tools` — tools surfaced by the agent's MCP servers; these
    ///   ALWAYS pass (claude returns `true` for `mcp__` names before any
    ///   disallowed check, `agentToolUtils.ts:82-85`), so they are appended
    ///   AFTER the always-disallowed/per-definition drops.
    /// * `_coordinator_mode` — reserved; future coordinator-only filters
    ///   will land in Plan 07.
    ///
    /// Pipeline (claude `resolveAgentTools` order-equivalent):
    /// 1. policy projection ([`AgentToolPolicy`]) — only removes tools;
    /// 2. always-disallowed drop (`Agent`/`TaskOutput`/… gated by `USER_TYPE`);
    /// 3. per-definition `disallowed_tools` subtraction (base-name match);
    /// 4. append per-agent MCP tools (never filtered);
    /// 5. Plan-mode read-only narrowing (LingXi-local last step).
    ///
    /// ## `use_exact_tools` full bypass (claude `runAgent.ts:500-502`)
    ///
    /// When the policy is [`AgentToolPolicy::All`] with `use_exact_tools == true`
    /// (the synthetic `FORK_AGENT`), claude SKIPS `resolveAgentTools` /
    /// `filterToolsForAgent` entirely — `resolvedTools = availableTools` — so the
    /// fork child keeps the parent's EXACT unfiltered tool pool. This is
    /// load-bearing: it (a) preserves the cache-identical API prefix and (b)
    /// keeps the `Agent` tool the recursion guard (`isInForkChild`) assumes is
    /// present. We therefore return `parent_tools` verbatim with NO
    /// always-disallowed strip, NO per-definition subtraction, and NO Plan-mode
    /// narrowing. (Per-agent MCP tools are not appended on this path either:
    /// claude's fork passes `availableTools = toolUseContext.options.tools`
    /// untouched.)
    #[must_use]
    pub fn resolve(
        agent_def: &AgentDefinition,
        parent_tools: &[Arc<dyn Tool>],
        agent_mcp_tools: &[Arc<dyn Tool>],
        _coordinator_mode: bool,
    ) -> Vec<Arc<dyn Tool>> {
        // claude `runAgent.ts:500-502`: `useExactTools ? availableTools : …`.
        // The fork child bypasses ALL filtering, keeping the parent's exact pool.
        if let AgentToolPolicy::All {
            use_exact_tools: true,
        } = &agent_def.tools
        {
            return parent_tools.to_vec();
        }

        let mut tools = match &agent_def.tools {
            AgentToolPolicy::All { use_exact_tools: _ } => parent_tools.to_vec(),
            AgentToolPolicy::Explicit(names) => parent_tools
                .iter()
                .filter(|t| names.contains(&t.name().to_string()))
                .cloned()
                .collect(),
            AgentToolPolicy::Except(names) => parent_tools
                .iter()
                .filter(|t| !names.contains(&t.name().to_string()))
                .cloned()
                .collect(),
        };

        // (2) Always-disallowed default drop (claude filterToolsForAgent →
        // ALL_AGENT_DISALLOWED_TOOLS.has()). Runs for ALL policies because
        // claude's filterToolsForAgent runs on `availableTools` regardless of
        // the agent's `tools` policy. Applied BEFORE the MCP extend so
        // `mcp__*` tools are never touched (they are appended after).
        let disallowed = Self::all_agent_disallowed_tools(Self::is_user_ant());
        tools.retain(|t| !disallowed.contains(&t.name()));

        // (3) Per-definition `disallowedTools` subtraction (claude
        // resolveAgentTools disallowedToolSet, agentToolUtils.ts:149-160). Each
        // spec's trailing `(rule content)` is stripped to its base tool name
        // (claude `permissionRuleValueFromString` → `toolName`); the bare-name
        // case is what custom agents use in practice.
        if !agent_def.disallowed_tools.is_empty() {
            let def_disallowed: HashSet<&str> = agent_def
                .disallowed_tools
                .iter()
                .map(|spec| tool_name_from_spec(spec))
                .collect();
            tools.retain(|t| !def_disallowed.contains(t.name()));
        }

        // (4) Per-agent MCP tools always pass (claude returns true for
        // `mcp__*` before any disallowed check) — append after the drops.
        tools.extend(agent_mcp_tools.iter().cloned());

        // (5) Plan-mode read-only narrowing (LingXi-local last step; only
        // further narrows, so leaving it last is byte-safe).
        if agent_def.permission_mode == AgentPermissionMode::Plan {
            tools.retain(|t| {
                matches!(
                    t.name(),
                    "Read" | "Grep" | "Glob" | "WebSearch" | "WebFetch"
                )
            });
        }
        tools
    }
}

/// Extract the base tool name from a `disallowedTools` spec, stripping any
/// trailing `(rule content)` — mirrors claude `permissionRuleValueFromString`'s
/// `toolName` extraction (e.g. `"Bash(rm -rf)"` → `"Bash"`). The bare-name
/// case (`"Bash"`) is returned unchanged (trimmed).
fn tool_name_from_spec(spec: &str) -> &str {
    spec.split('(').next().unwrap_or(spec).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{AgentModel, AgentSource};
    use async_trait::async_trait;
    use serde_json::Value;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, ToolCallResult, ToolError, ToolStaticContext,
    };

    /// Minimal stub tool exposing only `name()` + `aliases()` (the surface the
    /// resolver inspects).
    struct StubTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &Value {
            static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({"type": "object"}))
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &Value) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
            self.name.into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            self.name.into()
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            unreachable!("not invoked in this test")
        }
    }

    fn tool(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(StubTool { name })
    }

    fn pool(names: &[&'static str]) -> Vec<Arc<dyn Tool>> {
        names.iter().map(|n| tool(n)).collect()
    }

    fn names(tools: &[Arc<dyn Tool>]) -> Vec<String> {
        tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Build an `AgentDefinition` with the given tool policy + spawn-path
    /// defaults.
    fn agent_def(tools: AgentToolPolicy) -> AgentDefinition {
        AgentDefinition {
            agent_type: "test".into(),
            when_to_use: String::new(),
            tools,
            max_turns: 1,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
        }
    }

    fn all_policy() -> AgentToolPolicy {
        AgentToolPolicy::All {
            use_exact_tools: false,
        }
    }

    // ── pure core: all_agent_disallowed_tools(is_ant) ──
    // Tested directly (no env) to avoid the process-global USER_TYPE race.

    #[test]
    fn core_non_ant_includes_agent() {
        let set = AgentToolResolver::all_agent_disallowed_tools(false);
        assert!(set.contains(&"Agent"));
        assert!(set.contains(&"TaskOutput"));
        assert!(set.contains(&"ExitPlanMode"));
        assert!(set.contains(&"EnterPlanMode"));
        assert!(set.contains(&"AskUserQuestion"));
        assert!(set.contains(&"TaskStop"));
        // non-ant: Workflow is dropped from subagent pools (binary HDd ant-gate).
        assert!(set.contains(&"Workflow"));
    }

    #[test]
    fn core_ant_omits_agent_keeps_rest() {
        let set = AgentToolResolver::all_agent_disallowed_tools(true);
        assert!(!set.contains(&"Agent"));
        // ant: Workflow is kept (allowed for ant subagents), like Agent.
        assert!(!set.contains(&"Workflow"));
        assert!(set.contains(&"TaskOutput"));
        assert!(set.contains(&"TaskStop"));
    }

    // ── resolve(): always-disallowed drop ──
    // NOTE: these rely on the default env (USER_TYPE unset). They must NOT run
    // concurrently with a test that sets USER_TYPE=ant in the same process; we
    // therefore test the ant branch via the pure core above, never via env.

    #[test]
    fn all_policy_strips_agent_by_default() {
        let parent = pool(&["Read", "Bash", "Agent"]);
        let resolved = AgentToolResolver::resolve(&agent_def(all_policy()), &parent, &[], false);
        let got = names(&resolved);
        assert!(!got.contains(&"Agent".to_string()), "Agent must be stripped");
        assert!(got.contains(&"Read".to_string()));
        assert!(got.contains(&"Bash".to_string()));
    }

    #[test]
    fn all_policy_strips_workflow_for_non_ant() {
        // The registered Workflow tool must not leak into subagent pools (binary
        // HDd: Workflow disallowed for USER_TYPE !== "ant").
        let parent = pool(&["Read", "Workflow", "Bash"]);
        let resolved = AgentToolResolver::resolve(&agent_def(all_policy()), &parent, &[], false);
        let got = names(&resolved);
        assert!(
            !got.contains(&"Workflow".to_string()),
            "Workflow must be stripped from non-ant subagents"
        );
        assert!(got.contains(&"Read".to_string()));
        assert!(got.contains(&"Bash".to_string()));
    }

    #[test]
    fn all_policy_strips_other_disallowed() {
        let parent = pool(&[
            "Read",
            "ExitPlanMode",
            "EnterPlanMode",
            "AskUserQuestion",
            "TaskStop",
            "TaskOutput",
        ]);
        let resolved = AgentToolResolver::resolve(&agent_def(all_policy()), &parent, &[], false);
        assert_eq!(names(&resolved), vec!["Read".to_string()]);
    }

    #[test]
    fn mcp_tools_always_survive() {
        // An mcp__ tool is appended AFTER the drop and is never filtered; the
        // parent-pool Agent tool is still dropped.
        let parent = pool(&["Read", "Agent"]);
        let mcp = pool(&["mcp__x__y"]);
        let resolved = AgentToolResolver::resolve(&agent_def(all_policy()), &parent, &mcp, false);
        let got = names(&resolved);
        assert!(got.contains(&"mcp__x__y".to_string()));
        assert!(!got.contains(&"Agent".to_string()));
        assert!(got.contains(&"Read".to_string()));
    }

    #[test]
    fn explicit_policy_still_strips_agent() {
        // Even when the agent explicitly lists Agent, the always-disallowed
        // drop removes it (claude filterToolsForAgent runs regardless of the
        // `tools` policy).
        let parent = pool(&["Read", "Agent"]);
        let def = agent_def(AgentToolPolicy::Explicit(vec![
            "Read".to_string(),
            "Agent".to_string(),
        ]));
        let resolved = AgentToolResolver::resolve(&def, &parent, &[], false);
        assert_eq!(names(&resolved), vec!["Read".to_string()]);
    }

    // ── resolve(): per-definition disallowed_tools subtraction ──

    #[test]
    fn disallowed_tools_subtracts() {
        let parent = pool(&["Read", "Bash"]);
        let mut def = agent_def(all_policy());
        def.disallowed_tools = vec!["Bash".to_string()];
        let resolved = AgentToolResolver::resolve(&def, &parent, &[], false);
        assert_eq!(names(&resolved), vec!["Read".to_string()]);
    }

    #[test]
    fn disallowed_tools_strips_rule_content() {
        // claude permissionRuleValueFromString strips the `(rule)` pattern; we
        // match on the base tool name.
        let parent = pool(&["Read", "Bash"]);
        let mut def = agent_def(all_policy());
        def.disallowed_tools = vec!["Bash(rm -rf)".to_string()];
        let resolved = AgentToolResolver::resolve(&def, &parent, &[], false);
        assert_eq!(names(&resolved), vec!["Read".to_string()]);
    }

    // ── resolve(): Plan-mode read-only narrowing still applies last ──

    #[test]
    fn plan_mode_readonly_narrowing_applies_last() {
        let parent = pool(&["Read", "Bash", "Grep", "Agent"]);
        let def = AgentDefinition {
            permission_mode: AgentPermissionMode::Plan,
            ..agent_def(all_policy())
        };
        let resolved = AgentToolResolver::resolve(&def, &parent, &[], false);
        let got = names(&resolved);
        // Agent dropped by the always-disallowed set; Bash dropped by the
        // Plan-mode read-only narrowing; only Read+Grep survive.
        assert!(!got.contains(&"Agent".to_string()));
        assert!(!got.contains(&"Bash".to_string()));
        assert!(got.contains(&"Read".to_string()));
        assert!(got.contains(&"Grep".to_string()));
    }

    #[test]
    fn tool_name_from_spec_extracts_base_name() {
        assert_eq!(tool_name_from_spec("Bash"), "Bash");
        assert_eq!(tool_name_from_spec("Bash(rm -rf)"), "Bash");
        assert_eq!(tool_name_from_spec("Read (foo)"), "Read");
    }

    // ── resolve(): use_exact_tools FULL bypass (claude runAgent.ts:500-502) ──
    // The fork child keeps the parent's EXACT unfiltered pool: no
    // always-disallowed strip, no per-definition subtraction, no Plan-mode
    // narrowing, no MCP append.

    fn exact_policy() -> AgentToolPolicy {
        AgentToolPolicy::All {
            use_exact_tools: true,
        }
    }

    #[test]
    fn use_exact_tools_keeps_agent_and_full_pool() {
        // With useExactTools the Agent / TaskOutput / etc. that resolve() would
        // otherwise strip are KEPT — the recursion guard relies on Agent being
        // present, and the cache prefix relies on the pool being identical.
        let parent = pool(&[
            "Read",
            "Bash",
            "Agent",
            "TaskOutput",
            "ExitPlanMode",
            "EnterPlanMode",
            "AskUserQuestion",
            "TaskStop",
        ]);
        let resolved = AgentToolResolver::resolve(&agent_def(exact_policy()), &parent, &[], false);
        // Child pool == parent pool, byte-for-byte (same order, same set).
        assert_eq!(names(&resolved), names(&parent));
    }

    #[test]
    fn use_exact_tools_ignores_per_definition_disallowed() {
        // Even a per-definition disallowedTools entry is bypassed on the exact
        // path (claude skips resolveAgentTools entirely).
        let parent = pool(&["Read", "Bash", "Agent"]);
        let mut def = agent_def(exact_policy());
        def.disallowed_tools = vec!["Bash".to_string()];
        let resolved = AgentToolResolver::resolve(&def, &parent, &[], false);
        assert_eq!(names(&resolved), names(&parent));
    }

    #[test]
    fn use_exact_tools_ignores_plan_mode_narrowing_and_mcp() {
        // Plan-mode narrowing and the MCP append are also bypassed: the child
        // pool is the parent pool verbatim regardless of permission mode, and
        // the fork path passes availableTools untouched (no MCP extend).
        let parent = pool(&["Read", "Bash", "Agent"]);
        let mcp = pool(&["mcp__x__y"]);
        let def = AgentDefinition {
            permission_mode: AgentPermissionMode::Plan,
            ..agent_def(exact_policy())
        };
        let resolved = AgentToolResolver::resolve(&def, &parent, &mcp, false);
        assert_eq!(names(&resolved), names(&parent));
        assert!(!names(&resolved).contains(&"mcp__x__y".to_string()));
    }

    #[test]
    fn use_exact_tools_false_still_filters() {
        // Sanity: the bypass is gated on use_exact_tools==true; the false
        // branch keeps the always-disallowed strip.
        let parent = pool(&["Read", "Agent"]);
        let resolved = AgentToolResolver::resolve(&agent_def(all_policy()), &parent, &[], false);
        assert!(!names(&resolved).contains(&"Agent".to_string()));
    }
}
