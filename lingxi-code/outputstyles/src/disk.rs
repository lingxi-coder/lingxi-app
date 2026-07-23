//! Custom output-style DISK discovery (OUTSTYLE.3 — claude-code
//! `getOutputStyleDirStyles` + `markdownConfigLoader`).
//!
//! claude-code loads custom output styles from `~/.lingxi/output-styles/` and
//! `<cwd>/.lingxi/output-styles/` (each a `*.md` file: optional `---`-YAML
//! frontmatter + a markdown body that becomes the system-prompt addendum), and
//! merges them OVER the two compiled-in builtins (`Explanatory`/`Learning`) so a
//! `settings.outputStyle` naming a custom style activates it. The Rust port
//! previously resolved ONLY the builtins ([`crate::resolve_builtin_output_style`]),
//! so a custom style name resolved to "no style". This module adds the disk
//! loader + an OWNED [`ResolvedOutputStyle`] resolver that prefers disk styles
//! (project over user, both over builtins — claude-code priority order) and
//! falls back to the builtins.
//!
//! The parsing ([`parse_output_style`]) and resolution
//! ([`resolve_output_style_from`]) are PURE (no I/O), so they are unit-tested
//! without touching the filesystem; the thin [`load_output_styles_from_dir`] /
//! [`resolve_output_style`] wrappers add the directory walk.

use crate::registry::{resolve_builtin_output_style, DEFAULT_OUTPUT_STYLE_NAME};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// A custom output style loaded from a disk `*.md` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskOutputStyle {
    /// Style name (the `settings.outputStyle` selector + the
    /// `# Output Style: <name>` heading). Defaults to the file stem when the
    /// frontmatter omits `name`.
    pub name: String,
    /// Short description (frontmatter `description`, or empty).
    pub description: String,
    /// The markdown body — the verbatim system-prompt addendum.
    pub prompt: String,
    /// `keepCodingInstructions` (TS, default `true`): when `false`, the
    /// coding-instructions section is omitted from the assembled system prompt.
    pub keep_coding_instructions: bool,
    /// `force-for-plugin` (binary bytes 189331814, 206814560): when set on a
    /// plugin-owned output style, the style activates automatically when the
    /// plugin is enabled. Parsed and stored; enforcement is in the plugin
    /// activation layer (not yet wired in LingXi).
    pub force_for_plugin: Option<String>,
}

/// The active output style resolved for system-prompt assembly — OWNED (unlike
/// the `&'static` [`crate::registry::BuiltinOutputStyle`]) so it can carry a
/// disk-loaded custom style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOutputStyle {
    /// Style name (heading text + `settings.outputStyle` value).
    pub name: String,
    /// Verbatim system-prompt body appended after the heading.
    pub prompt: String,
    /// `keepCodingInstructions` (see [`DiskOutputStyle::keep_coding_instructions`]).
    pub keep_coding_instructions: bool,
}

/// Frontmatter fields we read from an output-style file (all optional). The
/// struct-level `#[serde(default)]` + the manual [`Default`] (which sets
/// `keep_coding_instructions = true`) make every field optional AND give the
/// TS-faithful `keepCodingInstructions` default of `true` for BOTH a missing
/// field and a file with no frontmatter at all.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct DiskFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "keepCodingInstructions")]
    keep_coding_instructions: bool,
    /// Plugin this output style is automatically applied for when the plugin is
    /// active. Binary bytes 189331814, 206814560. When set on a non-plugin style
    /// the binary logs a warning (not yet implemented in LingXi). For plugin
    /// styles, the style activates automatically when the plugin is enabled.
    #[serde(rename = "force-for-plugin")]
    force_for_plugin: Option<String>,
}

impl Default for DiskFrontmatter {
    fn default() -> Self {
        Self {
            name: None,
            description: None,
            keep_coding_instructions: true,
            force_for_plugin: None,
        }
    }
}

