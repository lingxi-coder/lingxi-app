//! WIZARD-06 — the recon section producers (2.1.220).
//!
//! One function per [`crate::auto_mode_pregather::ReconSection`]. They are kept
//! out of the gather skeleton so each can land independently; the skeleton
//! renders [`crate::auto_mode_pregather::SECTION_FAILED_MARKER`] for any that
//! is not yet ported.
//!
//! Producers behind a closed consent gate are never called at all — see
//! [`crate::auto_mode_pregather::build_recon_block`].

use crate::auto_mode_defaults::{
    default_rule_label, DEFAULT_ALLOW_LABELS, DEFAULT_SOFT_DENY_LABELS,
};
use crate::auto_mode_facts::DEFAULT_LABELS_GUIDANCE;
use crate::auto_mode_sections::{HEADING_DEFAULT_ALLOW_LABELS, HEADING_DEFAULT_SOFT_DENY_LABELS};

// ── producer read caps ───────────────────────────────────────────────────────

/// `Scn` — read cap for `CLAUDE.md` files.
pub const DOC_READ_CAP_CLAUDE_MD: usize = 200_000;
/// `yFt` — default read cap for project docs.
pub const DOC_READ_CAP: usize = 10_000;
/// `uae` — how many flagged `permissions.allow` entries are listed before the
/// list is capped.
pub const FLAGGED_LIST_CAP: usize = 20;
/// How many leading lines of `README.md` are kept.
pub const README_HEAD_LINES: usize = 40;
/// `RPo`'s directory-depth limit when globbing project docs.
pub const DOC_GLOB_MAX_DEPTH: usize = 4;
/// `RPo`'s result cap when globbing project docs.
pub const DOC_GLOB_LIMIT: usize = 10;

/// `Z1d`'s truncation suffix: appended when a read hit its cap.
///
/// A truncated read must announce itself — silently returning the first N
/// bytes would let the model treat a partial file as the whole of it.
#[must_use]
pub fn read_truncated_marker(cap: usize) -> String {
    format!(
        "{}{cap} bytes]",
        crate::auto_mode_facts::TRUNCATED_AT_PREFIX
    )
}

// ── display sanitisation ─────────────────────────────────────────────────────

