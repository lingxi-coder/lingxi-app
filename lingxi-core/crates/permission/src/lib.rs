//! Permission engine: rule-driven authorization with mode fallback.
//!
//! M1.3 ships minimal `authorize`. Plan 03 (Tools) wires classifiers and
//! shadow detection.

#![forbid(unsafe_code)]

pub mod classifier;
pub mod dangerous_patterns;
pub mod denial_tracking;
pub mod mode;
pub mod policy;
pub mod result;
pub mod rule;
pub mod shadow;
pub mod update;

pub use mode::PermissionMode;
pub use policy::PermissionPolicy;
pub use result::{
    ClassifierKind, PermissionDecisionReason, PermissionResult, PermissionUpdateDestination,
    SandboxOverrideReason,
};
pub use rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
pub use update::PermissionUpdate;
