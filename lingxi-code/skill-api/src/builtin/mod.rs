//! Compiled-in builtin skill templates plus the `register_desktop` /
//! `register_mobile` entry points the composition roots
//! (`apps/engine-{desktop,mobile}`) call to assemble their skill set.
//!
//! Folded in from the former standalone `skill-builtin` crate (its only deps
//! were on this crate). Mobile additionally bundles the local-app coordinator
//! and its independent design, accessibility, React, and QA specialists so
//! they remain available inside the device sandbox.

mod bundled;

use crate::model::{LoadedFrom, SkillSource};
use crate::{parse_skill_markdown, Skill, SkillRegistry};
use bundled::{BundledSkill, BUILTIN_DESKTOP, BUILTIN_MOBILE};

/// Register the desktop builtin skill set into `reg`.
pub fn register_desktop(reg: &mut SkillRegistry) {
    register_slice(reg, BUILTIN_DESKTOP);
}

/// Register the mobile builtin skill set into `reg`.
pub fn register_mobile(reg: &mut SkillRegistry) {
    register_slice(reg, BUILTIN_MOBILE);
}

fn register_slice(reg: &mut SkillRegistry, slice: &[BundledSkill]) {
    for entry in slice {
        reg.register(parse_builtin(entry));
    }
}

/// Parse a compiled-in skill template with bundled provenance.
fn parse_builtin(entry: &BundledSkill) -> Skill {
    let mut skill = parse_skill_markdown(
        entry.raw,
        std::path::PathBuf::from(format!("<bundled:{}>", entry.name)),
        SkillSource::Bundled,
        LoadedFrom::Bundled,
    )
    .unwrap_or_else(|e| panic!("bundled skill `{}` failed to parse: {e}", entry.name));
    skill.frontmatter.triggers = entry
        .triggers
        .iter()
        .map(|trigger| (*trigger).into())
        .collect();
    skill
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_registry_matches_bundled_len() {
        let mut r = SkillRegistry::new();
        register_desktop(&mut r);
        assert_eq!(r.names().len(), BUILTIN_DESKTOP.len());
    }

    #[test]
    fn mobile_registry_matches_bundled_len() {
        let mut r = SkillRegistry::new();
        register_mobile(&mut r);
        assert_eq!(r.names().len(), BUILTIN_MOBILE.len());
        let mut names = r.names();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "accessibility",
                "create-local-app",
                "frontend-design",
                "frontend-qa",
                "react-best-practices",
            ]
        );
        for name in names {
            let skill = r.get(name).expect("registered bundled skill");
            assert_eq!(skill.source, SkillSource::Bundled);
            assert_eq!(skill.loaded_from, LoadedFrom::Bundled);
            assert_eq!(skill.frontmatter.name, name);
            assert!(!skill.description.is_empty());
        }
    }

    #[test]
    fn local_app_specialists_are_independently_discoverable() {
        let mut r = SkillRegistry::new();
        register_mobile(&mut r);
        for (query, expected) in [
            (
                "make a distinctive native frontend design",
                "frontend-design",
            ),
            ("run browser and webview frontend QA", "frontend-qa"),
            ("audit accessibility semantics", "accessibility"),
            ("improve React state and effects", "react-best-practices"),
        ] {
            assert!(
                r.discover(query).iter().any(|skill| skill.name == expected),
                "{expected} should be independently discoverable from {query:?}"
            );
        }
    }
}
