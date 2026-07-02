//! Auto-mode permission classifier.
//!
//! Claude Code's current public surface exposes `--permission-mode auto` and the
//! `auto-mode` inspection command. This module keeps the same local boundary:
//! the permission crate owns classifier semantics, while hosts only consume the
//! resulting allow/deny/pass verdict. The implementation is deterministic and
//! offline: it applies the shipped auto-mode policy categories to the tool call
//! shape instead of starting an LLM request from the permission layer.

pub use crate::result::ClassifierKind;

use crate::result::PermissionDecisionReason;
use crate::{read_only_command, shell_command};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Classifier score used for deterministic rule hits.
pub const RULE_MATCH_SCORE: f64 = 1.0;
/// Classifier score used for conservative local-operation allows.
pub const LOCAL_ALLOW_SCORE: f64 = 0.72;

/// Whether auto-mode classifier permissions are enabled.
#[must_use]
pub const fn is_classifier_permissions_enabled() -> bool {
    true
}

/// Structured result of the auto-mode classifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum AutoModeClassifierVerdict {
    /// The classifier approved the call.
    Allow {
        /// Confidence in `[0.0, 1.0]`.
        score: f64,
        /// Human-readable policy category/reason.
        reason: String,
    },
    /// The classifier blocked the call.
    Deny {
        /// Confidence in `[0.0, 1.0]`.
        score: f64,
        /// Human-readable policy category/reason.
        reason: String,
        /// `true` for hard-deny policy hits.
        hard: bool,
    },
    /// The classifier cannot safely decide; fall back to the prompt path.
    Pass {
        /// Why no automatic decision was made.
        reason: String,
    },
}

/// Should an `Ask` reason be eligible for auto-mode classifier resolution?
#[must_use]
pub fn reason_allows_classifier(reason: &PermissionDecisionReason) -> bool {
    match reason {
        PermissionDecisionReason::PermissionMode { mode } => *mode == crate::PermissionMode::Auto,
        PermissionDecisionReason::SafetyCheck {
            classifier_approvable,
            ..
        } => *classifier_approvable,
        _ => false,
    }
}

/// Classify one tool call for auto mode.
#[must_use]
pub fn classify_tool_call(tool_name: &str, input: &Value) -> AutoModeClassifierVerdict {
    match tool_name {
        "Bash" | "Shell" | "PowerShell" => classify_shell(input),
        "Read" | "Glob" | "Grep" | "LSP" | "LS" => allow("Read-Only Operations"),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => classify_file_mutation(input),
        "TodoWrite" | "ExitPlanMode" => allow("Local Operations"),
        "WebFetch" | "WebSearch" => classify_web(tool_name, input),
        "Agent" | "Task" => classify_agent(input),
        _ => AutoModeClassifierVerdict::Pass {
            reason: format!("No auto-mode classifier rule for {tool_name}"),
        },
    }
}

/// Local critique used by `lingxi-cli auto-mode critique`.
#[must_use]
pub fn critique_rules(value: &Value) -> Vec<String> {
    let mut findings = Vec::new();
    for key in ["allow", "soft_deny", "hard_deny", "environment"] {
        match value.get(key).and_then(Value::as_array) {
            Some(items) if items.is_empty() => {
                findings.push(format!("{key}: empty rule list"));
            }
            Some(items) => {
                let mut seen = std::collections::HashSet::new();
                for (idx, item) in items.iter().enumerate() {
                    let Some(rule) = item.as_str() else {
                        findings.push(format!("{key}[{idx}]: rule is not a string"));
                        continue;
                    };
                    let trimmed = rule.trim();
                    if trimmed.is_empty() {
                        findings.push(format!("{key}[{idx}]: rule is blank"));
                    }
                    if !trimmed.contains(':') && key != "environment" {
                        findings.push(format!("{key}[{idx}]: rule is missing a category prefix"));
                    }
                    if !seen.insert(trimmed.to_string()) {
                        findings.push(format!("{key}[{idx}]: duplicate rule"));
                    }
                }
            }
            None => findings.push(format!("{key}: missing rule list")),
        }
    }
    if findings.is_empty() {
        findings.push("No structural issues found in the auto-mode rules.".to_string());
    }
    findings
}

