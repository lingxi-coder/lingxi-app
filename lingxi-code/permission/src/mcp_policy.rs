//! MCP-specific rule producers and tighten-only permission clamps.

use crate::result::PermissionPrompt;
use crate::{
    PermissionBehavior, PermissionDecisionReason, PermissionResult, PermissionRule,
    PermissionRuleSource, PermissionRuleValue,
};
use std::collections::BTreeMap;
use traits::{McpConfiguredToolPolicyDto, McpPermissionCeiling, McpToolPermissionPolicy};

fn behavior_from_policy(policy: McpToolPermissionPolicy) -> PermissionBehavior {
    match policy {
        McpToolPermissionPolicy::AlwaysAllow => PermissionBehavior::Allow,
        McpToolPermissionPolicy::AlwaysAsk => PermissionBehavior::Ask,
        McpToolPermissionPolicy::AlwaysDeny => PermissionBehavior::Deny,
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
    // `anthropic/requiresUserInteraction` is a tool-owned prompt requirement.
    // It tightens an otherwise-allowed call to Ask, while preserving any
    // existing Ask/Deny result and never widening a stricter ceiling.
    if out == PermissionBehavior::Allow && requires_user_interaction {
        out = PermissionBehavior::Ask;
    }
    if out != PermissionBehavior::Deny && !app_capability_authorized {
        out = PermissionBehavior::Deny;
    }
    out
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
        let Some(policy) = tool.permission_policy else {
            continue;
        };
        let tool_name = format!(
            "mcp__{normalized_server}__{}",
            protocol::normalize_name_for_mcp(&tool.name)
        );
        collapsed
            .entry(tool_name)
            .and_modify(|current| *current = current.strictest(policy))
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

#[cfg(test)]
mod tests {
    use super::{clamp_mcp_permission_result, permission_rules_from_mcp_tool_policies};
    use crate::result::{PermissionMetadata, PermissionPrompt};
    use crate::{
        PermissionBehavior, PermissionDecisionReason, PermissionMode, PermissionPolicy,
        PermissionResult, PermissionRuleSource,
    };
    use serde_json::json;
    use traits::{McpConfiguredToolPolicyDto, McpPermissionCeiling, McpToolPermissionPolicy};

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

    #[test]
    fn duplicate_permission_policy_entries_collapse_to_strictest_rule() {
        let rules = permission_rules_from_mcp_tool_policies(
            "my.server",
            &[
                McpConfiguredToolPolicyDto {
                    name: "write_file".into(),
                    permission_policy: Some(McpToolPermissionPolicy::AlwaysAllow),
                    org_max_permission: None,
                },
                McpConfiguredToolPolicyDto {
                    name: "write_file".into(),
                    permission_policy: Some(McpToolPermissionPolicy::AlwaysDeny),
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
                permission_policy: Some(McpToolPermissionPolicy::AlwaysDeny),
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
}
