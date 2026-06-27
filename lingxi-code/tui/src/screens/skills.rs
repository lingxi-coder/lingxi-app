//! `/skills` registry viewer (claude-code `SkillsMenu.tsx` parity): a
//! read-only, scrollable list of discovered skills grouped by source.
//!
//! Four-part split mirroring `agents.rs`/`theme.rs`: a [`SkillsState`]
//! (grouped sections + an embedded [`ScrollState`]), a [`SkillsOutcome`]
//! enum, a pure [`handle_skills_key`] reducer (scroll keys delegated to the
//! shared [`crate::screens::scroll::ScrollState`]; Esc/`q` close), and a pure
//! [`render_skills_to_string`] oracle.
//!
//! Literal-lock (byte-for-byte from claude-code `SkillsMenu.tsx`): the
//! `Skills` title, the `No skills found` / `Create skills in .lingxi/skills/
//! or ~/.lingxi/skills/` empty state, the `{N} skill(s)` subtitle, the
//! per-section titles (`Project skills` / `User skills` / `Managed skills` /
//! `Plugin skills` / `MCP skills`), the section RENDER ORDER (project, user,
//! managed/policy, plugin, mcp), and the per-skill row
//! `{name} · ~{tokens} description tokens` (with ` · {plugin}` inserted for
//! plugin skills). The frontmatter-token estimate ports
//! `estimateSkillFrontmatterTokens` (`round(len([name, description,
//! when_to_use].join(' ')) / 4)`) and the `~{n}`/compact `~{n.n}k` display
//! ports `formatTokens` (Intl compact notation, lowercased, `.0` stripped).
//!
//! Data note: the frozen `OrchestratorHandle` exposes no `list_skills`, so the
//! TUI loads skills off disk itself via [`load_skill_sections`] — a frozen-safe
//! `.lingxi/skills/` dir walk (ported from claude-code `loadSkillsFromSkillsDir`
//! and `getSkillDirCommands`) run on the blocking pool by `root::pump_open_skills`
//! (mirroring the `/stats` open pump). When no skill exists on disk the section
//! vec is empty → the locked empty state. Parity caveats (documented, not
//! blockers): the `Managed skills` (policy) source needs a `getManagedFilePath`
//! analogue, which has no seam outside the frozen `traits/`, so it is omitted in
//! v1; `Plugin`/`MCP` skills have no frozen-safe TUI seam to the plugin manager
//! / MCP registry, so those sections stay empty (claude-code's `SkillsMenu`
//! omits empty groups anyway). Dedup is by canonical path within the walk.
#![forbid(unsafe_code)]

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};

use crate::screens::scroll::{scroll_indicator, visible_slice, ScrollState};

/// The fixed viewport height (rows of the flattened body the window shows at
/// once before scrolling kicks in). A modest constant keeps the pure oracle
/// deterministic; the live render is line-by-line and not height-bounded, but
/// the embedded [`ScrollState`] keeps the screen scroll-capable and unit-
/// testable.
const VIEWPORT: usize = 16;

/// Locked dialog title (claude-code `SkillsMenu.tsx`).
pub const TITLE: &str = "Skills";
/// Locked empty-state subtitle.
pub const EMPTY_SUBTITLE: &str = "No skills found";
/// Locked empty-state body line.
pub const EMPTY_BODY: &str = "Create skills in .lingxi/skills/ or ~/.lingxi/skills/";

/// One skill row, already reduced to display fields (claude-code `renderSkill`
/// reads `name`, `description`, `when_to_use` for the token estimate and the
/// optional `plugin` name badge).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillRow {
    /// Canonical skill name (the command name shown on the row).
    pub name: String,
    /// Short description (counted toward the frontmatter-token estimate).
    pub description: String,
    /// Long-form `when_to_use` guidance, if any (also counted).
    pub when_to_use: Option<String>,
    /// Owning plugin name — shown as a ` · {plugin}` badge for plugin skills.
    pub plugin: Option<String>,
}

