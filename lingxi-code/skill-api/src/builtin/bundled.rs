//! Compiled-in builtin skill templates.
//!
//! Each entry is
//! `(canonical_name, raw_markdown)`; adding a template is a one-line const
//! addition, and `parse_builtin` in `mod.rs` turns it into a [`crate::Skill`].

/// Desktop builtin skill templates.
pub(crate) const BUILTIN_DESKTOP: &[(&str, &str)] = &[];

/// Mobile builtin skill templates.
pub(crate) const BUILTIN_MOBILE: &[(&str, &str)] = &[(
    "create-local-app",
    include_str!("../../../../skills/create-local-app/SKILL.md"),
)];
