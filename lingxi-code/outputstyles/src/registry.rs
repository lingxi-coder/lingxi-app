//! Output-style registry with builtin markdown default and async switching.
//!
//! Also hosts the two compiled-in *prompt* styles claude-code ships
//! (`Explanatory` / `Learning`) and the settings → style resolver — the
//! `OutputStyleConfig` analogue (OUTSTYLE.2). The verbatim prompt bodies live
//! in [`crate::builtin`]; this module wraps them in [`BuiltinOutputStyle`]
//! configs and resolves the active one from the `output_style` setting.

use crate::builtin::{EXPLANATORY_PROMPT, LEARNING_PROMPT};
use crate::disk::ResolvedOutputStyle;
use crate::model::{OutputFormat, OutputStyle, OutputStyleFrontmatter, OutputStyleSource};
use protocol::PluginId;
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::RwLock;

/// The settings value claude-code treats as "no active style".
///
/// Source: `claude-code/src/constants/outputStyles.ts:39`
/// (`DEFAULT_OUTPUT_STYLE_NAME = 'default'`). When `settings.outputStyle`
/// is this value (or unset/empty) the system prompt gets no output-style
/// section.
pub const DEFAULT_OUTPUT_STYLE_NAME: &str = "default";

/// One compiled-in (`source: 'built-in'`) output-style config, mirroring the
/// fields of the TS `OutputStyleConfig` (`outputStyles.ts:11-23`) that matter
/// for system-prompt assembly.
///
/// Only the two builtins (`Explanatory` / `Learning`) are represented here.
/// Disk / plugin / managed custom styles are resolved elsewhere in claude-code
/// (`getAllOutputStyles`) and are an OUTSTYLE.2 follow-up — out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinOutputStyle {
    /// Canonical, case-sensitive style name (`"Explanatory"` / `"Learning"`).
    /// Doubles as the `settings.outputStyle` value that selects the style and
    /// as the `# Output Style: <name>` heading text.
    pub name: &'static str,
    /// Short user-facing description (TS `description`).
    pub description: &'static str,
    /// Verbatim system-prompt body appended after the heading (TS `prompt`).
    pub prompt: &'static str,
    /// TS `keepCodingInstructions`. When `true`, claude-code keeps the
    /// coding-instructions section (`getSimpleDoingTasksSection`) in the
    /// prompt; both builtins set this `true`. Carried here for the
    /// coding-section-omission follow-up (OUTSTYLE.2 (c)).
    pub keep_coding_instructions: bool,
}

/// Builtin `Explanatory` config (`outputStyles.ts:43-55`).
const EXPLANATORY: BuiltinOutputStyle = BuiltinOutputStyle {
    name: "Explanatory",
    description: "Claude explains its implementation choices and codebase patterns",
    prompt: EXPLANATORY_PROMPT,
    keep_coding_instructions: true,
};

/// Builtin `Learning` config (`outputStyles.ts:56-134`).
const LEARNING: BuiltinOutputStyle = BuiltinOutputStyle {
    name: "Learning",
    description: "Claude pauses and asks you to write small pieces of code for hands-on practice",
    prompt: LEARNING_PROMPT,
    keep_coding_instructions: true,
};

/// Every compiled-in style, in the order `/output-style` lists them.
///
/// ⚠️ 2.1.270 ships FOUR built-ins — `Proactive` and `Concise` in addition to
/// these two — each with a verbatim prompt body plus `turnReminder` /
/// `waitingTurnReminder` surfaces this port has no analogue for. Porting those
/// is its own item; listing the two that exist is still the whole selectable
/// set HERE, so the listing is honest about what this build can switch to.
pub const BUILTIN_OUTPUT_STYLES: [BuiltinOutputStyle; 2] = [EXPLANATORY, LEARNING];

/// Resolve the active builtin output style from the engine settings
/// `output_style` value (the `Option<String>` from `lingxi_core::settings`).
///
/// Faithful to the built-in branch of TS `getOutputStyleConfig`
/// (`outputStyles.ts:181-211`): `settings.outputStyle || 'default'`, then
/// `allStyles[name] ?? null`. Concretely:
///
/// * `None` / `Some("")` — unset or empty collapses to `'default'` → `None`.
/// * `Some("default")` — explicit default → `None`.
/// * `Some("Explanatory")` / `Some("Learning")` — the matching builtin.
/// * any other value — `None`. (TS would next try a disk/plugin/managed
///   custom style; those are out of scope here, so an unrecognized name is
///   treated as "no style". This is the documented OUTSTYLE.2 follow-up seam.)
///
/// Matching is case-sensitive and not trimmed, matching the TS object lookup
/// (`allStyles[' Explanatory ']` would miss).
#[must_use]
pub fn resolve_builtin_output_style(setting: Option<&str>) -> Option<BuiltinOutputStyle> {
    let name = match setting {
        // `settings?.outputStyle` undefined, or `'' || 'default'` -> 'default'.
        None | Some("") => return None,
        Some(name) => name,
    };
    if name == DEFAULT_OUTPUT_STYLE_NAME {
        return None;
    }
    match name {
        "Explanatory" => Some(EXPLANATORY),
        "Learning" => Some(LEARNING),
        // Unknown / custom (disk/plugin) names: out of scope -> no builtin.
        _ => None,
    }
}