impl SkillRow {
    /// Port of `estimateSkillFrontmatterTokens`: rough token count of the
    /// joined `[name, description, when_to_use]` frontmatter text
    /// (`round(chars / 4)`, skipping empty parts like the TS `.filter(Boolean)`).
    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        let mut parts: Vec<&str> = Vec::with_capacity(3);
        if !self.name.is_empty() {
            parts.push(&self.name);
        }
        if !self.description.is_empty() {
            parts.push(&self.description);
        }
        if let Some(w) = &self.when_to_use {
            if !w.is_empty() {
                parts.push(w);
            }
        }
        let text = parts.join(" ");
        rough_token_count(&text)
    }
}

/// One source section (claude-code groups skills by `source`, renders a bold
/// dim title, then each skill row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSection {
    /// Locked section title (e.g. `"Project skills"`).
    pub title: String,
    /// Rows in this section, pre-sorted by name.
    pub rows: Vec<SkillRow>,
    /// (skills-section-subtitle-missing) The section's skills directory,
    /// display-shortened (claude-code `getSourceSubtitle` / `getDisplayPath`)
    /// — rendered as `"{title} ({subtitle})"`. `None` omits the parens.
    pub subtitle: Option<String>,
}

/// Screen state: the grouped sections + the embedded scroll window over the
/// flattened body lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillsState {
    /// Source sections in claude-code render order (project, user, managed,
    /// plugin, mcp); empty sections are omitted on build.
    pub sections: Vec<SkillSection>,
    /// Scroll window over the flattened content lines.
    pub scroll: ScrollState,
}

impl SkillsState {
    /// Build from grouped sections, sizing the embedded [`ScrollState`] to the
    /// flattened body-line count and the fixed [`VIEWPORT`].
    #[must_use]
    pub fn new(sections: Vec<SkillSection>) -> Self {
        let len = content_lines(&sections).len();
        Self {
            sections,
            scroll: ScrollState::new(len, VIEWPORT),
        }
    }

    /// Total skill count across all sections (drives the `{N} skill(s)`
    /// subtitle).
    #[must_use]
    pub fn total_skills(&self) -> usize {
        self.sections.iter().map(|s| s.rows.len()).sum()
    }

    /// `true` when no skill is registered (the locked empty state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_skills() == 0
    }
}

/// Build the grouped [`SkillSection`]s for the `/skills` viewer by reading the
/// on-disk `.lingxi/skills/` directories, ported from claude-code
/// `loadSkillsFromSkillsDir` + `getSkillDirCommands`.
///
/// Two file-based sources, pushed in claude-code render order (project, user;
/// `SkillsMenu` renders `projectSettings` then `userSettings`):
/// - **Project** — every `<ancestor>/.lingxi/skills` from `cwd` up to the git
///   root (or `lingxi_home`'s parent) inclusive, like `getProjectDirsUpToHome`.
/// - **User** — `<lingxi_home>/skills`.
///
/// For each directory, every immediate entry that is a directory (or a symlink
/// to one) is treated as a skill whose name is the ENTRY (dir) name — NOT the
/// frontmatter `name` — matching `loadSkillsFromSkillsDir` (single `.md` files
/// directly under `skills/` are ignored: directory format only). The entry's
/// `SKILL.md` is read + parsed via [`skill_api::parse_skill_markdown`]; entries
/// without a readable `SKILL.md` are skipped. Rows are deduplicated by canonical
/// path (`fs::canonicalize`, like `getFileIdentity`'s `realpath`) first-wins
/// across BOTH sources, then sorted by name within each section. Empty sections
/// are omitted (the existing [`content_lines`] also skips them).
///
/// All fs reads live in this fn so the caller (`root::pump_open_skills`) runs it
/// on the blocking pool, off the UI executor — mirroring the `/stats` walk.
///
/// FORCED DIVERGENCE from claude-code: the `Managed skills` (policy) source is
/// omitted — `getManagedFilePath` has no seam reachable outside the frozen
/// `traits/`; `Plugin`/`MCP` skills are likewise omitted (no frozen-safe TUI
/// seam). The git-root boundary uses a simple nearest-`.git` ancestor scan (no
/// submodule/worktree edge-case handling from `resolveStopBoundary`). Name sort
/// uses Rust `str` `Ord` rather than JS `localeCompare` (the codebase's accepted
/// 1:1 approximation, locked by a test).
#[must_use]
pub fn load_skill_sections(cwd: &Path, lingxi_home: &Path) -> Vec<SkillSection> {
    skill_api::load_file_skill_sections(cwd, lingxi_home)
        .into_iter()
        .map(|section| SkillSection {
            title: section.title,
            subtitle: section
                .path
                .map(|p| crate::components::messages::assistant_tool_use::get_display_path(
                    &p.to_string_lossy(),
                    cwd,
                )),
            rows: section
                .rows
                .into_iter()
                .map(|row| SkillRow {
                    name: row.name,
                    description: row.description,
                    when_to_use: row.when_to_use,
                    plugin: None,
                })
                .collect(),
        })
        .collect()
}

