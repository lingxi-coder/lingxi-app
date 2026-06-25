//! `@` file-ref completion: a dropdown of cwd path entries. Opens when an `@`
//! token is being typed. The filter/select logic takes the candidate `Vec`
//! as input so it's testable without touching the filesystem; `read_cwd_entries`
//! is the only fs-touching fn and is exercised by a separate fs test.
//!
//! Literal lock (design §2.8): inserts `@<path> ` (trailing space), mirroring
//! claude-code `QuickOpenDialog` handleInsert. Empty-state strings copied below.

use std::path::Path;

use iocraft::prelude::*;

use super::fuzzy::filtered_ranked;
use crate::theme::Theme;

/// Max dropdown rows (shared with the palette; claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// claude-code `QuickOpenDialog` empty-state literal when a query is present.
pub const EMPTY_WITH_QUERY: &str = "No matching files";
/// claude-code `QuickOpenDialog` empty-state literal for an empty query.
pub const EMPTY_NO_QUERY: &str = "Start typing to search…";

/// `@` completion overlay state.
#[derive(Debug, Clone, Default)]
pub struct CompletionState {
    /// Whether the dropdown is shown.
    pub open: bool,
    /// The text typed after the active `@` (the partial path).
    pub filter: String,
    /// Index into filtered candidates.
    pub selected: usize,
    /// Candidate paths (relative to cwd). Populated when the overlay opens.
    pub candidates: Vec<String>,
}

/// (cp-07) claude-code `findLongestCommonPrefix`: the longest shared prefix
/// across every string in `items`, or `""` if `items` is empty.
#[must_use]
fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for s in &items[1..] {
        let chars: Vec<char> = s.chars().collect();
        let n = prefix.len().min(chars.len());
        let mut i = 0;
        while i < n && prefix[i] == chars[i] {
            i += 1;
        }
        prefix.truncate(i);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

/// Find the active `@` token: the substring from the last `@` to the cursor,
/// iff that `@` is at the start or preceded by whitespace and the token has no
/// space. Returns `(at_byte_index, partial)` or `None`.
#[must_use]
pub fn active_at_token(prompt: &str, cursor: usize) -> Option<(usize, String)> {
    // Clamp to the nearest char boundary at or below `cursor` so `&prompt[..cursor]`
    // never panics. The live editor keeps `prompt_cursor` on boundaries today, but
    // this is a public fn and M7-08/09 vim motions manipulate the cursor freely.
    let mut cursor = cursor.min(prompt.len());
    while cursor > 0 && !prompt.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let head = &prompt[..cursor];
    let at = head.rfind('@')?;
    let preceded_ok = at == 0 || head[..at].ends_with(|c: char| c.is_whitespace());
    let partial = &head[at + 1..];
    if preceded_ok && !partial.contains(char::is_whitespace) {
        Some((at, partial.to_string()))
    } else {
        None
    }
}

impl CompletionState {
    /// Recompute open-state + filter from the prompt + cursor, using a
    /// pre-supplied candidate list (so tests inject candidates; the live path
    /// calls `read_cwd_entries` first — see Task 9).
    pub fn sync(&mut self, prompt: &str, cursor: usize, candidates: &[String]) {
        if let Some((_, partial)) = active_at_token(prompt, cursor) {
            if !self.open || partial != self.filter {
                self.selected = 0;
            }
            self.open = true;
            self.filter = partial;
            self.candidates = candidates.to_vec();
            let max = self.rows().len();
            if max == 0 {
                self.selected = 0;
            } else if self.selected >= max {
                self.selected = max - 1;
            }
        } else {
            self.open = false;
            self.filter.clear();
            self.selected = 0;
            self.candidates.clear();
        }
    }

    /// Filtered, ranked candidate paths for the current filter.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        filtered_ranked(&self.filter, &self.candidates)
            .into_iter()
            .map(str::to_string)
            .collect()
    }
}