/// Owns all known output styles plus a (mutable) pointer to the active one.
pub struct OutputStyleRegistry {
    styles: HashMap<String, OutputStyle>,
    current_active: RwLock<String>,
    plugin_styles: HashMap<PluginId, Vec<String>>,
    forced_plugin_styles: Vec<(PluginId, String)>,
}

/// Errors raised by [`OutputStyleRegistry`].
#[derive(Debug, Clone, Error)]
pub enum OutputStyleError {
    /// The requested style is not registered.
    #[error("not found: {0}")]
    NotFound(String),
}

impl OutputStyleRegistry {
    /// Construct a new registry pre-populated with the builtin `markdown`
    /// style, which is set active.
    #[must_use]
    pub fn new() -> Self {
        let mut s = HashMap::new();
        s.insert(
            "markdown".into(),
            OutputStyle {
                name: "markdown".into(),
                description: "Default markdown output".into(),
                source: OutputStyleSource::Builtin,
                frontmatter: OutputStyleFrontmatter {
                    name: "markdown".into(),
                    description: String::new(),
                    default: true,
                    format: OutputFormat::Markdown,
                    keep_coding_instructions: true,
                    force_for_plugin: false,
                },
                system_prompt_addendum: String::new(),
                source_path: None,
            },
        );
        Self {
            styles: s,
            current_active: RwLock::new("markdown".into()),
            plugin_styles: HashMap::new(),
            forced_plugin_styles: Vec::new(),
        }
    }

    /// Return a clone of the currently-active style.
    pub async fn active(&self) -> OutputStyle {
        let name = self.current_active.read().await.clone();
        self.styles
            .get(&name)
            .cloned()
            .expect("active style must exist")
    }

    /// Switch the active style by name.
    ///
    /// # Errors
    /// Returns [`OutputStyleError::NotFound`] if `name` is not registered.
    pub async fn switch(&self, name: &str) -> Result<(), OutputStyleError> {
        if !self.styles.contains_key(name) {
            return Err(OutputStyleError::NotFound(name.into()));
        }
        *self.current_active.write().await = name.into();
        Ok(())
    }

    /// Register a single style (or overwrite an existing one with the same name).
    pub fn register(&mut self, style: OutputStyle) {
        self.styles.insert(style.name.clone(), style);
    }

    /// Every registered style's name, sorted, for the `/output-style` listing.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.styles.keys().cloned().collect();
        out.sort();
        out
    }

    /// Look up a registered style by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&OutputStyle> {
        self.styles.get(name)
    }

    /// Resolve a non-default registered style into the prompt-facing shape.
    /// This is the live plugin-registry counterpart to the disk resolver.
    #[must_use]
    pub fn resolve(&self, setting: Option<&str>) -> Option<ResolvedOutputStyle> {
        let active;
        let name = match setting {
            Some(name) if !name.is_empty() && name != DEFAULT_OUTPUT_STYLE_NAME => name,
            _ => {
                active = self
                    .forced_plugin_styles
                    .last()
                    .map(|(_, name)| name.as_str())?;
                active
            }
        };
        let style = self.styles.get(name)?;
        Some(ResolvedOutputStyle {
            name: style.name.clone(),
            prompt: style.system_prompt_addendum.clone(),
            keep_coding_instructions: style.frontmatter.keep_coding_instructions,
        })
    }

    /// Register a batch of styles owned by `plugin_id`.
    pub fn register_plugin_styles(&mut self, plugin_id: PluginId, styles: Vec<OutputStyle>) {
        let names: Vec<String> = styles.iter().map(|s| s.name.clone()).collect();
        for s in styles {
            // `force-for-plugin` is a BOOLEAN (oracle schema): a plugin-owned
            // style activates automatically when the flag is set. The plugin
            // association comes from the style's `source`/owner, not the flag.
            if matches!(s.source, OutputStyleSource::Plugin) && s.frontmatter.force_for_plugin {
                self.forced_plugin_styles
                    .retain(|(owner, _)| owner != &plugin_id);
                self.forced_plugin_styles.push((plugin_id, s.name.clone()));
            }
            self.styles.insert(s.name.clone(), s);
        }
        self.plugin_styles.insert(plugin_id, names);
    }

    /// Remove every style previously registered under `plugin_id`.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.forced_plugin_styles
            .retain(|(owner, _)| owner != plugin_id);
        if let Some(names) = self.plugin_styles.remove(plugin_id) {
            for n in &names {
                self.styles.remove(n);
            }
        }
    }
}