/// Controller outcome after a key (mirrors `AgentsOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsOutcome {
    /// Stay open (scrolled or inert key).
    Stay,
    /// Close the screen (Esc / `q`).
    Close,
}

/// Reduce one key. Scroll keys (Up/Down/PageUp/PageDown/Home/End) are handled
/// by the embedded [`ScrollState`]; Esc and bare `q` close. Everything else is
/// inert. Pure — the caller owns closing the screen + telemetry.
#[must_use]
pub fn handle_skills_key(state: &mut SkillsState, key: KeyEvent) -> SkillsOutcome {
    // Scroll keys first (claude-code modal pager parity); `handle_scroll_key`
    // returns `true` when it consumed the key.
    if state.scroll.handle_scroll_key(key) {
        return SkillsOutcome::Stay;
    }
    match key.code {
        KeyCode::Esc => SkillsOutcome::Close,
        KeyCode::Char('q') if key.modifiers == crossterm::event::KeyModifiers::NONE => {
            SkillsOutcome::Close
        }
        _ => SkillsOutcome::Stay,
    }
}

/// Port of `roughTokenCountEstimation`: `Math.round(content.length / 4)`.
fn rough_token_count(content: &str) -> usize {
    // `chars().count()` matches JS `string.length` for the BMP names/
    // descriptions skills carry. `Math.round` is round-half-up; with a
    // divisor of 4, `(chars + 2) / 4` (integer division) yields the same
    // result (remainders 0,1 round down; 2,3 round up).
    let chars = content.chars().count();
    (chars + 2) / 4
}

/// The locked per-skill row text (claude-code `renderSkill`): `{name}` then the
/// dim suffix ` · ~{tokens} description tokens`, with ` · {plugin}` inserted
/// before the token clause for plugin skills.
fn render_skill_row(row: &SkillRow) -> String {
    let token_display = format!("~{}", format_tokens(row.estimated_tokens()));
    match &row.plugin {
        Some(p) if !p.is_empty() => {
            format!(
                "{} \u{00B7} {p} \u{00B7} {token_display} description tokens",
                row.name
            )
        }
        _ => format!("{} \u{00B7} {token_display} description tokens", row.name),
    }
}

/// Flatten the sections to the body content lines (no title/subtitle/footer):
/// for each non-empty section, a section-title line then one line per skill.
/// This is the list the embedded [`ScrollState`] scrolls over.
fn content_lines(sections: &[SkillSection]) -> Vec<String> {
    let mut out = Vec::new();
    let mut first = true;
    for section in sections {
        if section.rows.is_empty() {
            continue;
        }
        // (skills-gap-between-sections) blank line between consecutive non-empty
        // groups (not before the first).
        if !first {
            out.push(String::new());
        }
        first = false;
        // (skills-section-subtitle-missing) "{title} ({subtitle})" when the
        // section has a display-path subtitle.
        match &section.subtitle {
            Some(sub) => out.push(format!("{} ({sub})", section.title)),
            None => out.push(section.title.clone()),
        }
        for row in &section.rows {
            out.push(render_skill_row(row));
        }
    }
    out
}

/// Pure render oracle: the full screen body as text.
///
/// Empty: `Skills` / `No skills found` / `Create skills …`. Non-empty:
/// `Skills` / `{N} skill(s)` subtitle, then the visible window of the
/// flattened section/row lines, then (when scrolled) a scroll indicator, then
/// the `Esc to close` footer.
#[must_use]
pub fn render_skills_to_string(state: &SkillsState) -> String {
    let mut out = String::from(TITLE);
    out.push('\n');

    if state.is_empty() {
        out.push_str(EMPTY_SUBTITLE);
        out.push('\n');
        out.push_str(EMPTY_BODY);
        out.push('\n');
        out.push_str("Esc to close");
        return out;
    }

    let n = state.total_skills();
    out.push_str(&format!("{n} {}\n", plural(n, "skill")));

    let lines = content_lines(&state.sections);
    for line in visible_slice(&lines, &state.scroll) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(ind) = scroll_indicator(&state.scroll) {
        out.push_str(&ind);
        out.push('\n');
    }
    out.push_str("Esc to close");
    out
}

