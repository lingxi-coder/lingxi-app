//! The one scope vocabulary.
//!
//! Before this module the engine modelled "which rung does this configuration
//! sit on" twenty-four separate times — `SettingsLayer`, `HookSource`,
//! `SkillSource`, `CommandSource`, `ConfigScope`, `MemoryTier`, … — across
//! twenty-one crates. They drifted, and the drift has already cost a bug:
//! `permission`'s citation ladder once ranked `session` highest, the reverse of
//! the reference implementation.
//!
//! Three facts were tangled together in those enums. This module separates
//! them:
//!
//! 1. [`Scope`] — **which store** the thing lives in (`~/.lingxi/`, the project
//!    tree, the binary, this session's memory, …).
//! 2. [`Origin`] — **what delivered it** (a plugin, an MCP server, an agent's
//!    front-matter, …). Orthogonal to the rung: a project-level plugin's hook
//!    is `(Scope::Project, Origin::Plugin(id))`, a sentence the old enums could
//!    not express because both facts fought for the same variant.
//! 3. Resolved locations (`repo_root`, `team_dir`, …) — deliberately NOT here.
//!    Those belong to whichever crate resolved them, next to a [`Scope`] field.
//!
//! # `Scope` is unordered on purpose
//!
//! It would be natural to `derive(PartialOrd, Ord)` here and call the result
//! "the precedence ladder". That would be wrong, and the codebase proves it:
//! the same rungs are ordered several mutually contradictory ways, each
//! correct for its own question.
//!
//! | Question | `User` vs `Project` vs `Local` |
//! |---|---|
//! | Which settings *value* wins? | `Local` > `Project` > `User` |
//! | Which rule does a denial *cite*? | `User` > `Project` > `Local` |
//! | What order is `LINGXI.md` spliced in? | `User` before `Project` before `Local` |
//!
//! The first two are exact opposites and both match the reference
//! implementation — value-precedence and citation-precedence are genuinely
//! different axes. A derived `Ord` would silently pick one and invert the
//! others, which is the same failure that produced the original bug, only
//! harder to see.
//!
//! So: **`Scope` carries identity, never order.** Each question owns a named
//! ordering function next to the code that asks it, and each such function owns
//! a test pinning the pairs it depends on. The `scope_is_not_ordered` test
//! below keeps a comparison trait from being derived here.

use crate::ids::{AgentId, McpConnectionId, PluginId};
use serde::{Deserialize, Serialize};

/// Which store a piece of configuration lives in.
///
/// Identity only — see the module docs for why this type is deliberately not
/// ordered and not comparable beyond equality.
///
/// The derived `Serialize` form is PascalCase (`"Project"`), which preserves
/// the historical `MemoryEntry` round-trip. It is **one** of several spellings
/// this engine emits, not the canonical one: the hook payload wants
/// `snake_case`, the MCP telemetry inventory wants a different bucket name
/// entirely (`Local` reports as `"user"` there), and the settings admin panel
/// wants its own. Never reach for the derived form to satisfy one of those —
/// each has its own mapping function, and they are not interchangeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// Compiled into the binary. No file backs it.
    Builtin,
    /// Environment variables.
    Env,
    /// Arguments given on the command line for this run.
    Cli,
    /// Configuration attached to a feature flag.
    Flag,
    /// Lives only for the current session; never written to disk.
    Session,
    /// `<project>/.lingxi/*.local.json` — per-clone, gitignored.
    Local,
    /// `<project>/.lingxi/` — checked into the project.
    Project,
    /// `~/.lingxi/` — this user, all projects.
    User,
    /// A shared team directory.
    Team,
    /// Administrator-managed policy.
    Managed,
}