impl Default for OutputStyleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_is_markdown() {
        let r = OutputStyleRegistry::new();
        assert_eq!(r.active().await.name, "markdown");
    }

    #[tokio::test]
    async fn switch_to_unknown_errors() {
        let r = OutputStyleRegistry::new();
        assert!(r.switch("nope").await.is_err());
    }

    #[test]
    fn force_for_plugin_activates_and_unload_restores_default() {
        let mut registry = OutputStyleRegistry::new();
        let plugin_id = PluginId::new();
        registry.register_plugin_styles(
            plugin_id,
            vec![OutputStyle {
                name: "demo:terse".into(),
                description: String::new(),
                source: OutputStyleSource::Plugin,
                frontmatter: OutputStyleFrontmatter {
                    name: "demo:terse".into(),
                    force_for_plugin: true,
                    ..OutputStyleFrontmatter::default()
                },
                system_prompt_addendum: "Be terse.".into(),
                source_path: None,
            }],
        );

        assert_eq!(
            registry.resolve(None).map(|style| style.name),
            Some("demo:terse".into())
        );
        registry.unregister_plugin(&plugin_id);
        assert!(registry.resolve(None).is_none());
    }

    // ---- OUTSTYLE.2: builtin prompt-style configs + settings resolver ----

    #[test]
    fn resolve_none_default_and_empty_are_no_style() {
        // TS: undefined / '' (-> 'default') / 'default' all map to null.
        assert_eq!(resolve_builtin_output_style(None), None);
        assert_eq!(resolve_builtin_output_style(Some("")), None);
        assert_eq!(resolve_builtin_output_style(Some("default")), None);
    }

    #[test]
    fn resolve_unknown_name_is_no_style() {
        // Custom/disk/plugin styles are out of scope -> treated as no style.
        assert_eq!(resolve_builtin_output_style(Some("Nonexistent")), None);
        // Case-sensitive, untrimmed (matches the TS object lookup).
        assert_eq!(resolve_builtin_output_style(Some("explanatory")), None);
        assert_eq!(resolve_builtin_output_style(Some(" Explanatory ")), None);
    }

    #[test]
    fn resolve_explanatory_builtin() {
        let s = resolve_builtin_output_style(Some("Explanatory")).expect("Explanatory resolves");
        assert_eq!(s.name, "Explanatory");
        assert_eq!(
            s.description,
            "Claude explains its implementation choices and codebase patterns"
        );
        assert!(s.keep_coding_instructions);
        // Verbatim prompt anchors (claude-code outputStyles.ts:49-54).
        assert!(s.prompt.starts_with(
            "You are an interactive CLI tool that helps users with software engineering tasks."
        ));
        assert!(s.prompt.contains("# Explanatory Style Active"));
        // figures.star (U+2605) is substituted into the Insight banner.
        assert!(s.prompt.contains("\u{2605} Insight"));
        assert!(s
            .prompt
            .ends_with("rather than general programming concepts."));
        // Exact byte length lock (measured from the rendered TS template).
        assert_eq!(s.prompt.chars().count(), 1023);
        assert_eq!(s.prompt.len(), 1197);
    }

    #[test]
    fn resolve_learning_builtin() {
        let s = resolve_builtin_output_style(Some("Learning")).expect("Learning resolves");
        assert_eq!(s.name, "Learning");
        assert_eq!(
            s.description,
            "Claude pauses and asks you to write small pieces of code for hands-on practice"
        );
        assert!(s.keep_coding_instructions);
        assert!(s.prompt.contains("# Learning Style Active"));
        assert!(s.prompt.contains("**Learn by Doing**"));
        assert!(s.prompt.contains("TODO(human)"));
        // figures.bullet (U+25CF) appears in each Learn-by-Doing header (4x).
        assert_eq!(s.prompt.matches('\u{25CF}').count(), 4);
        // figures.star (U+2605) from the shared Insight banner (1x).
        assert_eq!(s.prompt.matches('\u{2605}').count(), 1);
        // Trailing whitespace from the TS source is preserved verbatim.
        assert!(s.prompt.contains("routine implementation yourself.   \n"));
        assert_eq!(s.prompt.chars().count(), 4888);
        assert_eq!(s.prompt.len(), 5076);
    }
}
