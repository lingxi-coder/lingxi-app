//! Re-export shim — workspace-wide single source of truth lives in
//! [`lingxi_traits::permission_gate`] and [`lingxi_traits::prompting_gate`].
//! `lingxi-permission` re-exports so downstream crates only need to depend on
//! `lingxi-permission`, not on the traits crate directly.
#![forbid(unsafe_code)]

pub use lingxi_traits::permission_gate::{PermissionDecision, PermissionGate};
pub use lingxi_traits::prompting_gate::{
    PermissionRequest, PermissionResponse, PromptDecision, PromptDefault, PromptError,
    PromptingGate,
};
