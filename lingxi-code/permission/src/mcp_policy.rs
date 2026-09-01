//! MCP per-tool permission policy producers and tighten-only permission clamps.
//!
//! Producer side is 1:1 with claude-code `LXe`/`tir` (offset ~154729744,
//! `session.js`) plus the org-admin ceiling (`Kp`, offset ~176360614; schema
//! `cP`, offset ~154584475/155746241).
//!
//! ## Oracle algorithm
//!
//! `LXe(servers)` walks every **`dynamic`-scope** `http`/`sse` server's
//! `tools[]` array (each entry `{name, permission_policy?}`), builds the FQN
//! `mcp__<normalize(server)>__<normalize(tool)>` (`xc`/`Ul`, using the SAME
//! [`protocol::normalize_name_for_mcp`] normalizer), and resolves duplicate
//! FQNs with **strictest-wins** via the ordinal `{always_allow:0,
//! always_ask:1, always_deny:2}`. `tir(session, servers)` merges the resulting
//! name lists into `session.{alwaysAllowRules,alwaysDenyRules,alwaysAskRules}.mcpServerPolicy`
//! — exactly the [`crate::PermissionRuleSource::McpServerPolicy`] bucket this
//! module targets.
//!
//! The org-admin ceiling (`tools[].org_max_permission`, `allow`|`ask`|`blocked`)
//! is a SEPARATE oracle mechanism: `Kp(tools)` folds it into the on-disk
//! `toolPermissions` map (only entries `!== "allow"`), which tool discovery
//! attaches as `mcpInfo.effectiveMaxPermission` and the invocation gate
//! consults directly. `permission/` cannot depend on the live MCP registry, so
//! this port rides the ceiling on the SAME
//! [`crate::PermissionRuleSource::McpServerPolicy`] bucket: a `blocked` ceiling
//! becomes a tool-wide DENY rule; an `ask` ceiling becomes a tool-wide ASK
//! rule. That reproduces precedence but not tool-list filtering.

use crate::result::PermissionPrompt;
use crate::{
    PermissionBehavior, PermissionDecisionReason, PermissionResult, PermissionRule,
    PermissionRuleSource, PermissionRuleValue,
};
use std::collections::{BTreeMap, HashMap};
use traits::{
    McpConfiguredToolPolicyDto, McpPermissionCeiling,
    McpToolPermissionPolicy as ConfiguredMcpToolPermissionPolicy,
};

/// `tools[].permission_policy` (oracle `cP`/`p()` schema). Declared per-tool on
/// a `dynamic`-scope `http`/`sse` server's `tools[]` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpToolPermissionPolicy {
    /// Tool-wide allow, no prompt.
    AlwaysAllow,
    /// Force a prompt on every call.
    AlwaysAsk,
    /// Tool-wide deny.
    AlwaysDeny,
}

impl From<ConfiguredMcpToolPermissionPolicy> for McpToolPermissionPolicy {
    fn from(value: ConfiguredMcpToolPermissionPolicy) -> Self {
        match value {
            ConfiguredMcpToolPermissionPolicy::AlwaysAllow => Self::AlwaysAllow,
            ConfiguredMcpToolPermissionPolicy::AlwaysAsk => Self::AlwaysAsk,
            ConfiguredMcpToolPermissionPolicy::AlwaysDeny => Self::AlwaysDeny,
        }
    }
}

/// `tools[].org_max_permission` / on-disk `toolPermissions` value (oracle
/// `LTt`: `["allow","ask","blocked"]`). "Org admin's per-tool ceiling."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpToolMaxPermission {
    /// No ceiling — the absence of a restriction (`Kp` never emits an entry
    /// for this case).
    Allow,
    /// Force a prompt regardless of mode (`isOrgAskCeiling`).
    Ask,
    /// Filter the tool out entirely (`hI`'s hard filter).
    Blocked,
}