/// The three rungs backed by a settings file a user can edit.
///
/// Six of the enums this module replaced were *exactly* this subset —
/// `configuration-admin`'s `Scope`, the CLI's `Scope`, `SettingsSource`,
/// `AgentMemoryScope`, and the two wire DTOs — so it earns a shared type
/// rather than a sixfold repeat.
///
/// It exists to keep a guarantee that a bare [`Scope`] would lose. A field
/// typed `Option<Scope>` accepts `Managed` and `Env`; several of those six
/// reach `serde` on a production path (an `--agents` JSON blob is
/// deserialized straight into `AgentDefinition`), so widening the type would
/// quietly widen what those paths accept, in the permissive direction. The
/// narrowing is therefore a type, not a runtime check: values outside the
/// triple fail to parse instead of being honoured.
///
/// Converting *up* to [`Scope`] is total; converting *down* is fallible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettingsScope {
    /// `~/.lingxi/settings.json`.
    User,
    /// `<project>/.lingxi/settings.json` — checked in.
    Project,
    /// `<project>/.lingxi/settings.local.json` — gitignored.
    Local,
}

impl From<SettingsScope> for Scope {
    fn from(value: SettingsScope) -> Self {
        match value {
            SettingsScope::User => Self::User,
            SettingsScope::Project => Self::Project,
            SettingsScope::Local => Self::Local,
        }
    }
}

impl TryFrom<Scope> for SettingsScope {
    type Error = Scope;

    /// Returns the offending [`Scope`] as the error so a caller can name it in
    /// a message ("cannot write to the managed settings tier").
    fn try_from(value: Scope) -> Result<Self, Self::Error> {
        match value {
            Scope::User => Ok(Self::User),
            Scope::Project => Ok(Self::Project),
            Scope::Local => Ok(Self::Local),
            other => Err(other),
        }
    }
}

/// What delivered a piece of configuration to its [`Scope`].
///
/// A variant earns a place here only when **more than one subsystem** produces
/// it. Producers specific to a single subsystem stay in that subsystem —
/// `permission`'s `ToolsNarrowing`, `mcp`'s `ClaudeAi` — because hoisting them
/// would hand every unrelated `match` an arm it can never see, which is how the
/// twenty-four enums grew in the first place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Origin {
    /// The scope's own store: the settings file at that rung, or — at
    /// [`Scope::Builtin`] — the binary itself.
    Store,
    /// Supplied by an installed plugin.
    Plugin(PluginId),
    /// Derived from an MCP server's tools or prompts.
    Mcp(McpConnectionId),
    /// Declared in an agent file's front-matter.
    Frontmatter(AgentId),
    /// Packaged inside a skill bundle.
    Skill,
}

/// Where a piece of configuration came from: its rung and its deliverer.
///
/// This replaces the mixed enums (`HookSource`, `CommandSource`, `SkillSource`,
/// `AgentSource`, `ConfigScope`, …) that spent one variant list on two
/// independent questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    /// Which store it lives in.
    pub scope: Scope,
    /// What delivered it there.
    pub origin: Origin,
}

impl Provenance {
    /// Configuration read straight from the store at `scope`.
    #[must_use]
    pub const fn store(scope: Scope) -> Self {
        Self {
            scope,
            origin: Origin::Store,
        }
    }

    /// Configuration delivered to `scope` by `origin`.
    #[must_use]
    pub const fn new(scope: Scope, origin: Origin) -> Self {
        Self { scope, origin }
    }
}

#[cfg(test)]
mod tests {
    use super::{Origin, Provenance, Scope, SettingsScope};
    use crate::ids::PluginId;

    /// Both sides of the narrowing, because only pinning the accepted side
    /// would let the gate widen silently — and it widens in the permissive
    /// direction, which is the dangerous one.
    #[test]
    fn settings_scope_accepts_exactly_the_three_editable_rungs() {
        for (scope, expected) in [
            (Scope::User, SettingsScope::User),
            (Scope::Project, SettingsScope::Project),
            (Scope::Local, SettingsScope::Local),
        ] {
            assert_eq!(SettingsScope::try_from(scope), Ok(expected));
            assert_eq!(Scope::from(expected), scope, "round trip");
        }

        for rejected in [
            Scope::Builtin,
            Scope::Env,
            Scope::Cli,
            Scope::Flag,
            Scope::Session,
            Scope::Team,
            Scope::Managed,
        ] {
            assert_eq!(
                SettingsScope::try_from(rejected),
                Err(rejected),
                "{rejected:?} is not user-editable and must not narrow"
            );
        }
    }