/// Outcome of a completion key. `Accept` carries the rewritten prompt + cursor
/// because inserting a path edits the buffer in place (replacing the `@token`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKeyOutcome {
    /// Navigation handled; key swallowed.
    Consumed,
    /// Commit the selected path. The dispatcher sets `new_prompt`/`new_cursor`.
    Accept {
        /// The full prompt buffer after inserting `@<path> `.
        new_prompt: String,
        /// The new cursor byte index (end of the inserted token).
        new_cursor: usize,
    },
    /// `Esc` — close; key swallowed.
    Dismiss,
    /// Nothing actionable — fall through to default input.
    PassThrough,
}

impl CompletionState {
    /// Navigation-only handler (no prompt rewrite). Used for Up/Down/Esc.
    pub fn handle_key(&mut self, code: KeyCode) -> CompletionKeyOutcome {
        let len = self.rows().len();
        match code {
            KeyCode::Down => {
                // (cp-04) wrap: last → first.
                if len > 0 {
                    self.selected = (self.selected + 1) % len;
                }
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Up => {
                // (cp-04) wrap: first → last.
                if len > 0 {
                    self.selected = (self.selected + len - 1) % len;
                }
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Esc => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
                CompletionKeyOutcome::Dismiss
            }
            _ => CompletionKeyOutcome::PassThrough,
        }
    }

    /// Tab/Enter handler that rewrites the prompt in place: replaces the active
    /// `@token` (located via `active_at_token`) with the completion.
    ///
    /// Enter always commits the highlighted row outright (`@<selected> `,
    /// trailing space, overlay closes). Tab is two-stage (cp-07, claude-code
    /// `handleTab`'s file branch): if every currently-filtered row shares a
    /// prefix longer than what's typed, it completes to that shared prefix
    /// (no trailing space) and leaves the overlay OPEN — re-filtering against
    /// the longer prefix is what makes a shared directory prefix "drill
    /// down" into that directory's children, with no special-casing needed.
    pub fn handle_key_with_prompt(
        &mut self,
        code: KeyCode,
        prompt: &str,
        cursor: usize,
    ) -> CompletionKeyOutcome {
        match code {
            KeyCode::Tab | KeyCode::Enter => {
                let rows = self.rows();
                let Some(sel) = rows.get(self.selected).cloned() else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let Some((at, partial)) = active_at_token(prompt, cursor) else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let cursor = cursor.min(prompt.len());
                let lcp = (code == KeyCode::Tab)
                    .then(|| longest_common_prefix(&rows))
                    .filter(|p| p.chars().count() > partial.chars().count());
                let (insert, keep_open) = match &lcp {
                    Some(prefix) => (format!("@{prefix}"), true),
                    None => (format!("@{sel} "), false),
                };
                let mut new_prompt = String::with_capacity(prompt.len() + insert.len());
                new_prompt.push_str(&prompt[..at]);
                new_prompt.push_str(&insert);
                let new_cursor = new_prompt.len();
                new_prompt.push_str(&prompt[cursor..]);
                if !keep_open {
                    // Full commit: close. (Leaving `candidates` intact in the
                    // `keep_open` case matters — the caller re-syncs using
                    // `self.candidates` right after, so clearing it here
                    // would throw away the cached listing mid drill-down.)
                    self.open = false;
                    self.filter.clear();
                    self.selected = 0;
                    self.candidates.clear();
                }
                CompletionKeyOutcome::Accept {
                    new_prompt,
                    new_cursor,
                }
            }
            _ => self.handle_key(code),
        }
    }
}

/// claude-code `getTopLevelPaths`: the immediate (non-recursive) entries of
/// `dir`, directories suffixed with `/`, sorted ASCII-ascending. No dotfile
/// filtering (mirrors raw `fs.readdir`). Used for the bare-`@`/empty-partial
/// case; a non-empty partial uses [`list_project_paths`] instead (cp-05).
/// Errors → empty list (the overlay just shows the empty state).
#[must_use]
pub fn read_cwd_entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some(if is_dir { format!("{name}/") } else { name })
        })
        .collect();
    out.sort_unstable();
    out
}