fn classify_shell(input: &Value) -> AutoModeClassifierVerdict {
    let Some(command) = shell_command::command_from_input(input).map(str::trim) else {
        return AutoModeClassifierVerdict::Pass {
            reason: "Shell command is missing".to_string(),
        };
    };
    if command.is_empty() {
        return AutoModeClassifierVerdict::Pass {
            reason: "Shell command is empty".to_string(),
        };
    }
    let lower = command.to_ascii_lowercase();
    if hard_shell_denial(&lower) {
        return deny_hard("Data Exfiltration / credential or persistence risk");
    }
    if soft_shell_denial(&lower) {
        return deny_soft("Auto-mode BLOCK policy matched shell command");
    }
    if read_only_command::command_is_read_only(command) {
        return allow("Read-Only Operations");
    }
    if local_shell_allow(&lower) {
        return AutoModeClassifierVerdict::Allow {
            score: LOCAL_ALLOW_SCORE,
            reason: "Local Operations".to_string(),
        };
    }
    AutoModeClassifierVerdict::Pass {
        reason: "Shell command needs user approval".to_string(),
    }
}

fn classify_file_mutation(input: &Value) -> AutoModeClassifierVerdict {
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .or_else(|| input.get("notebook_path"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    if lower.is_empty() {
        return AutoModeClassifierVerdict::Pass {
            reason: "File mutation is missing a path".to_string(),
        };
    }
    if is_self_modification_path(&lower) {
        return deny_soft("Self-Modification");
    }
    if looks_like_secret_path(&lower) {
        return deny_soft("Credential Leakage");
    }
    if lower.starts_with('/') || lower.contains("../") || lower == ".." {
        return AutoModeClassifierVerdict::Pass {
            reason: "File mutation path is outside classifier local-scope proof".to_string(),
        };
    }
    AutoModeClassifierVerdict::Allow {
        score: LOCAL_ALLOW_SCORE,
        reason: "Local Operations".to_string(),
    }
}

fn classify_web(tool_name: &str, input: &Value) -> AutoModeClassifierVerdict {
    let haystack = input.to_string().to_ascii_lowercase();
    if contains_any(
        &haystack,
        &["webhook.site", "oastify.com", "ngrok.io", "requestbin"],
    ) {
        return deny_soft("Exfil Scouting");
    }
    if tool_name == "WebFetch"
        && contains_any(&haystack, &["\"method\":\"get\"", "\"method\": \"get\""])
    {
        return allow("Read-Only Operations");
    }
    AutoModeClassifierVerdict::Pass {
        reason: "Network destination is not trusted by auto-mode environment".to_string(),
    }
}

fn classify_agent(input: &Value) -> AutoModeClassifierVerdict {
    let text = input.to_string().to_ascii_lowercase();
    if contains_any(
        &text,
        &[
            "dangerously-skip-permissions",
            "bypasspermissions",
            "no-sandbox",
            "disable approval",
        ],
    ) {
        deny_soft("Create Unsafe Agents")
    } else {
        AutoModeClassifierVerdict::Pass {
            reason: "Subagent request needs transcript-aware approval".to_string(),
        }
    }
}

fn hard_shell_denial(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "curl ",
            "wget ",
            "nc ",
            "netcat ",
            "scp ",
            "rsync ",
            "sftp ",
            "webhook.site",
            "oastify.com",
            "ngrok.io",
            "requestbin",
            "base64 ",
            "kubectl get secret",
            "aws secretsmanager",
            "gcloud secrets",
            "az keyvault secret",
            "printenv",
            "env |",
            "cat ~/.ssh",
            "cat .env",
        ],
    )
}

fn soft_shell_denial(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "git push --force",
            "git push -f",
            "git push origin main",
            "git push origin master",
            "git reset --hard",
            "git clean -fd",
            "git clean -xdf",
            "stash drop",
            "stash clear",
            "rm -rf",
            "rm -fr",
            "remove-item -recurse",
            "terraform destroy",
            "terraform apply -auto-approve",
            "pulumi destroy",
            "cdk destroy",
            "kubectl delete",
            "kubectl exec",
            "docker exec",
            "docker run -v /:",
            "ssh ",
            "chmod 777",
            "set-mppreference",
            "executionpolicy bypass",
            "curl | sh",
            "curl | bash",
            "wget | sh",
            "wget | bash",
            "invoke-expression",
            "iex ",
            "eval ",
            "crontab",
            "systemctl enable",
            "launchctl load",
        ],
    )
}