/// Parse one output-style file's raw contents into a [`DiskOutputStyle`].
///
/// Accepts an optional leading `---`-delimited YAML frontmatter block followed
/// by `\n---\n` and the markdown body (the same shape as skill / command
/// markdown). When no (or malformed) frontmatter is present, the whole input is
/// the body and `name` defaults to `stem` (the file name without `.md`),
/// `description` empty, `keepCodingInstructions` true. PURE — no I/O.
#[must_use]
pub fn parse_output_style(raw: &str, stem: &str) -> DiskOutputStyle {
    let (fm, body) = if let Some(rest) = raw.strip_prefix("---") {
        if let Some(end) = rest.find("\n---\n") {
            let fm: DiskFrontmatter = serde_yaml::from_str(&rest[..end]).unwrap_or_default();
            (fm, rest[end + 5..].trim().to_string())
        } else {
            (DiskFrontmatter::default(), raw.trim().to_string())
        }
    } else {
        (DiskFrontmatter::default(), raw.trim().to_string())
    };
    DiskOutputStyle {
        name: fm.name.unwrap_or_else(|| stem.to_string()),
        description: fm.description.unwrap_or_default(),
        prompt: body,
        keep_coding_instructions: fm.keep_coding_instructions,
        force_for_plugin: fm.force_for_plugin,
    }
}

/// Load every `*.md` output style from `dir` (e.g. `~/.lingxi/output-styles` or
/// `<cwd>/.lingxi/output-styles`). A missing/unreadable directory yields an
/// empty list. Thin glue over [`parse_output_style`].
#[must_use]
pub fn load_output_styles_from_dir(dir: &Path) -> Vec<DiskOutputStyle> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut styles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let style = parse_output_style(&raw, stem);
        if style.force_for_plugin.is_some() {
            eprintln!(
                "warning: output style '{}' declares force-for-plugin outside a plugin; ignoring automatic activation",
                style.name
            );
        }
        styles.push(style);
    }
    styles
}

/// Resolve the active style from `setting` against already-loaded `disk` styles
/// then the builtins. PURE (no I/O). `disk` is in INCREASING priority order
/// (e.g. `[user…, project…]`); the highest-priority match wins, and any disk
/// match OVERRIDES a same-named builtin (claude-code merge order). `None` /
/// `""` / `"default"` / an unknown name → `None`.
#[must_use]
pub fn resolve_output_style_from(
    setting: Option<&str>,
    disk: &[DiskOutputStyle],
) -> Option<ResolvedOutputStyle> {
    let name = match setting {
        None | Some("") => return None,
        Some(name) => name,
    };
    if name == DEFAULT_OUTPUT_STYLE_NAME {
        return None;
    }
    // Disk styles override builtins; the last (highest-priority) match wins.
    if let Some(d) = disk.iter().rev().find(|s| s.name == name) {
        return Some(ResolvedOutputStyle {
            name: d.name.clone(),
            prompt: d.prompt.clone(),
            keep_coding_instructions: d.keep_coding_instructions,
        });
    }
    resolve_builtin_output_style(setting).map(|b| ResolvedOutputStyle {
        name: b.name.to_string(),
        prompt: b.prompt.to_string(),
        keep_coding_instructions: b.keep_coding_instructions,
    })
}

