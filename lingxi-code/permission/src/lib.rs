//! Permission engine: rule-driven authorization with mode fallback.
//!
//! M1.3 ships minimal `authorize`. Plan 03 (Tools) wires classifiers and
//! shadow detection.

#![forbid(unsafe_code)]

pub mod allow_suggestion;
pub mod auto_edit_safety;
pub mod bash_security;
#[cfg(feature = "bash-ast")]
pub mod bash_tree_sitter;
pub mod bypass_guard;
pub mod classifier;
pub mod cli_mode;
pub mod command_path_containment;
pub mod dangerous_patterns;
pub mod dangerous_perms;
pub mod dangerous_removal;
pub mod defaults_per_tool;
pub mod denial_tracking;
pub mod filesystem;
pub mod gate;
pub mod headless_gate;
pub mod loader;
pub mod mode;
pub mod mode_policy;
pub mod path_constraints;
pub mod persist;
pub mod policy;
pub mod policy_gate;
pub mod prompting_gate;
pub mod read_deny_globs;
pub mod read_only_command;
pub mod result;
pub mod rule;
pub mod sandbox_auto_allow;
pub mod sed_validation;
pub mod shadow;
pub mod shell_command;
pub mod shell_rule_matching;
pub mod update;

pub use auto_edit_safety::{
    check_path_safety_for_auto_edit, has_suspicious_windows_path_pattern,
    is_dangerous_file_path_to_auto_edit, normalize_case_for_comparison, AutoEditSafety,
    DANGEROUS_DIRECTORIES, DANGEROUS_FILES,
};
pub use bash_security::{bash_command_is_safe, BashSafetyVerdict};
pub use bypass_guard::{enforce_bypass_safety, BypassEnv};
pub use classifier::is_classifier_permissions_enabled;
pub use command_path_containment::check_command_path_containment;
pub use cli_mode::{initial_permission_mode_from_cli, permission_mode_from_cli_string, CliModeSettings};
pub use dangerous_patterns::{
    dangerous_bash_patterns, CROSS_PLATFORM_CODE_EXEC, POWERSHELL_DANGEROUS_PATTERNS,
};
pub use dangerous_perms::{
    find_dangerous_classifier_permissions, is_dangerous_bash_permission,
    is_dangerous_classifier_permission, is_dangerous_powershell_permission,
    is_dangerous_task_permission, DangerousPermissionInfo,
};
pub use dangerous_removal::{check_dangerous_removal, is_dangerous_removal_path, DangerousRemoval};
pub use defaults_per_tool::tool_default;
pub use filesystem::FsRoots;
pub use gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PermissionResponse, PromptDecision,
    PromptDefault, PromptError, PromptingGate,
};
pub use headless_gate::DenyOnAskGate;
pub use loader::{
    additional_directories_from_settings_json, bypass_permissions_disabled_from_settings_json,
    default_mode_from_settings_json, permission_rules_from_settings_json,
};
pub use mode::{next_permission_mode, PermissionMode};
pub use mode_policy::is_plan_safe_tool;
pub use path_constraints::{check_path_constraints, PathConstraintAsk};
pub use persist::{
    persist_permission_update, persist_workspace_directory, remove_permission_update,
    PermissionPaths, PersistError,
};
pub use policy::{tool_wide_name_matches, PermissionPolicy};
pub use policy_gate::PolicyPermissionGate;
pub use prompting_gate::InteractivePromptingGate;
pub use read_deny_globs::read_deny_exclude_globs;
pub use read_only_command::command_is_read_only;
pub use sandbox_auto_allow::SandboxAutoAllowConfig;
pub use result::{
    ClassifierKind, PermissionDecisionReason, PermissionResult, PermissionUpdateDestination,
    SandboxOverrideReason,
};
pub use allow_suggestion::{allow_suggestion, call_matches_rule};
pub use rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
pub use sed_validation::{
    sed_auto_allow_verdict, sed_constraint_verdict, SedVerdict, SED_ASK_MESSAGE, SED_ASK_REASON,
};
pub use shadow::{
    detect_unreachable_rules, is_shared_setting_source, ShadowType, UnreachableRule,
};
pub use update::PermissionUpdate;