fn local_shell_allow(lower: &str) -> bool {
    let trimmed = lower.trim();
    if contains_any(trimmed, &["|", ">", "<", "$(", "`", "&&", ";"]) {
        return false;
    }
    starts_with_any(
        trimmed,
        &[
            "cargo test",
            "cargo check",
            "cargo build",
            "cargo fmt",
            "cargo clippy",
            "npm test",
            "npm run test",
            "npm run build",
            "npm install",
            "npm ci",
            "pnpm test",
            "pnpm run test",
            "pnpm run build",
            "pnpm install",
            "yarn test",
            "yarn build",
            "yarn install",
            "go test",
            "go build",
            "pytest",
            "python -m pytest",
            "uv run pytest",
            "mkdir ",
            "touch ",
        ],
    )
}

fn is_self_modification_path(lower: &str) -> bool {
    lower == "lingxi.md"
        || lower.ends_with("/lingxi.md")
        || lower == "lingxi.local.md"
        || lower.ends_with("/lingxi.local.md")
        || lower.contains("/.lingxi/settings")
        || lower.starts_with(".lingxi/settings")
        || lower.contains("/.lingxi/hooks/")
        || lower.contains("/.lingxi/agents/")
        || lower.contains("/.lingxi/skills/")
        || lower.contains("/.mcp.json")
        || lower.ends_with("/.mcp.json")
}

fn looks_like_secret_path(lower: &str) -> bool {
    lower.ends_with(".env")
        || lower.contains("/.env.")
        || lower.contains("secret")
        || lower.contains("credential")
        || lower.contains("private_key")
        || lower.contains("id_rsa")
}

fn allow(reason: &str) -> AutoModeClassifierVerdict {
    AutoModeClassifierVerdict::Allow {
        score: RULE_MATCH_SCORE,
        reason: reason.to_string(),
    }
}

fn deny_soft(reason: &str) -> AutoModeClassifierVerdict {
    AutoModeClassifierVerdict::Deny {
        score: RULE_MATCH_SCORE,
        reason: reason.to_string(),
        hard: false,
    }
}

fn deny_hard(reason: &str) -> AutoModeClassifierVerdict {
    AutoModeClassifierVerdict::Deny {
        score: RULE_MATCH_SCORE,
        reason: reason.to_string(),
        hard: true,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn starts_with_any(haystack: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| haystack.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifier_permissions_enabled() {
        assert!(is_classifier_permissions_enabled());
    }

    #[test]
    fn safe_local_build_is_allowed() {
        assert!(matches!(
            classify_tool_call("Bash", &json!({ "command": "cargo test -p permission" })),
            AutoModeClassifierVerdict::Allow { reason, .. } if reason == "Local Operations"
        ));
    }

    #[test]
    fn destructive_git_is_denied() {
        assert!(matches!(
            classify_tool_call("Bash", &json!({ "command": "git reset --hard" })),
            AutoModeClassifierVerdict::Deny { hard: false, reason, .. }
                if reason == "Auto-mode BLOCK policy matched shell command"
        ));
    }

    #[test]
    fn exfil_shape_is_hard_denied() {
        assert!(matches!(
            classify_tool_call("Bash", &json!({ "command": "cat .env | base64" })),
            AutoModeClassifierVerdict::Deny { hard: true, .. }
        ));
    }

    #[test]
    fn local_file_edit_is_allowed_but_config_edit_is_denied() {
        assert!(matches!(
            classify_tool_call("Edit", &json!({ "file_path": "src/lib.rs" })),
            AutoModeClassifierVerdict::Allow { .. }
        ));
        assert!(matches!(
            classify_tool_call("Edit", &json!({ "file_path": ".lingxi/settings.json" })),
            AutoModeClassifierVerdict::Deny { reason, .. } if reason == "Self-Modification"
        ));
    }

    #[test]
    fn explicit_rule_ask_is_not_classifier_eligible() {
        let rule = crate::PermissionRule {
            behavior: crate::PermissionBehavior::Ask,
            source: crate::PermissionRuleSource::UserSettings,
            value: crate::PermissionRuleValue {
                tool_name: "Bash".to_string(),
                rule_content: None,
            },
        };
        assert!(!reason_allows_classifier(
            &PermissionDecisionReason::MatchedRule { rule }
        ));
    }
}