/// `plural(n, "skill")` (claude-code `plural`): `"skill"` when `n == 1`, else
/// `"skills"`.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Port of `formatTokens` (`formatNumber(count).replace('.0', '')`): values
/// under 1000 render as the plain integer; 1000+ use compact `k`/`m` notation
/// with one fraction digit, lowercased, trailing `.0` stripped.
#[allow(clippy::cast_precision_loss)]
fn format_tokens(count: usize) -> String {
    if count < 1000 {
        return count.to_string();
    }
    let (value, suffix) = if count >= 1_000_000 {
        (count as f64 / 1_000_000.0, 'm')
    } else {
        (count as f64 / 1000.0, 'k')
    };
    // One fraction digit (Intl `maximumFractionDigits: 1`), then strip a
    // trailing `.0` to match `formatTokens`' `.replace('.0', '')`.
    let mut s = format!("{value:.1}");
    if let Some(stripped) = s.strip_suffix(".0") {
        s = stripped.to_string();
    }
    format!("{s}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(name: &str, desc: &str) -> SkillRow {
        SkillRow {
            name: name.into(),
            description: desc.into(),
            when_to_use: None,
            plugin: None,
        }
    }

    fn section(title: &str, rows: Vec<SkillRow>) -> SkillSection {
        SkillSection {
            title: title.into(),
            rows,
            subtitle: None,
        }
    }

    #[test]
    fn empty_state_is_byte_locked() {
        let s = SkillsState::new(vec![]);
        assert!(s.is_empty());
        let out = render_skills_to_string(&s);
        assert_eq!(
            out,
            "Skills\nNo skills found\nCreate skills in .lingxi/skills/ or ~/.lingxi/skills/\nEsc to close"
        );
    }

    #[test]
    fn header_and_subtitle_plural() {
        let s = SkillsState::new(vec![section(
            "Project skills",
            vec![row("alpha", "a"), row("beta", "b")],
        )]);
        let out = render_skills_to_string(&s);
        assert!(out.starts_with("Skills\n2 skills\n"));
        // Single skill → singular subtitle.
        let one = SkillsState::new(vec![section("User skills", vec![row("solo", "x")])]);
        assert!(render_skills_to_string(&one).starts_with("Skills\n1 skill\n"));
    }

    #[test]
    fn section_grouping_and_row_format() {
        let s = SkillsState::new(vec![
            section("Project skills", vec![row("alpha", "does alpha things")]),
            section("MCP skills", vec![row("srv:beta", "beta")]),
        ]);
        let out = render_skills_to_string(&s);
        // Section titles present in order.
        let idx_proj = out.find("Project skills").expect("project title");
        let idx_mcp = out.find("MCP skills").expect("mcp title");
        assert!(idx_proj < idx_mcp);
        // Row format: name · ~N description tokens.
        assert!(out.contains("alpha \u{00B7} ~"));
        assert!(out.contains(" description tokens"));
    }

    #[test]
    fn plugin_badge_inserted_for_plugin_rows() {
        let r = SkillRow {
            name: "fmt".into(),
            description: "format code".into(),
            when_to_use: None,
            plugin: Some("prettier".into()),
        };
        let line = render_skill_row(&r);
        assert!(
            line.starts_with("fmt \u{00B7} prettier \u{00B7} ~"),
            "got: {line}"
        );
        assert!(line.ends_with(" description tokens"));
    }

    #[test]
    fn token_estimate_rounds_chars_over_four() {
        // "ab cd" → join of name+desc; len 5 (incl. the join space) → round(5/4)=1.
        let r = row("ab", "cd");
        assert_eq!(r.estimated_tokens(), 1);
        // when_to_use counted too: "n" + "d" + "wwww" joined → "n d wwww" len 8 → 2.
        let r2 = SkillRow {
            name: "n".into(),
            description: "d".into(),
            when_to_use: Some("wwww".into()),
            plugin: None,
        };
        assert_eq!(r2.estimated_tokens(), 2);
    }

    #[test]
    fn format_tokens_compact() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(1300), "1.3k");
        assert_eq!(format_tokens(2_000_000), "2m");
    }

    #[test]
    fn esc_and_q_close_other_keys_stay() {
        let mut s = SkillsState::new(vec![section("User skills", vec![row("a", "x")])]);
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::Esc)),
            SkillsOutcome::Close
        );
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::Char('q'))),
            SkillsOutcome::Close
        );
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::Enter)),
            SkillsOutcome::Stay
        );
    }

    #[test]
    fn scroll_keys_move_window_and_stay() {
        // 3 sections × (1 title + many rows) → a tall body so scrolling is live.
        let rows: Vec<SkillRow> = (0..30).map(|i| row(&format!("s{i}"), "d")).collect();
        let mut s = SkillsState::new(vec![section("Project skills", rows)]);
        assert!(s.scroll.is_scrollable());
        assert_eq!(s.scroll.offset(), 0);
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::Down)),
            SkillsOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), 1);
        // End jumps to max_offset; clamp holds.
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::End)),
            SkillsOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
        // Further Down clamps (still Stay).
        assert_eq!(
            handle_skills_key(&mut s, k(KeyCode::Down)),
            SkillsOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
    }

    #[test]
    fn short_list_shows_no_indicator() {
        let s = SkillsState::new(vec![section("User skills", vec![row("a", "x")])]);
        let out = render_skills_to_string(&s);
        assert!(!out.contains("more"));
    }

    // ---- `load_skill_sections` disk-loader tests (M9-09 real data) --------
    //
    // Each builds a tempdir tree and a `lingxi_home` (the User-skills root) so
    // the walk is deterministic + offline. A `.git` marker at the project cwd
    // bounds the project-dir walk to exactly the cwd (mirrors a git root), so a
    // stray `.git` in a real ancestor of the tempdir can't perturb the result.

    /// Write `<base>/<name>/SKILL.md` with the given frontmatter fields, as a
    /// directory-format skill (the only format `loadSkillsFromSkillsDir` reads).
    fn write_skill(base: &Path, name: &str, description: &str, when_to_use: Option<&str>) {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir skill dir");
        let mut fm = format!("---\nname: {name}\ndescription: {description}\n");
        if let Some(w) = when_to_use {
            fm.push_str(&format!("when_to_use: {w}\n"));
        }
        fm.push_str("---\nBody.\n");
        std::fs::write(dir.join("SKILL.md"), fm).expect("write SKILL.md");
    }

    /// `(project_cwd, lingxi_home)` rooted under a fresh tempdir, with a `.git`
    /// marker at the cwd so the project walk stops there. The `lingxi_home` sits
    /// in a SEPARATE subtree so it is never seen as a project ancestor.
    fn fixture(tmp: &Path) -> (PathBuf, PathBuf) {
        let cwd = tmp.join("proj");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");
        std::fs::create_dir_all(cwd.join(".git")).expect("mkdir .git");
        let lingxi_home = tmp.join("home").join(".lingxi");
        std::fs::create_dir_all(&lingxi_home).expect("mkdir lingxi_home");
        (cwd, lingxi_home)
    }

    #[test]
    fn loads_project_skill_with_dir_name_and_carries_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        let proj_skills = cwd.join(".lingxi").join("skills");
        // Frontmatter `name` deliberately DIFFERS from the dir name to lock the
        // claude-code rule: the skill name is the ENTRY (dir) name, NOT the
        // frontmatter name.
        std::fs::create_dir_all(proj_skills.join("alpha")).expect("mkdir alpha");
        std::fs::write(
            proj_skills.join("alpha").join("SKILL.md"),
            "---\nname: not-alpha\ndescription: does alpha things\nwhen_to_use: when alpha\n---\nBody.\n",
        )
        .expect("write SKILL.md");

        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert_eq!(sections.len(), 1, "one non-empty section");
        assert_eq!(sections[0].title, "Project skills");
        assert_eq!(sections[0].rows.len(), 1);
        let r = &sections[0].rows[0];
        // Name is the DIR name, not the frontmatter `name`.
        assert_eq!(r.name, "alpha");
        assert_eq!(r.description, "does alpha things");
        assert_eq!(r.when_to_use.as_deref(), Some("when alpha"));
        // Token estimate flows through the existing `estimated_tokens`.
        assert!(r.estimated_tokens() > 0);
    }

    #[test]
    fn section_subtitle_is_display_path_and_renders_in_content_lines() {
        // (skills-section-subtitle-missing)
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        std::fs::create_dir_all(cwd.join(".lingxi").join("skills").join("alpha"))
            .expect("mkdir alpha");
        std::fs::write(
            cwd.join(".lingxi").join("skills").join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: d\n---\nBody.\n",
        )
        .expect("write SKILL.md");
        std::fs::create_dir_all(lingxi_home.join("skills").join("beta")).expect("mkdir beta");
        std::fs::write(
            lingxi_home.join("skills").join("beta").join("SKILL.md"),
            "---\nname: beta\ndescription: d\n---\nBody.\n",
        )
        .expect("write SKILL.md");

        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert_eq!(sections.len(), 2);
        // Project: relative to cwd (under cwd).
        assert_eq!(
            sections[0].subtitle.as_deref(),
            Some(".lingxi/skills"),
            "{:?}",
            sections[0].subtitle
        );
        // User: `~`-prefixed (under $HOME via lingxi_home).
        let user_sub = sections[1].subtitle.as_deref().unwrap_or("");
        assert!(user_sub.ends_with("/skills"), "{user_sub:?}");

        let lines = content_lines(&sections);
        assert_eq!(lines[0], "Project skills (.lingxi/skills)");
    }

    #[test]
    fn user_skills_form_their_own_section_after_project() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        write_skill(&cwd.join(".lingxi").join("skills"), "pjr", "p", None);
        write_skill(&lingxi_home.join("skills"), "usr", "u", None);

        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Project skills");
        assert_eq!(sections[1].title, "User skills");
        // Render order: project before user (SkillsMenu projectSettings→userSettings).
        let out = render_skills_to_string(&SkillsState::new(sections));
        let idx_proj = out.find("Project skills").expect("project title");
        let idx_user = out.find("User skills").expect("user title");
        assert!(idx_proj < idx_user);
    }

    #[test]
    fn missing_dirs_yield_empty_vec_and_locked_empty_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        // No skills written anywhere → empty Vec → the byte-locked empty state.
        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert!(sections.is_empty());
        let s = SkillsState::new(sections);
        assert!(s.is_empty());
        assert_eq!(
            render_skills_to_string(&s),
            "Skills\nNo skills found\nCreate skills in .lingxi/skills/ or ~/.lingxi/skills/\nEsc to close"
        );
    }

    #[test]
    fn entry_without_skill_md_is_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        let proj_skills = cwd.join(".lingxi").join("skills");
        // A real skill...
        write_skill(&proj_skills, "good", "g", None);
        // ...and a sibling dir with NO SKILL.md (just some other file).
        std::fs::create_dir_all(proj_skills.join("empty")).expect("mkdir empty");
        std::fs::write(proj_skills.join("empty/README.md"), "x").expect("write readme");

        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert_eq!(sections.len(), 1);
        let names: Vec<&str> = sections[0].rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["good"]);
    }

    #[test]
    fn rows_sorted_by_name_within_section() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        let proj_skills = cwd.join(".lingxi").join("skills");
        // Created out of order; the loader must sort by name (str `Ord`).
        write_skill(&proj_skills, "charlie", "c", None);
        write_skill(&proj_skills, "alpha", "a", None);
        write_skill(&proj_skills, "bravo", "b", None);

        let sections = load_skill_sections(&cwd, &lingxi_home);
        let names: Vec<&str> = sections[0].rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn plain_md_file_directly_under_skills_is_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (cwd, lingxi_home) = fixture(tmp.path());
        let proj_skills = cwd.join(".lingxi").join("skills");
        std::fs::create_dir_all(&proj_skills).expect("mkdir skills");
        // A single `.md` file (NOT a subdir/SKILL.md) — directory format only.
        std::fs::write(
            proj_skills.join("loose.md"),
            "---\nname: loose\ndescription: d\n---\nBody.\n",
        )
        .expect("write loose.md");

        let sections = load_skill_sections(&cwd, &lingxi_home);
        assert!(
            sections.is_empty(),
            "a plain .md file under skills/ must be ignored (directory format only)"
        );
    }
}
