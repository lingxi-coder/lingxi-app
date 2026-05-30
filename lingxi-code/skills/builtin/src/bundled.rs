//! Compiled-in builtin skill templates.
//!
//! Empty in M8 — there are no Rust-bundled skills today (skills are markdown
//! loaded from disk by the session loader). Each entry is
//! `(canonical_name, raw_markdown)`; adding a template is a one-line const
//! addition, and `parse_builtin` in `lib.rs` turns it into a [`skill_api::Skill`].

/// Desktop builtin skill templates.
pub(crate) const BUILTIN_DESKTOP: &[(&str, &str)] = &[];

/// Mobile builtin skill templates.
pub(crate) const BUILTIN_MOBILE: &[(&str, &str)] = &[];
