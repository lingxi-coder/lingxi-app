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
    if !entry.references.is_empty() {
        skill.content.push_str(
            "\n\n---\n\n## Bundled references\n\nEach heading below preserves the original relative resource path. Follow `references/router.md` first, then apply only the profiles that router matches to the confirmed surface and targets; do not apply unrelated profiles.\n",
        );
    }
    for reference in entry.references {
        skill.content.push_str("\n\n---\n\n");
        skill
            .content
            .push_str(&format!("## Bundled resource: `{}`\n\n", reference.path));
        skill.content.push_str(reference.content);
    }
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
        let mut names = r.names();
        names.sort_unstable();
        assert_eq!(names, vec!["claude-api"]);
        let skill = r.get("claude-api").expect("registered bundled skill");
        assert_eq!(skill.source, SkillSource::Bundled);
        assert_eq!(skill.loaded_from, LoadedFrom::Bundled);
        assert_eq!(skill.frontmatter.name, "claude-api");
        assert!(skill.description.contains("Anthropic SDKs"));
        assert!(skill.content.contains("/claude-api upgrade python"));
        assert!(skill.content.contains("anthropic.Timeout"));
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
                "babylon-3d-local-app",
                "canvas-2d-local-app",
                "create-local-app",
                "frontend-design",
                "frontend-qa",
                "ionic-react-local-app",
                "phaser-2d-local-app",
                "react-best-practices",
                "threejs-local-app",
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
            (
                "build an Ionic React routed DOM app",
                "ionic-react-local-app",
            ),
            ("build a Canvas2D game", "canvas-2d-local-app"),
            ("build a Three.js 3D scene", "threejs-local-app"),
            ("build a Phaser arcade game", "phaser-2d-local-app"),
            ("build a Babylon 3D scene", "babylon-3d-local-app"),
        ] {
            assert!(
                r.discover(query).iter().any(|skill| skill.name == expected),
                "{expected} should be independently discoverable from {query:?}"
            );
        }
    }

    #[test]
    fn bundled_references_are_compiled_into_skill_content_without_agent_prompts() {
        let mut r = SkillRegistry::new();
        register_mobile(&mut r);

        for entry in bundled::BUILTIN_MOBILE {
            let skill = r.get(entry.name).expect("registered bundled skill");
            assert!(
                !skill.content.contains("agents/openai.yaml"),
                "{} must not include the agent prompt metadata",
                entry.name
            );
            assert!(
                !skill.content.contains("default_prompt:"),
                "{} must not inline agent YAML body markers",
                entry.name
            );
            assert!(
                !skill.content.contains("display_name:"),
                "{} must not inline agent display-name metadata",
                entry.name
            );
            if entry.references.is_empty() {
                assert_eq!(entry.name, "create-local-app");
                assert!(!skill.content.contains("## Bundled resource:"));
                continue;
            }

            assert!(
                skill.content.contains("## Bundled references"),
                "{} must include the bundled-reference preamble",
                entry.name
            );
            assert!(
                skill
                    .content
                    .contains("Follow `references/router.md` first"),
                "{} must explain router-first profile selection",
                entry.name
            );
            for reference in entry.references {
                assert!(
                    skill
                        .content
                        .contains(&format!("## Bundled resource: `{}`", reference.path)),
                    "{} must label {} with its relative path",
                    entry.name,
                    reference.path
                );
                assert!(
                    skill.content.contains(reference.content),
                    "{} must include the full {} resource body",
                    entry.name,
                    reference.path
                );
            }
        }
    }

    #[test]
    fn bundled_reference_lists_match_checked_in_markdown_files() {
        let skills_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills");

        for entry in bundled::BUILTIN_MOBILE {
            let root = skills_root.join(entry.name);
            let mut actual = std::collections::BTreeSet::new();
            let references_root = root.join("references");
            let mut pending = if references_root.is_dir() {
                vec![references_root]
            } else {
                Vec::new()
            };
            while let Some(directory) = pending.pop() {
                for item in std::fs::read_dir(&directory).expect("read checked-in references") {
                    let path = item.expect("read checked-in reference entry").path();
                    if path.is_dir() {
                        pending.push(path);
                    } else if path.extension().is_some_and(|extension| extension == "md") {
                        actual.insert(
                            path.strip_prefix(&root)
                                .expect("reference is under its skill root")
                                .to_string_lossy()
                                .replace('\\', "/"),
                        );
                    }
                }
            }
            let listed = entry
                .references
                .iter()
                .map(|reference| reference.path.to_string())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                listed, actual,
                "{} BundledResource entries must match every checked-in references/*.md file",
                entry.name
            );
        }
    }

    #[test]
    fn canvas_skill_requires_checked_in_frame_loop_helper() {
        let entry = bundled::BUILTIN_MOBILE
            .iter()
            .find(|entry| entry.name == "canvas-2d-local-app")
            .expect("Canvas skill is bundled");
        assert!(entry
            .raw
            .contains("profile-managed `createFrameLoop` from `lib/frame-loop.js`"));
        assert!(entry
            .raw
            .contains("do not call `requestAnimationFrame` directly"));
        assert!(!entry.raw.contains("one owned `requestAnimationFrame` loop"));
    }

    #[test]
    fn claude_api_desktop_skill_is_discoverable_for_python_upgrade() {
        let mut r = SkillRegistry::new();
        register_desktop(&mut r);
        assert!(
            r.discover("/claude-api upgrade python")
                .iter()
                .any(|skill| skill.name == "claude-api"),
            "claude-api should be discoverable from its builtin command surface"
        );
    }
}