/// (cp-05) claude-code `getPathsForSuggestions`: a recursive project file
/// listing (tracked + untracked, gitignore-respecting) plus the unique set of
/// their parent directories (trailing `/`), paths relative to `dir`. Tries
/// `git ls-files` first (fast path for git repos); falls back to a
/// gitignore-aware recursive walk (the `ignore` crate — the in-process
/// equivalent of claude-code's `rg --hidden` fallback) for non-git dirs or if
/// git is unavailable.
#[must_use]
pub fn list_project_paths(dir: &Path) -> Vec<String> {
    let mut files = git_ls_files(dir).unwrap_or_else(|| walk_project_files(dir));
    files.sort_unstable();
    files.dedup();
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in &files {
        let mut p = Path::new(f.as_str());
        while let Some(parent) = p.parent() {
            let s = parent.to_string_lossy();
            if s.is_empty() {
                break;
            }
            dirs.insert(format!("{s}/"));
            p = parent;
        }
    }
    let mut out: Vec<String> = dirs.into_iter().collect();
    out.append(&mut files);
    out
}

/// `git ls-files` (tracked) + `git ls-files --others --exclude-standard`
/// (untracked, gitignore-excluded) merged. `None` when `dir` isn't a git repo
/// or the `git` binary is unavailable — the caller falls back to a walk.
fn git_ls_files(dir: &Path) -> Option<Vec<String>> {
    let run = |args: &[&str]| -> Option<Vec<String>> {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.quotepath=false")
            .args(args)
            .current_dir(dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        )
    };
    let mut tracked = run(&["ls-files", "--recurse-submodules"])?;
    if let Some(mut untracked) = run(&["ls-files", "--others", "--exclude-standard"]) {
        tracked.append(&mut untracked);
    }
    Some(tracked)
}

