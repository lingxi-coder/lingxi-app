//! Permission engine: rule-driven authorization with mode fallback.
//!
//! M1.3 ships minimal `authorize`. Plan 03 (Tools) wires classifiers and
//! shadow detection.

#![forbid(unsafe_code)]

pub mod classifier;
pub mod dangerous_patterns;
pub mod defaults_per_tool;
pub mod denial_tracking;
pub mod gate;
pub mod loader;
pub mod mode;
pub mod policy;
pub mod prompting_gate;
pub mod result;
pub mod rule;
pub mod shadow;
pub mod update;

pub use defaults_per_tool::tool_default;
pub use gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PermissionResponse, PromptDecision,
    PromptDefault, PromptError, PromptingGate,
};
pub use loader::permission_rules_from_settings_json;
pub use mode::PermissionMode;
pub use policy::PermissionPolicy;
pub use prompting_gate::InteractivePromptingGate;
pub use result::{
    ClassifierKind, PermissionDecisionReason, PermissionResult, PermissionUpdateDestination,
    SandboxOverrideReason,
};
pub use rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
pub use update::PermissionUpdate;
