//! `skill-builtin` (M8-P8) — compiled-in builtin skill templates plus the
//! `register_desktop` / `register_mobile` entry points the composition roots
//! (`apps/engine-{desktop,mobile}`) call to assemble their skill set.
//!
//! There are no Rust-bundled skills today (skills are markdown loaded from
//! disk), so [`bundled::BUILTIN_DESKTOP`] / [`bundled::BUILTIN_MOBILE`] are
//! empty and both registration entry points produce an empty registry. The
//! loader plumbing (`parse_builtin`) is in place so adding a template later is
//! a one-line const entry — no wiring changes.

#![forbid(unsafe_code)]

mod bundled;

use bundled::{BUILTIN_DESKTOP, BUILTIN_MOBILE};
use skill_api::model::{LoadedFrom, SkillSource};
use skill_api::{parse_skill_markdown, Skill, SkillRegistry};

/// Register the desktop builtin skill set into `reg`.
pub fn register_desktop(reg: &mut SkillRegistry) {
    register_slice(reg, BUILTIN_DESKTOP);
}

/// Register the mobile builtin skill set into `reg`.
pub fn register_mobile(reg: &mut SkillRegistry) {
    register_slice(reg, BUILTIN_MOBILE);
}

fn register_slice(reg: &mut SkillRegistry, slice: &[(&str, &str)]) {
    for (name, raw) in slice {
        reg.register(parse_builtin(raw, name));
    }
}

/// Parse a compiled-in skill template with bundled provenance.
fn parse_builtin(raw: &str, name: &str) -> Skill {
    parse_skill_markdown(
        raw,
        std::path::PathBuf::from(format!("<bundled:{name}>")),
        SkillSource::Bundled,
        LoadedFrom::Bundled,
    )
    .unwrap_or_else(|e| panic!("bundled skill `{name}` failed to parse: {e}"))
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
    }
}