/// One `tools[]` entry for a single server (oracle `cP`).
#[derive(Debug, Clone)]
pub struct McpServerToolDecl {
    /// Upstream (unprefixed) tool name.
    pub name: String,
    /// This tool's declared `permission_policy`, if any.
    pub permission_policy: Option<McpToolPermissionPolicy>,
    /// This tool's declared `org_max_permission`, if any.
    pub org_max_permission: Option<McpToolMaxPermission>,
}

/// The already-resolved view of one MCP server this module needs.
#[derive(Debug, Clone, Default)]
pub struct McpServerPolicyView {
    /// The server's configured name (unnormalized — normalized inside this
    /// module when building each FQN).
    pub server_name: String,
    /// `config.scope === "dynamic"` (`tir`'s filter).
    pub is_dynamic_scope: bool,
    /// Whether this server's transport is `http` or `sse`.
    pub is_http_or_sse: bool,
    /// This server's declared `tools[]` array.
    pub tools: Vec<McpServerToolDecl>,
    /// On-disk `toolPermissions` map, keyed by upstream tool name.
    pub tool_permissions: HashMap<String, McpToolMaxPermission>,
}

fn behavior_from_policy(policy: McpToolPermissionPolicy) -> PermissionBehavior {
    match policy {
        McpToolPermissionPolicy::AlwaysAllow => PermissionBehavior::Allow,
        McpToolPermissionPolicy::AlwaysAsk => PermissionBehavior::Ask,
        McpToolPermissionPolicy::AlwaysDeny => PermissionBehavior::Deny,
    }
}

fn policy_severity(policy: McpToolPermissionPolicy) -> u8 {
    match policy {
        McpToolPermissionPolicy::AlwaysAllow => 0,
        McpToolPermissionPolicy::AlwaysAsk => 1,
        McpToolPermissionPolicy::AlwaysDeny => 2,
    }
}

fn strictest_policy(
    current: McpToolPermissionPolicy,
    candidate: McpToolPermissionPolicy,
) -> McpToolPermissionPolicy {
    if policy_severity(candidate) > policy_severity(current) {
        candidate
    } else {
        current
    }
}

fn clamp_behavior(
    base: PermissionBehavior,
    ceiling: Option<McpPermissionCeiling>,
    requires_user_interaction: bool,
    app_capability_authorized: bool,
) -> PermissionBehavior {
    let mut out = match (base, ceiling.unwrap_or(McpPermissionCeiling::Allow)) {
        (PermissionBehavior::Deny, _) => PermissionBehavior::Deny,
        (_, McpPermissionCeiling::Deny) => PermissionBehavior::Deny,
        (PermissionBehavior::Ask, _) => PermissionBehavior::Ask,
        (_, McpPermissionCeiling::Ask) => PermissionBehavior::Ask,
        (PermissionBehavior::Allow, McpPermissionCeiling::Allow) => PermissionBehavior::Allow,
    };
    if out == PermissionBehavior::Allow && requires_user_interaction {
        out = PermissionBehavior::Ask;
    }
    if out != PermissionBehavior::Deny && !app_capability_authorized {
        out = PermissionBehavior::Deny;
    }
    out
}

fn ceiling_severity(ceiling: McpToolMaxPermission) -> Option<u8> {
    match ceiling {
        McpToolMaxPermission::Allow => None,
        McpToolMaxPermission::Ask => Some(1),
        McpToolMaxPermission::Blocked => Some(2),
    }
}

/// NOTE: `Blocked` maps to [`PermissionBehavior::Deny`], which is NOT what
/// `hI` does — the tool stays visible to the model; only the call is refused.
fn behavior_for_severity(severity: u8) -> PermissionBehavior {
    match severity {
        0 => PermissionBehavior::Allow,
        1 => PermissionBehavior::Ask,
        _ => PermissionBehavior::Deny,
    }
}

fn fqn(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        protocol::normalize_name_for_mcp(server_name),
        protocol::normalize_name_for_mcp(tool_name)
    )
}

