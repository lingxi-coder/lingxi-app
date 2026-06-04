//! Permission engine: rule-driven authorization with mode fallback.
//!
//! M1.3 ships minimal `authorize`. Plan 03 (Tools) wires classifiers and
//! shadow detection.

#![forbid(unsafe_code)]

pub mod auto_edit_safety;
pub mod classifier;
pub mod dangerous_patterns;
pub mod defaults_per_tool;
pub mod denial_tracking;
pub mod filesystem;
pub mod gate;
pub mod loader;
pub mod mode;
pub mod persist;
pub mod policy;
pub mod policy_gate;
pub mod prompting_gate;
pub mod result;
pub mod rule;
pub mod shadow;
pub mod shell_command;
pub mod shell_rule_matching;
pub mod update;

pub use auto_edit_safety::{
    check_path_safety_for_auto_edit, has_suspicious_windows_path_pattern,
    is_dangerous_file_path_to_auto_edit, normalize_case_for_comparison, AutoEditSafety,
    DANGEROUS_DIRECTORIES, DANGEROUS_FILES,
};
pub use classifier::is_classifier_permissions_enabled;
pub use defaults_per_tool::tool_default;
pub use filesystem::FsRoots;
pub use gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PermissionResponse, PromptDecision,
    PromptDefault, PromptError, PromptingGate,
};
pub use loader::{
    bypass_permissions_disabled_from_settings_json, default_mode_from_settings_json,
    permission_rules_from_settings_json,
};
pub use mode::{next_permission_mode, PermissionMode};
pub use persist::{persist_permission_update, PermissionPaths, PersistError};
pub use policy::PermissionPolicy;
pub use policy_gate::PolicyPermissionGate;
pub use prompting_gate::InteractivePromptingGate;
pub use result::{
    ClassifierKind, PermissionDecisionReason, PermissionResult, PermissionUpdateDestination,
    SandboxOverrideReason,
};
pub use rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
pub use shadow::{
    detect_unreachable_rules, is_shared_setting_source, ShadowType, UnreachableRule,
};
pub use update::PermissionUpdate;
