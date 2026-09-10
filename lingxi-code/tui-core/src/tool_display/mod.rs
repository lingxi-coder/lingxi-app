//! Transport-neutral derivation of how one tool call is presented.
//!
//! Everything here is plain data computed from `(tool_name, input_json,
//! result_json)`. It has no terminal dependency and no styling: the terminal
//! renders it through `crate::render`, and `client-adapter` lowers it onto the
//! wire for the iOS, Android, and Electron clients. Deriving it once is the
//! point — before this module each surface built its own header text and
//! result summary, and four implementations had already drifted apart.
//!
//! **This module must never grow a terminal-only dependency.** It lives under
//! `tui-core` because that crate is already on every consumer's link path
//! (`client-adapter` → `engine-mobile` → `ios-framework`/`android-aar`, plus
//! `bridge-server` and `tui`) and because the diff derivation it pairs with
//! needs `render::diff`'s private internals. It is deliberately NOT under
//! `render/`, where every module returns terminal-shaped `StyledLine`s.
//!
//! Three pieces:
//!
//! - [`header`] — the parameterized call header (`Update(src/host.rs)`).
//! - [`result`] — the `⎿` result headline and the expandable body.
//! - [`plan`] — the model-managed todo checklist pinned above the composer.

pub mod header;
pub mod plan;
pub mod result;

pub use header::{
    activity_label, tool_header, tool_header_with_result, tool_icon, ToolHeader, ToolIcon,
    ToolSubLine, ToolVerb,
};
pub use plan::{PlanTask, PlanTaskState};
pub use result::{added_removed_header, result_body, result_headline, result_is_error};