/// Collapse config-side MCP `tools[].permission_policy` declarations into the
/// existing `PermissionRuleSource::McpServerPolicy` bucket.
#[must_use]
pub fn permission_rules_from_mcp_tool_policies(
    server_name: &str,
    tools: &[McpConfiguredToolPolicyDto],
) -> Vec<PermissionRule> {
    let normalized_server = protocol::normalize_name_for_mcp(server_name);
    let mut collapsed = BTreeMap::<String, McpToolPermissionPolicy>::new();
    for tool in tools {
        let Some(policy) = tool.permission_policy.map(Into::into) else {
            continue;
        };
        let tool_name = format!(
            "mcp__{normalized_server}__{}",
            protocol::normalize_name_for_mcp(&tool.name)
        );
        collapsed
            .entry(tool_name)
            .and_modify(|current| *current = strictest_policy(*current, policy))
            .or_insert(policy);
    }
    collapsed
        .into_iter()
        .map(|(tool_name, policy)| PermissionRule {
            value: PermissionRuleValue {
                tool_name,
                rule_content: None,
            },
            behavior: behavior_from_policy(policy),
            source: PermissionRuleSource::McpServerPolicy,
        })
        .collect()
}

/// Turn a resolved view of the MCP server set into the
/// [`PermissionRuleSource::McpServerPolicy`] rules `LXe`+`tir` would have
/// produced, extended with the org-admin ceiling folded into the same bucket.
#[must_use]
pub fn mcp_server_policy_rules(servers: &[McpServerPolicyView]) -> Vec<PermissionRule> {
    let mut severity: HashMap<String, u8> = HashMap::new();

    for server in servers {
        if !server.is_http_or_sse {
            continue;
        }
        if server.is_dynamic_scope {
            for tool in &server.tools {
                let Some(policy) = tool.permission_policy else {
                    continue;
                };
                let candidate = policy_severity(policy);
                let name = fqn(&server.server_name, &tool.name);
                let entry = severity.entry(name).or_insert(candidate);
                if candidate > *entry {
                    *entry = candidate;
                }
            }
        }
        for (tool_name, ceiling) in &server.tool_permissions {
            let Some(candidate) = ceiling_severity(*ceiling) else {
                continue;
            };
            let name = fqn(&server.server_name, tool_name);
            let entry = severity.entry(name).or_insert(candidate);
            if candidate > *entry {
                *entry = candidate;
            }
        }
    }

    let mut names: Vec<&String> = severity.keys().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| PermissionRule {
            value: PermissionRuleValue {
                tool_name: name.clone(),
                rule_content: None,
            },
            behavior: behavior_for_severity(severity[name]),
            source: PermissionRuleSource::McpServerPolicy,
        })
        .collect()
}