/// Recursive gitignore-aware fallback walk (non-git dirs): every file under
/// `dir` except VCS metadata dirs, dotfiles INCLUDED (claude-code's ripgrep
/// fallback passes `--hidden`).
fn walk_project_files(dir: &Path) -> Vec<String> {
    use ignore::{overrides::OverrideBuilder, WalkBuilder};
    let mut wb = WalkBuilder::new(dir);
    wb.hidden(false);
    let mut ov = OverrideBuilder::new(dir);
    for pat in ["!.git/", "!.svn/", "!.hg/", "!.bzr/", "!.jj/", "!.sl/"] {
        let _ = ov.add(pat);
    }
    if let Ok(overrides) = ov.build() {
        wb.overrides(overrides);
    }
    wb.build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0 && !e.file_type().is_some_and(|t| t.is_dir()))
        .filter_map(|e| {
            e.path()
                .strip_prefix(dir)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

/// Props for the completion dropdown overlay.
#[derive(Default, Props)]
pub struct CompletionOverlayProps {
    /// Filtered candidate paths.
    pub rows: Vec<String>,
    /// Highlighted row index.
    pub selected: usize,
    /// Whether the active `@` token has no partial text yet (drives the
    /// empty-state literal).
    pub empty_query: bool,
    /// (M7-15) Active palette — the selected-row `suggestion` accent + dim
    /// rest are centralized here.
    pub theme: Theme,
}

/// Render the `@` completion dropdown. Each row is `+ <path>` (claude-code
/// file icon `+`). The empty state shows the QuickOpenDialog literal.
#[component]
pub fn CompletionOverlay(props: &CompletionOverlayProps) -> impl Into<AnyElement<'static>> {
    let selected = props.selected;
    let theme = props.theme;
    if props.rows.is_empty() {
        // (cp-06) The inline `@`-overlay collapses to nothing when there are no
        // matches — claude-code does NOT render a "No matching files" /
        // "Start typing to search…" row inline (that literal belongs to the
        // full QuickOpenDialog surface, not the inline dropdown).
        let _ = (EMPTY_NO_QUERY, EMPTY_WITH_QUERY, theme);
        return element! { View(height: 0) }.into_any();
    }
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, path)| {
                let line = format!("+ {path}");
                // (M7-15) Centralized: selected row uses the theme's `suggestion`
                // accent, the rest dim.
                let color = if i == selected { theme.suggestion } else { theme.dim };
                element! {
                    View(height: 1) { Text(content: line, color: color) }
                }
            }))
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<String> {
        vec![
            "src/main.rs".into(),
            "src/lib.rs".into(),
            "README.md".into(),
        ]
    }

    #[test]
    fn active_token_at_start() {
        assert_eq!(active_at_token("@src", 4), Some((0, "src".into())));
    }

    #[test]
    fn active_token_after_whitespace() {
        assert_eq!(active_at_token("see @lib", 8), Some((4, "lib".into())));
    }

    #[test]
    fn no_token_without_at() {
        assert_eq!(active_at_token("plain text", 5), None);
    }

    #[test]
    fn no_token_for_plain_text_at_end() {
        // Important #1 gate: no `@` ⇒ no active token ⇒ live path skips the fs read.
        assert_eq!(active_at_token("hello", 5), None);
    }

    #[test]
    fn token_active_for_at_prefix() {
        // Important #1 gate: an active `@s` token ⇒ live path performs the fs read.
        assert_eq!(active_at_token("@s", 2), Some((0, "s".into())));
    }

    #[test]
    fn cursor_off_char_boundary_does_not_panic() {
        // Minor #2: `@café` — `é` is 2 bytes (bytes 4..6); cursor 5 lands mid-`é`,
        // which is NOT a UTF-8 char boundary. Slicing `&prompt[..5]` would panic;
        // the entry clamp must walk it down to a boundary (4) instead.
        let out = active_at_token("@café", 5);
        // Clamped to byte 4 ⇒ token is "caf" starting at the `@` (byte 0).
        assert_eq!(out, Some((0, "caf".into())));
    }

    #[test]
    fn at_mid_word_is_not_a_token() {
        // email-like — `@` not preceded by whitespace → not a completion token.
        assert_eq!(active_at_token("user@host", 9), None);
    }

    #[test]
    fn opens_and_filters_on_at() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        let rows = c.rows();
        assert!(rows.iter().all(|r| r.contains("src")));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn closes_when_token_gone() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        c.sync("plain", 5, &cands());
        assert!(!c.open);
    }

    #[test]
    fn down_up_move_and_clamp() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands()); // 2 rows
        assert_eq!(c.selected, 0);
        c.handle_key(KeyCode::Down);
        assert_eq!(c.selected, 1);
        c.handle_key(KeyCode::Down); // (cp-04) wrap last → first
        assert_eq!(c.selected, 0);
        c.handle_key(KeyCode::Up); // (cp-04) wrap first → last
        assert_eq!(c.selected, 1);
    }

    #[test]
    fn enter_inserts_selected_path_with_at_and_trailing_space() {
        let mut c = CompletionState::default();
        // prompt is "@s", cursor 2; selecting replaces the @token in place.
        c.sync("@s", 2, &cands());
        let sel = c.rows()[c.selected].clone();
        let outcome = c.handle_key_with_prompt(KeyCode::Enter, "@s", 2);
        match outcome {
            CompletionKeyOutcome::Accept {
                new_prompt,
                new_cursor,
            } => {
                let expected = format!("@{sel} ");
                assert_eq!(new_prompt, expected);
                assert_eq!(new_cursor, expected.len());
            }
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!c.open);
    }

    #[test]
    fn tab_completes_to_shared_prefix_and_keeps_overlay_open() {
        // (cp-07) "src/lib.rs" and "src/main.rs" both match "s"; their shared
        // prefix "src/" is longer than the typed "s", so Tab completes to
        // the prefix (no trailing space) and leaves the overlay open/intact
        // for the caller's re-sync (drill-down), instead of committing.
        let mut c = CompletionState::default();
        c.sync("@s", 2, &cands());
        assert_eq!(c.rows(), vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]);
        let outcome = c.handle_key_with_prompt(KeyCode::Tab, "@s", 2);
        match outcome {
            CompletionKeyOutcome::Accept {
                new_prompt,
                new_cursor,
            } => {
                assert_eq!(new_prompt, "@src/");
                assert_eq!(new_cursor, "@src/".len());
            }
            other => panic!("expected Accept, got {other:?}"),
        }
        // Not closed, and the candidate cache survives the call — the
        // caller (root.rs) re-syncs using `self.candidates` right after.
        assert!(c.open);
        assert!(!c.candidates.is_empty());
    }

    #[test]
    fn tab_on_unambiguous_match_completes_then_commits_on_second_press() {
        // (cp-07) Shell-style two-stage Tab: a single full match still
        // completes-without-committing on the first Tab (claude-code doesn't
        // special-case "only one row" — `commonPrefix` is just that row's
        // full text, still longer than the typed partial); the second Tab,
        // once the prefix typed equals the full match, has no further
        // common-prefix gain and commits as a normal selection.
        let mut c = CompletionState::default();
        c.sync("@README", 7, &cands());
        assert_eq!(c.rows(), vec!["README.md".to_string()]);
        let outcome = c.handle_key_with_prompt(KeyCode::Tab, "@README", 7);
        let CompletionKeyOutcome::Accept { new_prompt, new_cursor } = outcome else {
            panic!("expected Accept, got {outcome:?}");
        };
        assert_eq!(new_prompt, "@README.md");
        assert!(c.open, "first Tab completes but stays open");

        // Mirror root.rs's post-Accept resync: re-derive filter/candidates
        // from the rewritten prompt using the still-cached candidate list.
        let candidates = c.candidates.clone();
        c.sync(&new_prompt, new_cursor, &candidates);
        assert_eq!(c.filter, "README.md");

        let outcome2 = c.handle_key_with_prompt(KeyCode::Tab, &new_prompt, new_cursor);
        match outcome2 {
            CompletionKeyOutcome::Accept { new_prompt, .. } => {
                assert_eq!(new_prompt, "@README.md ", "second Tab commits with trailing space");
            }
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!c.open, "second Tab closes the overlay");
    }

    #[test]
    fn esc_dismisses() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(matches!(
            c.handle_key(KeyCode::Esc),
            CompletionKeyOutcome::Dismiss
        ));
        assert!(!c.open);
    }

    #[test]
    fn read_cwd_entries_includes_dotfiles_dirs_trailing_sep_and_is_sorted() {
        // (cp-05) Top-level listing mirrors raw `fs.readdir`: no dotfile
        // filtering, directories get a trailing `/`.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let entries = read_cwd_entries(dir.path());
        assert_eq!(
            entries,
            vec![
                ".hidden".to_string(),
                "a.txt".to_string(),
                "b.txt".to_string(),
                "sub/".to_string(),
            ]
        );
    }

    #[test]
    fn list_project_paths_is_recursive_and_includes_dirs() {
        // (cp-05) Non-git dir: walk_project_files fallback. Recursive, dirs
        // get a trailing `/`, files matched by full relative path.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::write(dir.path().join("src/nested/deep.rs"), "").unwrap();
        std::fs::write(dir.path().join("top.txt"), "").unwrap();
        let paths = list_project_paths(dir.path());
        assert!(paths.contains(&"src/".to_string()), "{paths:?}");
        assert!(paths.contains(&"src/nested/".to_string()), "{paths:?}");
        assert!(paths.contains(&"src/nested/deep.rs".to_string()), "{paths:?}");
        assert!(paths.contains(&"top.txt".to_string()), "{paths:?}");
    }

    #[test]
    fn list_project_paths_respects_gitignore_via_git_ls_files() {
        // Exercises the `git_ls_files` fast path: tracked + untracked-but-
        // not-ignored files are included; gitignored files are excluded.
        // (The non-git fallback walk intentionally mirrors ripgrep's own
        // default `require_git` behavior — `.gitignore` is only honored
        // inside an actual git work tree — so this is the realistic path.)
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t.test"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "").unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "").unwrap();
        git(&["add", ".gitignore", "tracked.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        let paths = list_project_paths(dir.path());
        assert!(!paths.contains(&"ignored.txt".to_string()), "{paths:?}");
        assert!(paths.contains(&"kept.txt".to_string()), "{paths:?}");
        assert!(paths.contains(&"tracked.txt".to_string()), "{paths:?}");
    }
}