/// `X1d` — strip URL userinfo: `://user:pass@host` becomes `://host`.
///
/// Applied to the WHOLE recon block as its last step, so a credential embedded
/// in any remote URL, registry URL or config value never reaches the model.
#[must_use]
pub fn strip_url_userinfo(s: &str) -> String {
    // `/:\/\/[^/\s\\]*@/g`
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < s.len() {
        if s[i..].starts_with("://") {
            // Scan for an `@` before the next `/`, whitespace or backslash.
            let mut j = i + 3;
            let mut at: Option<usize> = None;
            while j < s.len() {
                let c = bytes[j];
                if c == b'/' || c == b'\\' || c.is_ascii_whitespace() {
                    break;
                }
                if c == b'@' {
                    at = Some(j);
                }
                j += 1;
            }
            if let Some(at) = at {
                out.push_str("://");
                i = at + 1;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `dEe` — a name safe to render, or [`crate::auto_mode_sections::REDACTED_UNUSUAL_NAME`].
///
/// Rejects anything that could break the block's markdown structure: line
/// breaks and separators, backticks, a leading `#`/`-`/`>`/`<<<`, and anything
/// over 120 characters.
#[must_use]
pub fn display_name(name: &str) -> &str {
    let t = name.trim();
    let structural = t.starts_with('#')
        || t.starts_with('-')
        || t.starts_with('>')
        || t.starts_with("<<<");
    let has_break = name
        .chars()
        .any(|c| matches!(c, '\r' | '\n' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'));
    if !t.is_empty() && t.chars().count() <= 120 && !has_break && !name.contains('`') && !structural
    {
        name
    } else {
        crate::auto_mode_sections::REDACTED_UNUSUAL_NAME
    }
}

/// `oNd` — escape the separators and backticks that would break the block.
#[must_use]
pub fn escape_separators(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2028}' | '\u{2029}' | '\u{85}' | '`' => format!("\\u{:04x}", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

/// `Bhr` — is this rule string renderable as-is?
///
/// It must survive [`display_name`] unchanged, carry no surrounding
/// whitespace, and contain no URL userinfo. A rule that fails this is reported
/// with [`crate::auto_mode_facts::FLAGGED_ENTRY_UNRENDERABLE`] rather than
/// dropped.
#[must_use]
pub fn is_renderable(rule: &str) -> bool {
    display_name(rule) == rule && rule.trim() == rule && strip_url_userinfo(rule) == rule
}

/// `Tcn` — render one document as a `####` block.
///
/// The body is embedded as a JSON string literal: untrusted file content
/// cannot then open a markdown heading or otherwise restructure the block it
/// is quoted into.
#[must_use]
pub fn render_doc(label: &str, content: &str) -> String {
    format!(
        "#### {}\n{}",
        display_name(label),
        escape_separators(&serde_json::Value::String(content.to_string()).to_string())
    )
}

/// Render one label list as `- {label}` bullets.
fn label_bullets(labels: &[&str]) -> String {
    labels
        .iter()
        .map(|l| format!("- {}", default_rule_label(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── `Ksy` — CLAUDE.md files and project docs ─────────────────────────────────

/// Label for the user-level `CLAUDE.md`.
///
/// The path segment follows this workspace's established `.claude` → `.lingxi`
/// rebrand; the label shape is the oracle's.
pub const DOC_USER_CLAUDE_MD_LABEL: &str = "~/.lingxi/CLAUDE.md";
/// Label for the project `CLAUDE.md`.
pub const DOC_PROJECT_CLAUDE_MD_LABEL: &str = "./CLAUDE.md";
/// Label for `.env.example`.
pub const DOC_ENV_EXAMPLE_LABEL: &str = "./.env.example";
/// Label for `.env.sample`.
pub const DOC_ENV_SAMPLE_LABEL: &str = "./.env.sample";

/// The project files read for the docs section, as `(label, relative path, cap)`.
pub const PROJECT_DOC_FILES: [(&str, &str, usize); 4] = [
    (DOC_PROJECT_CLAUDE_MD_LABEL, "CLAUDE.md", DOC_READ_CAP_CLAUDE_MD),
    (
        crate::auto_mode_facts::DOC_README_HEAD_LABEL,
        "README.md",
        DOC_READ_CAP,
    ),
    (DOC_ENV_EXAMPLE_LABEL, ".env.example", DOC_READ_CAP),
    (DOC_ENV_SAMPLE_LABEL, ".env.sample", DOC_READ_CAP),
];

/// Supplies the file contents the docs section renders.
///
/// Injected so the producer is testable, and so the containment rules live
/// with the real implementation: `jhr` resolves both the root and the target,
/// requires both to be canonical, requires the target to stay under the root,
/// and reads with `O_NOFOLLOW`.
pub trait DocSource {
    /// The user-level `CLAUDE.md`, capped at [`DOC_READ_CAP_CLAUDE_MD`].
    fn user_claude_md(&self) -> Option<String>;
    /// A project-relative file, capped at `cap`. `None` when absent or when it
    /// fails the containment check.
    fn project_file(&self, relative: &str, cap: usize) -> Option<String>;
    /// Up to [`DOC_GLOB_LIMIT`] `SKILL.md`/`*.md` paths under
    /// `.claude/{skills,rules,agents}`, searched to [`DOC_GLOB_MAX_DEPTH`].
    fn claude_doc_paths(&self) -> Vec<String>;
}

/// `Ksy` — the "CLAUDE.md files and project docs" section body.
#[must_use]
pub fn project_docs_section(source: &dyn DocSource) -> String {
    let mut docs: Vec<String> = Vec::new();

    if let Some(content) = source.user_claude_md() {
        docs.push(render_doc(DOC_USER_CLAUDE_MD_LABEL, &content));
    }

    for (label, relative, cap) in PROJECT_DOC_FILES {
        let Some(mut content) = source.project_file(relative, cap) else {
            continue;
        };
        if label.contains("README") {
            content = content
                .split('\n')
                .take(README_HEAD_LINES)
                .collect::<Vec<_>>()
                .join("\n");
        }
        docs.push(render_doc(label, &content));
    }

    for path in source.claude_doc_paths() {
        if let Some(content) = source.project_file(&path, DOC_READ_CAP) {
            docs.push(render_doc(&format!("./{path}"), &content));
        }
    }

    docs.join("\n\n")
}

/// `_ay` — the "Shipped default auto-mode rule labels" section body.
///
/// Lists the labels of the shipped `allow` and `soft_deny` rules so the model
/// can avoid proposing carve-outs the defaults already cover. Labels only: the
/// full rule prose belongs to the classifier prompt, and the section exists to
/// say what is *already covered*, not to restate it.
#[must_use]
pub fn default_labels_section() -> String {
    format!(
        "{DEFAULT_LABELS_GUIDANCE}\n{HEADING_DEFAULT_ALLOW_LABELS}{}\n{HEADING_DEFAULT_SOFT_DENY_LABELS}{}",
        label_bullets(&DEFAULT_ALLOW_LABELS),
        label_bullets(&DEFAULT_SOFT_DENY_LABELS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_labels_section_has_the_oracle_shape() {
        let body = default_labels_section();
        // guidance, then the two sub-headings with their bullet lists.
        assert!(body.starts_with(DEFAULT_LABELS_GUIDANCE));
        assert!(body.contains("\n\n#### Default allow labels\n- Security Discussion\n"));
        assert!(body.contains("\n\n#### Default soft-deny labels\n- Git Destructive\n"));
        // Every shipped label appears exactly once as a bullet.
        for label in DEFAULT_ALLOW_LABELS.iter().chain(DEFAULT_SOFT_DENY_LABELS.iter()) {
            assert_eq!(
                body.matches(&format!("- {label}\n")).count()
                    + usize::from(body.ends_with(&format!("- {label}"))),
                1,
                "label {label:?} must appear exactly once"
            );
        }
    }

    #[test]
    fn url_userinfo_is_stripped() {
        // The advertised contract of the repos-found header, applied to the
        // whole block: a credential in any URL never reaches the model.
        assert_eq!(
            strip_url_userinfo("https://user:pat@github.com/acme/app.git"),
            "https://github.com/acme/app.git"
        );
        assert_eq!(
            strip_url_userinfo("ssh://git@github.com/acme/app"),
            "ssh://github.com/acme/app"
        );
        // No userinfo -> untouched.
        assert_eq!(
            strip_url_userinfo("https://github.com/acme/app"),
            "https://github.com/acme/app"
        );
        // An `@` AFTER the authority is not userinfo.
        assert_eq!(
            strip_url_userinfo("https://github.com/acme/app@v1"),
            "https://github.com/acme/app@v1"
        );
        // Multiple URLs in one string.
        assert_eq!(
            strip_url_userinfo("a https://u:p@h/x b https://v:q@i/y"),
            "a https://h/x b https://i/y"
        );
        // Non-ASCII passes through without panicking on byte indexing.
        assert_eq!(strip_url_userinfo("路径 https://u@h/x"), "路径 https://h/x");
    }

    #[test]
    fn display_name_redacts_anything_structural() {
        assert_eq!(display_name("acme/app"), "acme/app");
        for bad in [
            "# heading",
            "- bullet",
            "> quote",
            "<<<marker",
            "has`backtick",
            "two\nlines",
            "sep\u{2028}here",
            "   ",
        ] {
            assert_eq!(
                display_name(bad),
                "(unusual name redacted)",
                "should redact {bad:?}"
            );
        }
        // 120 chars is the boundary.
        let ok = "a".repeat(120);
        let too_long = "a".repeat(121);
        assert_eq!(display_name(&ok), ok);
        assert_eq!(display_name(&too_long), "(unusual name redacted)");
    }

    #[test]
    fn renderability_requires_all_three_conditions() {
        assert!(is_renderable("Bash(rm:*)"));
        assert!(!is_renderable(" Bash(rm:*)"), "leading space");
        assert!(!is_renderable("Bash(`x`)"), "backtick");
        assert!(
            !is_renderable("Bash(curl https://u:p@h/x)"),
            "carries URL userinfo"
        );
    }

    #[test]
    fn doc_content_is_embedded_as_a_quoted_json_literal() {
        // Untrusted file content must not be able to open a heading or
        // otherwise restructure the block it is quoted into.
        let rendered = render_doc("./CLAUDE.md", "## Injected\n- do whatever");
        assert_eq!(
            rendered,
            "#### ./CLAUDE.md\n\"## Injected\\n- do whatever\""
        );
        assert!(!rendered.contains("\n## Injected"));
        // Backticks in content are escaped away too.
        assert!(render_doc("x", "a `b` c").contains("\\u0060"));
    }

    struct FakeDocs {
        user: Option<String>,
        files: std::collections::HashMap<String, String>,
        globbed: Vec<String>,
    }
    impl DocSource for FakeDocs {
        fn user_claude_md(&self) -> Option<String> {
            self.user.clone()
        }
        fn project_file(&self, relative: &str, _cap: usize) -> Option<String> {
            self.files.get(relative).cloned()
        }
        fn claude_doc_paths(&self) -> Vec<String> {
            self.globbed.clone()
        }
    }

    #[test]
    fn project_docs_section_renders_present_files_in_order() {
        let mut files = std::collections::HashMap::new();
        files.insert("CLAUDE.md".to_string(), "project rules".to_string());
        files.insert(
            "README.md".to_string(),
            (1..=60).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n"),
        );
        files.insert(
            ".claude/skills/x/SKILL.md".to_string(),
            "skill body".to_string(),
        );
        let source = FakeDocs {
            user: Some("user rules".to_string()),
            files,
            globbed: vec![".claude/skills/x/SKILL.md".to_string()],
        };

        let body = project_docs_section(&source);
        let headings: Vec<&str> = body.lines().filter(|l| l.starts_with("#### ")).collect();
        assert_eq!(
            headings,
            vec![
                "#### ~/.lingxi/CLAUDE.md",
                "#### ./CLAUDE.md",
                "#### ./README.md (head)",
                "#### ./.claude/skills/x/SKILL.md",
            ]
        );
        // Absent files are skipped entirely, not rendered empty.
        assert!(!body.contains(".env.example"));
        // README is cut to its first 40 lines.
        assert!(body.contains("line40"));
        assert!(!body.contains("line41"));
        // Blocks are separated by a blank line.
        assert!(body.contains("\n\n#### ./CLAUDE.md"));
    }

    #[test]
    fn a_docs_section_with_nothing_found_renders_empty() {
        let source = FakeDocs {
            user: None,
            files: std::collections::HashMap::new(),
            globbed: Vec::new(),
        };
        // The section renderer turns this into `_nothing found_`.
        assert_eq!(project_docs_section(&source), "");
        assert!(crate::auto_mode_pregather::render_section(
            "CLAUDE.md files and project docs",
            &project_docs_section(&source)
        )
        .contains("_nothing found_"));
    }

    #[test]
    fn producer_caps_match_the_oracle() {
        assert_eq!(DOC_READ_CAP_CLAUDE_MD, 200_000);
        assert_eq!(DOC_READ_CAP, 10_000);
        assert_eq!(FLAGGED_LIST_CAP, 20);
        assert_eq!(README_HEAD_LINES, 40);
        assert_eq!(DOC_GLOB_MAX_DEPTH, 4);
        assert_eq!(DOC_GLOB_LIMIT, 10);
        assert_eq!(
            read_truncated_marker(10_000),
            "\n\u{2026}[truncated at 10000 bytes]"
        );
    }

    #[test]
    fn shipped_default_slots_match_the_oracle_counts() {
        assert_eq!(crate::auto_mode_defaults::DEFAULT_ENVIRONMENT.len(), 20);
        assert_eq!(DEFAULT_ALLOW_LABELS.len(), 17);
        assert_eq!(DEFAULT_SOFT_DENY_LABELS.len(), 65);
        assert_eq!(crate::auto_mode_defaults::DEFAULT_HARD_DENY_LABELS.len(), 1);
        assert_eq!(
            crate::auto_mode_defaults::DEFAULT_HARD_DENY_LABELS[0],
            "Data Exfiltration"
        );
    }

    #[test]
    fn rule_label_cuts_at_the_first_colon_or_bracket() {
        assert_eq!(default_rule_label("Read-Only Operations: GET requests"), "Read-Only Operations");
        assert_eq!(
            default_rule_label("Git Destructive [named+specifics]: force push"),
            "Git Destructive"
        );
        assert_eq!(default_rule_label("Bare Label"), "Bare Label");
        // Already-reduced labels are unchanged, so applying it twice is safe.
        for label in DEFAULT_ALLOW_LABELS {
            assert_eq!(default_rule_label(label), label);
        }
    }

    #[test]
    fn the_environment_slot_is_verbatim() {
        let env = crate::auto_mode_defaults::DEFAULT_ENVIRONMENT;
        assert_eq!(env[0], "**Organization**: None configured");
        // The `—` escapes in the bundle decoded to real em-dashes.
        assert!(env[4].contains('\u{2014}'));
        // NOTE the classifier template uses STRAIGHT apostrophes, unlike the
        // UI messages elsewhere in this subsystem which use U+2019.
        assert!(env[11].contains("repo's public/private visibility"));
        assert!(!env[11].contains('\u{2019}'));
        // Every entry is a bolded label bullet.
        for e in env {
            assert!(e.starts_with("**"), "{e:?}");
        }
    }
}