/// Resolve the active output style from `setting`, discovering custom styles in
/// `dirs` (in INCREASING priority — e.g. `[user_dir, project_dir]`) and falling
/// back to the builtins. Combines [`load_output_styles_from_dir`] +
/// [`resolve_output_style_from`].
#[must_use]
pub fn resolve_output_style(
    setting: Option<&str>,
    dirs: &[PathBuf],
) -> Option<ResolvedOutputStyle> {
    let disk: Vec<DiskOutputStyle> = dirs
        .iter()
        .flat_map(|d| load_output_styles_from_dir(d))
        .collect();
    resolve_output_style_from(setting, &disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_with_frontmatter() {
        let raw = "---\nname: Terse\ndescription: short replies\nkeepCodingInstructions: false\n---\nBe terse.\n";
        let s = parse_output_style(raw, "terse-file");
        assert_eq!(s.name, "Terse"); // frontmatter name wins over the stem
        assert_eq!(s.description, "short replies");
        assert_eq!(s.prompt, "Be terse.");
        assert!(!s.keep_coding_instructions);
    }

    #[test]
    fn parse_without_frontmatter_defaults_name_to_stem() {
        let s = parse_output_style("Just a body, no frontmatter.", "my-style");
        assert_eq!(s.name, "my-style");
        assert_eq!(s.description, "");
        assert_eq!(s.prompt, "Just a body, no frontmatter.");
        assert!(s.keep_coding_instructions); // default true
    }

    #[test]
    fn parse_malformed_frontmatter_treats_all_as_body() {
        // A leading `---` with no closing `\n---\n` is not frontmatter.
        let s = parse_output_style("---\nno close", "x");
        assert_eq!(s.name, "x");
        assert_eq!(s.prompt, "---\nno close");
    }

    fn disk(name: &str, prompt: &str) -> DiskOutputStyle {
        DiskOutputStyle {
            name: name.into(),
            description: String::new(),
            prompt: prompt.into(),
            keep_coding_instructions: true,
            force_for_plugin: None,
        }
    }

    #[test]
    fn resolve_disk_style_by_name() {
        let styles = [disk("Terse", "Be terse.")];
        let r = resolve_output_style_from(Some("Terse"), &styles).unwrap();
        assert_eq!(r.name, "Terse");
        assert_eq!(r.prompt, "Be terse.");
    }

    #[test]
    fn resolve_disk_overrides_builtin_and_project_wins_over_user() {
        // A disk style named "Explanatory" overrides the builtin; and a later
        // (project) entry overrides an earlier (user) one of the same name.
        let styles = [
            disk("Explanatory", "USER custom explanatory"),
            disk("Explanatory", "PROJECT custom explanatory"),
        ];
        let r = resolve_output_style_from(Some("Explanatory"), &styles).unwrap();
        assert_eq!(r.prompt, "PROJECT custom explanatory");
    }

    #[test]
    fn resolve_falls_back_to_builtin() {
        // No disk style of that name → the compiled-in builtin.
        let r = resolve_output_style_from(Some("Explanatory"), &[]).unwrap();
        assert_eq!(r.name, "Explanatory");
        assert!(r.prompt.contains("# Explanatory Style Active"));
    }

    #[test]
    fn resolve_none_default_empty_and_unknown_are_no_style() {
        assert_eq!(resolve_output_style_from(None, &[]), None);
        assert_eq!(resolve_output_style_from(Some(""), &[]), None);
        assert_eq!(resolve_output_style_from(Some("default"), &[]), None);
        assert_eq!(resolve_output_style_from(Some("Nonexistent"), &[]), None);
    }

    #[test]
    fn load_from_missing_dir_is_empty() {
        let styles = load_output_styles_from_dir(Path::new("/no/such/dir/xyz"));
        assert!(styles.is_empty());
    }

    // ---- force-for-plugin (P2 gap, binary bytes 189331814) ------------------

    #[test]
    fn force_for_plugin_parsed() {
        let raw = "---\nname: MyStyle\nforce-for-plugin: my-plugin\n---\nBe concise.\n";
        let s = parse_output_style(raw, "my-style");
        assert_eq!(
            s.force_for_plugin.as_deref(),
            Some("my-plugin"),
            "force-for-plugin must be parsed from frontmatter"
        );
    }

    #[test]
    fn force_for_plugin_defaults_none() {
        let s = parse_output_style("Just a body.", "no-plugin");
        assert_eq!(s.force_for_plugin, None, "force-for-plugin absent → None");
    }
}