    /// The reason this is a type and not a runtime check: these values reach
    /// `serde` on a production path (`--agents` JSON → `AgentDefinition`).
    /// A field typed `Scope` would newly accept every rung above.
    #[test]
    fn settings_scope_refuses_a_wider_rung_through_serde() {
        assert!(serde_json::from_str::<SettingsScope>("\"Project\"").is_ok());
        assert!(serde_json::from_str::<SettingsScope>("\"Managed\"").is_err());
        assert!(serde_json::from_str::<SettingsScope>("\"Session\"").is_err());

        // The wide type does accept them — which is exactly what the narrow
        // type is protecting those fields from.
        assert!(serde_json::from_str::<Scope>("\"Managed\"").is_ok());
    }

    /// The pairing the old enums could not express: a plugin installed at the
    /// project rung. `HookSource` had to choose between `Project` and `Plugin`
    /// and lost the other fact whichever it picked.
    #[test]
    fn a_rung_and_a_deliverer_are_independently_representable() {
        let id = PluginId::new();
        let p = Provenance::new(Scope::Project, Origin::Plugin(id));

        assert_eq!(p.scope, Scope::Project);
        assert_eq!(p.origin, Origin::Plugin(id));
        assert_ne!(p, Provenance::new(Scope::User, Origin::Plugin(id)));
        assert_ne!(p, Provenance::store(Scope::Project));
    }

    /// `MemoryEntry.tier` serialized its four tiers as PascalCase before this
    /// type existed. Widening the enum must not have changed those four bytes.
    #[test]
    fn the_memory_tier_rungs_keep_their_historical_pascal_case_spelling() {
        for (scope, expected) in [
            (Scope::Session, "\"Session\""),
            (Scope::Project, "\"Project\""),
            (Scope::Team, "\"Team\""),
            (Scope::User, "\"User\""),
        ] {
            assert_eq!(serde_json::to_string(&scope).unwrap(), expected);
        }
    }

    /// Guards the module's central claim. `Scope` must never gain a comparison
    /// beyond equality: several subsystems order these rungs in mutually
    /// contradictory ways (settings-value precedence puts `Local` above `User`,
    /// denial citation puts `User` above `Local`), so a single derived order
    /// would silently invert one of them.
    ///
    /// Written as an autoref-specialization probe rather than a grep, so it
    /// cannot be fooled by a commented-out derive or a reformatted attribute.
    /// The *bounded inherent* method exists only while `T: PartialOrd`, and an
    /// inherent method shadows the trait one — so `probe()` answers `true`
    /// exactly when the type is ordered, and deriving an order on [`Scope`]
    /// turns this test red.
    #[test]
    fn scope_is_not_ordered() {
        struct Probe<T>(std::marker::PhantomData<T>);

        // Fallback: applies to every `Probe<T>`.
        trait Unordered {
            fn probe(&self) -> bool {
                false
            }
        }
        impl<T> Unordered for Probe<T> {}

        // Shadows the fallback, but only exists when `T` is ordered.
        impl<T: PartialOrd> Probe<T> {
            #[allow(clippy::unused_self)]
            fn probe(&self) -> bool {
                true
            }
        }

        // The probe itself must be able to say "yes" — otherwise the assertion
        // below would pass for a type that IS ordered, and prove nothing.
        assert!(
            Probe::<u8>(std::marker::PhantomData).probe(),
            "probe is broken: it cannot detect an ordered type"
        );

        assert!(
            !Probe::<Scope>(std::marker::PhantomData).probe(),
            "Scope gained a PartialOrd/Ord impl — see the module docs: the \
             rungs have several conflicting correct orders, so precedence \
             belongs in a named function per question, never on the type."
        );
    }
}
