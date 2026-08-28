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
    pub(crate) references: &'static [BundledResource],
}

/// A markdown reference compiled into a bundled skill body. Bundled skills
/// have no filesystem `skill_root`, so keeping these resources in the body
/// makes the same router/profile guidance available to every caller.
pub(crate) struct BundledResource {
    pub(crate) path: &'static str,
    pub(crate) content: &'static str,
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
    references: &[],
}];

/// Mobile builtin skill templates.
pub(crate) const BUILTIN_MOBILE: &[BundledSkill] = &[
    BundledSkill {
        name: "create-local-app",
        raw: include_str!("../../../../skills/create-local-app/SKILL.md"),
        triggers: &["create local app", "local app", "local-app-build"],
        references: &[],
    },
    BundledSkill {
        name: "frontend-design",
        raw: include_str!("../../../../skills/frontend-design/SKILL.md"),
        triggers: &["frontend design", "native frontend design", "design tokens"],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/frontend-design/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/android.md",
                content: include_str!("../../../../skills/frontend-design/references/profiles/android.md"),
            },
            BundledResource {
                path: "references/profiles/canvas-overlay.md",
                content: include_str!("../../../../skills/frontend-design/references/profiles/canvas-overlay.md"),
            },
            BundledResource {
                path: "references/profiles/desktop.md",
                content: include_str!("../../../../skills/frontend-design/references/profiles/desktop.md"),
            },
            BundledResource {
                path: "references/profiles/dom.md",
                content: include_str!("../../../../skills/frontend-design/references/profiles/dom.md"),
            },
            BundledResource {
                path: "references/profiles/ios-ipados.md",
                content: include_str!("../../../../skills/frontend-design/references/profiles/ios-ipados.md"),
            },
        ],
    },
    BundledSkill {
        name: "frontend-qa",
        raw: include_str!("../../../../skills/frontend-qa/SKILL.md"),
        triggers: &["frontend qa", "browser qa", "webview verification"],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/frontend-qa/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/canvas-and-three.md",
                content: include_str!("../../../../skills/frontend-qa/references/profiles/canvas-and-three.md"),
            },
            BundledResource {
                path: "references/profiles/canvas-tool-protocol.md",
                content: include_str!("../../../../skills/frontend-qa/references/profiles/canvas-tool-protocol.md"),
            },
            BundledResource {
                path: "references/profiles/dom-and-webview.md",
                content: include_str!("../../../../skills/frontend-qa/references/profiles/dom-and-webview.md"),
            },
            BundledResource {
                path: "references/profiles/evidence-model.md",
                content: include_str!("../../../../skills/frontend-qa/references/profiles/evidence-model.md"),
            },
            BundledResource {
                path: "references/profiles/platform-matrix.md",
                content: include_str!("../../../../skills/frontend-qa/references/profiles/platform-matrix.md"),
            },
        ],
    },
    BundledSkill {
        name: "accessibility",
        raw: include_str!("../../../../skills/accessibility/SKILL.md"),
        triggers: &["accessibility", "accessibility audit", "wcag"],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/accessibility/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/canvas.md",
                content: include_str!("../../../../skills/accessibility/references/profiles/canvas.md"),
            },
            BundledResource {
                path: "references/profiles/dom.md",
                content: include_str!("../../../../skills/accessibility/references/profiles/dom.md"),
            },
            BundledResource {
                path: "references/profiles/platforms.md",
                content: include_str!("../../../../skills/accessibility/references/profiles/platforms.md"),
            },
        ],
    },
    BundledSkill {
        name: "react-best-practices",
        raw: include_str!("../../../../skills/react-best-practices/SKILL.md"),
        triggers: &["react", "react best practices", "react performance"],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/react-best-practices/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/client-state-and-effects.md",
                content: include_str!("../../../../skills/react-best-practices/references/profiles/client-state-and-effects.md"),
            },
            BundledResource {
                path: "references/profiles/render-performance.md",
                content: include_str!("../../../../skills/react-best-practices/references/profiles/render-performance.md"),
            },
        ],
    },
    BundledSkill {
        name: "ionic-react-local-app",
        raw: include_str!("../../../../skills/ionic-react-local-app/SKILL.md"),
        triggers: &[
            "ionic react",
            "ionic",
            "dom local app",
            "dom app",
            "routed local app",
            "/ionic-react-local-app",
        ],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/ionic-react-local-app/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/app-shell-and-routing.md",
                content: include_str!("../../../../skills/ionic-react-local-app/references/profiles/app-shell-and-routing.md"),
            },
            BundledResource {
                path: "references/profiles/bridge-and-data.md",
                content: include_str!("../../../../skills/ionic-react-local-app/references/profiles/bridge-and-data.md"),
            },
        ],
    },
    BundledSkill {
        name: "canvas-2d-local-app",
        raw: include_str!("../../../../skills/canvas-2d-local-app/SKILL.md"),
        triggers: &[
            "canvas 2d",
            "canvas2d",
            "2d canvas",
            "2d local app",
            "canvas local app",
            "canvas game",
            "2d game",
            "/canvas-2d-local-app",
        ],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/canvas-2d-local-app/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/assets-audio-and-qa.md",
                content: include_str!("../../../../skills/canvas-2d-local-app/references/profiles/assets-audio-and-qa.md"),
            },
            BundledResource {
                path: "references/profiles/game-loop-and-state.md",
                content: include_str!("../../../../skills/canvas-2d-local-app/references/profiles/game-loop-and-state.md"),
            },
        ],
    },
    BundledSkill {
        name: "threejs-local-app",
        raw: include_str!("../../../../skills/threejs-local-app/SKILL.md"),
        triggers: &[
            "three.js",
            "threejs",
            "three js",
            "3d scene",
            "3d local app",
            "webgl scene",
            "webgl",
            "/threejs-local-app",
        ],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/threejs-local-app/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/interaction-and-qa.md",
                content: include_str!("../../../../skills/threejs-local-app/references/profiles/interaction-and-qa.md"),
            },
            BundledResource {
                path: "references/profiles/lifecycle-and-performance.md",
                content: include_str!("../../../../skills/threejs-local-app/references/profiles/lifecycle-and-performance.md"),
            },
        ],
    },
    BundledSkill {
        name: "phaser-2d-local-app",
        raw: include_str!("../../../../skills/phaser-2d-local-app/SKILL.md"),
        triggers: &[
            "phaser",
            "phaser 2d",
            "phaser game",
            "2d arcade game",
            "/phaser-2d-local-app",
        ],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/phaser-2d-local-app/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/assets-performance-and-qa.md",
                content: include_str!("../../../../skills/phaser-2d-local-app/references/profiles/assets-performance-and-qa.md"),
            },
            BundledResource {
                path: "references/profiles/scene-lifecycle-and-input.md",
                content: include_str!("../../../../skills/phaser-2d-local-app/references/profiles/scene-lifecycle-and-input.md"),
            },
        ],
    },
    BundledSkill {
        name: "babylon-3d-local-app",
        raw: include_str!("../../../../skills/babylon-3d-local-app/SKILL.md"),
        triggers: &[
            "babylon",
            "babylon js",
            "babylon 3d",
            "babylon scene",
            "/babylon-3d-local-app",
        ],
        references: &[
            BundledResource {
                path: "references/router.md",
                content: include_str!("../../../../skills/babylon-3d-local-app/references/router.md"),
            },
            BundledResource {
                path: "references/profiles/engine-lifecycle-and-performance.md",
                content: include_str!("../../../../skills/babylon-3d-local-app/references/profiles/engine-lifecycle-and-performance.md"),
            },
            BundledResource {
                path: "references/profiles/input-overlay-and-qa.md",
                content: include_str!("../../../../skills/babylon-3d-local-app/references/profiles/input-overlay-and-qa.md"),
            },
        ],
    },
];
