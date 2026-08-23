//! Compiled-in builtin skill templates.
//!
//! Each entry carries the canonical name, raw markdown, and the registry-only
//! discovery phrases. The phrases live beside compiled bundled metadata rather
//! than in SKILL.md frontmatter because the repository's portable Skill
//! validator intentionally accepts only the public frontmatter schema.

/// Desktop builtin skill templates.
pub(crate) struct BundledSkill {
    pub(crate) name: &'static str,
    pub(crate) raw: &'static str,
    pub(crate) triggers: &'static [&'static str],
}

/// Desktop builtin skill templates.
pub(crate) const BUILTIN_DESKTOP: &[BundledSkill] = &[BundledSkill {
    name: "claude-api",
    raw: include_str!("claude-api.md"),
    triggers: &[
        "claude-api",
        "/claude-api",
        "/claude-api upgrade python",
        "claude api upgrade python",
        "anthropic sdk migration",
    ],
}];

/// Mobile builtin skill templates.
pub(crate) const BUILTIN_MOBILE: &[BundledSkill] = &[
    BundledSkill {
        name: "create-local-app",
        raw: include_str!("../../../../skills/create-local-app/SKILL.md"),
        triggers: &["create local app", "local app", "local-app-build"],
    },
    BundledSkill {
        name: "frontend-design",
        raw: include_str!("../../../../skills/frontend-design/SKILL.md"),
        triggers: &["frontend design", "native frontend design", "design tokens"],
    },
    BundledSkill {
        name: "frontend-qa",
        raw: include_str!("../../../../skills/frontend-qa/SKILL.md"),
        triggers: &["frontend qa", "browser qa", "webview verification"],
    },
    BundledSkill {
        name: "accessibility",
        raw: include_str!("../../../../skills/accessibility/SKILL.md"),
        triggers: &["accessibility", "accessibility audit", "wcag"],
    },
    BundledSkill {
        name: "react-best-practices",
        raw: include_str!("../../../../skills/react-best-practices/SKILL.md"),
        triggers: &["react", "react best practices", "react performance"],
    },
];