/// Apply Local App / server-org ceilings, tool-owned interaction requirements,
/// and independent app-capability authorization without ever widening
/// `result`.
#[must_use]
pub fn clamp_mcp_permission_result(
    result: PermissionResult,
    tool_name: &str,
    local_ceiling: Option<McpPermissionCeiling>,
    server_ceiling: Option<McpPermissionCeiling>,
    requires_user_interaction: bool,
    app_capability_authorized: bool,
) -> PermissionResult {
    let metadata = match &result {
        PermissionResult::Allow { metadata, .. }
        | PermissionResult::Ask { metadata, .. }
        | PermissionResult::Deny { metadata, .. } => metadata.clone(),
    };
    let base = match result {
        PermissionResult::Allow { .. } => PermissionBehavior::Allow,
        PermissionResult::Ask { .. } => PermissionBehavior::Ask,
        PermissionResult::Deny { .. } => PermissionBehavior::Deny,
    };
    let ceiling = match (local_ceiling, server_ceiling) {
        (Some(a), Some(b)) => Some(a.strictest(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let final_behavior = clamp_behavior(
        base,
        ceiling,
        requires_user_interaction,
        app_capability_authorized,
    );
    if final_behavior == base {
        return result;
    }
    let interaction_ask = final_behavior == PermissionBehavior::Ask
        && base == PermissionBehavior::Allow
        && requires_user_interaction;
    match final_behavior {
        PermissionBehavior::Allow => result,
        PermissionBehavior::Ask => PermissionResult::Ask {
            reason: if interaction_ask {
                PermissionDecisionReason::PermissionPromptTool {
                    tool_name: tool_name.to_string(),
                }
            } else {
                PermissionDecisionReason::Other {
                    reason: format!("MCP tool {tool_name} requires approval"),
                }
            },
            prompt: PermissionPrompt {
                title: "Permission required".into(),
                message: format!("MCP tool {tool_name} requires approval"),
                options: vec!["Allow once".into(), "Deny".into()],
            },
            pending_classifier_check: None,
            metadata,
        },
        PermissionBehavior::Deny => PermissionResult::Deny {
            reason: PermissionDecisionReason::Other {
                reason: format!("MCP tool {tool_name} is blocked"),
            },
            explanation: Some(format!("MCP tool {tool_name} is blocked")),
            metadata,
        },
    }
}

/// The matching diagnostic: `toolPermissions` was declared with entries, but
/// none of them named a tool the server actually advertised.
#[must_use]
pub fn upstream_name_drift_warning(
    server_name: &str,
    tool_permissions: &HashMap<String, McpToolMaxPermission>,
    discovered_tool_names: &[String],
) -> Option<String> {
    if tool_permissions.is_empty() {
        return None;
    }
    let any_matched = discovered_tool_names
        .iter()
        .any(|discovered| tool_permissions.contains_key(discovered));
    if any_matched {
        return None;
    }
    Some(format!(
        "[claudeai-mcp] {server_name}: toolPermissions has {} entries but none matched upstream tool names — backend name drift?",
        tool_permissions.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{PermissionMetadata, PermissionPrompt};
    use crate::{
        PermissionBehavior, PermissionDecisionReason, PermissionMode, PermissionPolicy,
        PermissionResult, PermissionRule, PermissionRuleSource,
    };
    use serde_json::json;
    use traits::{
        McpConfiguredToolPolicyDto, McpPermissionCeiling,
        McpToolPermissionPolicy as ConfiguredMcpToolPermissionPolicy,
    };

    fn allow_result() -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::PermissionMode {
                mode: PermissionMode::AcceptEdits,
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    fn view(
        server_name: &str,
        dynamic: bool,
        tools: Vec<(&str, Option<McpToolPermissionPolicy>)>,
    ) -> McpServerPolicyView {
        McpServerPolicyView {
            server_name: server_name.to_string(),
            is_dynamic_scope: dynamic,
            is_http_or_sse: true,
            tools: tools
                .into_iter()
                .map(|(name, permission_policy)| McpServerToolDecl {
                    name: name.to_string(),
                    permission_policy,
                    org_max_permission: None,
                })
                .collect(),
            tool_permissions: HashMap::new(),
        }
    }

    #[test]
    fn duplicate_permission_policy_entries_collapse_to_strictest_rule() {
        let rules = permission_rules_from_mcp_tool_policies(
            "my.server",
            &[
                McpConfiguredToolPolicyDto {
                    name: "write_file".into(),
                    permission_policy: Some(ConfiguredMcpToolPermissionPolicy::AlwaysAllow),
                    org_max_permission: None,
                },
                McpConfiguredToolPolicyDto {
                    name: "write_file".into(),
                    permission_policy: Some(ConfiguredMcpToolPermissionPolicy::AlwaysDeny),
                    org_max_permission: None,
                },
            ],
        );
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "mcp__my_server__write_file");
        assert_eq!(rules[0].behavior, PermissionBehavior::Deny);
        assert_eq!(rules[0].source, PermissionRuleSource::McpServerPolicy);
    }

    #[test]
    fn producer_rules_take_effect_through_existing_policy_bucket() {
        let rules = permission_rules_from_mcp_tool_policies(
            "srv",
            &[McpConfiguredToolPolicyDto {
                name: "danger".into(),
                permission_policy: Some(ConfiguredMcpToolPermissionPolicy::AlwaysDeny),
                org_max_permission: None,
            }],
        );
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, rules);
        assert!(matches!(
            policy.authorize("mcp__srv__danger", &json!({})),
            PermissionResult::Deny {
                reason: PermissionDecisionReason::MatchedRule { .. },
                ..
            }
        ));
    }

    #[test]
    fn always_allow_produces_an_allow_rule() {
        let rules = mcp_server_policy_rules(&[view(
            "srv",
            true,
            vec![("tool", Some(McpToolPermissionPolicy::AlwaysAllow))],
        )]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "mcp__srv__tool");
        assert_eq!(rules[0].behavior, PermissionBehavior::Allow);
        assert_eq!(rules[0].source, PermissionRuleSource::McpServerPolicy);
    }

    #[test]
    fn always_deny_produces_a_deny_rule() {
        let rules = mcp_server_policy_rules(&[view(
            "srv",
            true,
            vec![("tool", Some(McpToolPermissionPolicy::AlwaysDeny))],
        )]);
        assert_eq!(rules[0].behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn non_dynamic_scope_is_never_walked() {
        let rules = mcp_server_policy_rules(&[view(
            "srv",
            false,
            vec![("tool", Some(McpToolPermissionPolicy::AlwaysDeny))],
        )]);
        assert!(rules.is_empty());
    }

    #[test]
    fn non_http_sse_transport_is_skipped() {
        let mut v = view(
            "srv",
            true,
            vec![("tool", Some(McpToolPermissionPolicy::AlwaysDeny))],
        );
        v.is_http_or_sse = false;
        assert!(mcp_server_policy_rules(&[v]).is_empty());
    }

    #[test]
    fn duplicate_fqn_is_strictest_wins_regardless_of_order() {
        let rules = mcp_server_policy_rules(&[view(
            "srv",
            true,
            vec![
                ("my.tool", Some(McpToolPermissionPolicy::AlwaysDeny)),
                ("my_tool", Some(McpToolPermissionPolicy::AlwaysAllow)),
            ],
        )]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "mcp__srv__my_tool");
        assert_eq!(rules[0].behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn strictest_wins_the_other_order_too() {
        let rules = mcp_server_policy_rules(&[view(
            "srv",
            true,
            vec![
                ("tool", Some(McpToolPermissionPolicy::AlwaysAsk)),
                ("tool", Some(McpToolPermissionPolicy::AlwaysDeny)),
            ],
        )]);
        assert_eq!(rules[0].behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn tool_name_and_server_name_are_normalized_into_the_fqn() {
        let rules = mcp_server_policy_rules(&[view(
            "My Server",
            true,
            vec![("My Tool", Some(McpToolPermissionPolicy::AlwaysDeny))],
        )]);
        assert_eq!(rules[0].value.tool_name, "mcp__My_Server__My_Tool");
    }

    #[test]
    fn ceiling_blocked_produces_a_deny_rule_regardless_of_scope() {
        let mut v = view("srv", false, vec![]);
        v.tool_permissions
            .insert("tool".to_string(), McpToolMaxPermission::Blocked);
        let rules = mcp_server_policy_rules(&[v]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "mcp__srv__tool");
        assert_eq!(rules[0].behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn ceiling_ask_produces_an_ask_rule() {
        let mut v = view("srv", false, vec![]);
        v.tool_permissions
            .insert("tool".to_string(), McpToolMaxPermission::Ask);
        let rules = mcp_server_policy_rules(&[v]);
        assert_eq!(rules[0].behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn ceiling_allow_contributes_no_rule() {
        let mut v = view("srv", false, vec![]);
        v.tool_permissions
            .insert("tool".to_string(), McpToolMaxPermission::Allow);
        assert!(mcp_server_policy_rules(&[v]).is_empty());
    }

    #[test]
    fn ceiling_ask_beats_a_local_always_allow_on_the_same_tool() {
        let mut v = view(
            "srv",
            true,
            vec![("tool", Some(McpToolPermissionPolicy::AlwaysAllow))],
        );
        v.tool_permissions
            .insert("tool".to_string(), McpToolMaxPermission::Ask);
        let rules = mcp_server_policy_rules(&[v]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn ceiling_ask_beats_a_higher_priority_allow_rule() {
        let mut v = view("srv", false, vec![]);
        v.tool_permissions
            .insert("tool".to_string(), McpToolMaxPermission::Ask);
        let mut rules = mcp_server_policy_rules(&[v]);
        rules.push(PermissionRule::allow_tool_session("mcp__srv__tool"));
        let policy = PermissionPolicy::from_rules(PermissionMode::Auto, rules);
        let result = policy.authorize("mcp__srv__tool", &serde_json::json!({}));
        assert!(matches!(result, PermissionResult::Ask { .. }));
    }

    #[test]
    fn always_deny_and_blocked_survive_wildcard_allow_and_auto_mode() {
        let denied = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__danger",
            Some(McpPermissionCeiling::Deny),
            Some(McpPermissionCeiling::Allow),
            false,
            true,
        );
        assert!(matches!(denied, PermissionResult::Deny { .. }));

        let blocked = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__danger",
            None,
            Some(McpPermissionCeiling::Deny),
            false,
            true,
        );
        assert!(matches!(blocked, PermissionResult::Deny { .. }));
    }

    #[test]
    fn requires_user_interaction_produces_protected_ask() {
        let asked = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__interactive",
            None,
            None,
            true,
            true,
        );
        assert!(matches!(
            asked,
            PermissionResult::Ask {
                reason: PermissionDecisionReason::PermissionPromptTool { tool_name },
                ..
            } if tool_name == "mcp__srv__interactive"
        ));
    }

    #[test]
    fn deny_ceiling_precedes_requires_user_interaction() {
        let denied = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__interactive",
            Some(McpPermissionCeiling::Deny),
            None,
            true,
            true,
        );
        assert!(matches!(denied, PermissionResult::Deny { .. }));
    }

    #[test]
    fn ask_ceiling_and_requires_user_interaction_remain_ask() {
        let asked = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__interactive",
            Some(McpPermissionCeiling::Ask),
            None,
            true,
            true,
        );
        assert!(matches!(asked, PermissionResult::Ask { .. }));
    }

    #[test]
    fn app_capability_authorization_is_an_independent_deny() {
        let denied = clamp_mcp_permission_result(
            allow_result(),
            "mcp__srv__camera",
            None,
            None,
            false,
            false,
        );
        assert!(matches!(denied, PermissionResult::Deny { .. }));
    }

    #[test]
    fn existing_ask_is_not_widened_by_allow_ceiling() {
        let base = PermissionResult::Ask {
            reason: PermissionDecisionReason::Other {
                reason: "already asking".into(),
            },
            prompt: PermissionPrompt {
                title: "Permission required".into(),
                message: "already asking".into(),
                options: vec!["Allow once".into(), "Deny".into()],
            },
            pending_classifier_check: None,
            metadata: PermissionMetadata::default(),
        };
        let out = clamp_mcp_permission_result(
            base,
            "mcp__srv__camera",
            Some(McpPermissionCeiling::Allow),
            Some(McpPermissionCeiling::Allow),
            false,
            true,
        );
        assert!(matches!(out, PermissionResult::Ask { .. }));
    }

    #[test]
    fn drift_warning_fires_only_when_nothing_matched() {
        let mut perms = HashMap::new();
        perms.insert("renamed_tool".to_string(), McpToolMaxPermission::Ask);
        let warning = upstream_name_drift_warning("srv", &perms, &["current_tool".to_string()]);
        assert_eq!(
            warning.as_deref(),
            Some(
                "[claudeai-mcp] srv: toolPermissions has 1 entries but none matched upstream tool names — backend name drift?"
            )
        );
    }

    #[test]
    fn drift_warning_silent_on_any_match() {
        let mut perms = HashMap::new();
        perms.insert("tool".to_string(), McpToolMaxPermission::Ask);
        assert_eq!(
            upstream_name_drift_warning("srv", &perms, &["tool".to_string()]),
            None
        );
    }

    #[test]
    fn drift_warning_silent_when_empty() {
        assert_eq!(
            upstream_name_drift_warning("srv", &HashMap::new(), &[]),
            None
        );
    }
}
