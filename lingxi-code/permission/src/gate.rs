//! Re-export shim — workspace-wide single source of truth lives in
//! [`traits::permission_gate`] and [`traits::prompting_gate`].
//! `lingxi-permission` re-exports so downstream crates only need to depend on
//! `lingxi-permission`, not on the traits crate directly.
#![forbid(unsafe_code)]

pub use traits::permission_gate::{
    PermissionDecision, PermissionDecisionSource, PermissionGate, PermissionResolution,
};
pub use traits::prompting_gate::{
    PermissionRequest, PermissionResponse, PromptDecision, PromptDefault, PromptError,
    PromptingGate,
};
