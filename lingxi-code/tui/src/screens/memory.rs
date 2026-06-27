//! Memory file editor screen (M7-14).
//!
//! A `MemoryFileSelector` lists the project/user LINGXI.md tiers (resolved
//! via [`memory::lingxi_md::hierarchy::walk`]); selecting a tier
//! opens an inline edit view. Reads go through the M3 loader
//! ([`memory::lingxi_md::loader::load_file`]); writes go back to the
//! same on-disk path (the M3 store — §4 R7, no new persistence).

use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use iocraft::prelude::*;

use memory::lingxi_md::hierarchy::{walk, FILE_NAME};
use memory::lingxi_md::loader::{load_file, LoaderError};

/// One resolved memory tier the selector lists. The project/user tiers are
/// always offered (even when the file does not exist yet — marked `(new)`);
/// additional discovered project-parent LINGXI.md files are appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTierEntry {
    /// Selector label, literal-locked to claude-code (`"Project memory"`,
    /// `"User memory"`, or a display path for parent files).
    pub label: String,
    /// One-line description shown under the label (literal-locked).
    pub description: String,
    /// On-disk path of the LINGXI.md file (the M3 store target).
    pub path: PathBuf,
    /// Whether the file currently exists. `false` → the row shows `" (new)"`.
    pub exists: bool,
}

/// Resolve the memory tiers to list, innermost-first. The project tier is
/// `<cwd>/LINGXI.md` and the user tier is `<home>/.lingxi/LINGXI.md`; both
/// are always present (creatable). Any other LINGXI.md the walker finds
/// (project parents) is appended after, by display path.
///
/// Mirrors `claude-code/src/components/memory/MemoryFileSelector.tsx`
/// (single-user subset: auto-memory / team / agent folders are M8).
/// `true` when `cwd` or any ancestor contains a `.git` entry (dir for a normal
/// repo, file for a worktree/submodule) — the nearest-`.git`-ancestor scan used
/// across the TUI (mirrors `skills.rs`). Drives the MEM-1 git-conditional
/// project-memory description.
fn cwd_is_in_git_repo(cwd: &Path) -> bool {
    cwd.ancestors().any(|dir| dir.join(".git").exists())
}

#[must_use]
pub fn memory_tiers(cwd: &Path, home: &Path) -> Vec<MemoryTierEntry> {
    let project_path = cwd.join(FILE_NAME);
    // User-tier LINGXI.md must resolve via `$LINGXI_CONFIG_DIR` (else `~/.claude`)
    // — the SAME env-aware resolver the prompt loader (`hierarchy::walk`) uses, so
    // the /memory editor writes exactly the file the system prompt loads (no
    // split-brain when `$LINGXI_CONFIG_DIR` is set).
    let user_path = memory::lingxi_md::user_config_dir(home).join(FILE_NAME);

    let mut tiers = vec![
        MemoryTierEntry {
            label: "Project memory".to_string(),
            // (MEM-1) git-conditional, matching MemoryFileSelector.tsx:88,93:
            // "Checked in at ./LINGXI.md" inside a git repo, else "Saved in …".
            description: if cwd_is_in_git_repo(cwd) {
                "Checked in at ./LINGXI.md".to_string()
            } else {
                "Saved in ./LINGXI.md".to_string()
            },
            exists: project_path.is_file(),
            path: project_path.clone(),
        },
        MemoryTierEntry {
            label: "User memory".to_string(),
            description: "Saved in ~/.lingxi/LINGXI.md".to_string(),
            exists: user_path.is_file(),
            path: user_path.clone(),
        },
    ];

    // Append any other discovered LINGXI.md (project parents) not already
    // covered by the project/user rows, in walk order. The Managed tier is not
    // surfaced in this editor (read-only enterprise policy, not user-editable),
    // so pass `None`.
    let h = walk(cwd, home, None);
    for entry in h.entries {
        if entry.path == project_path || entry.path == user_path {
            continue;
        }
        let display = entry.path.display().to_string();
        tiers.push(MemoryTierEntry {
            label: display.clone(),
            // (MEM-2) These are nested-directory discoveries (claude-code
            // `getMemoryFilesForNestedDirectory`'s walk, `file.isNested`),
            // not `@import`-following (`file.parent`) — so the description
            // is "dynamically loaded", not "@-imported" (MemoryFileSelector
            // .tsx:91-99 picks `@-imported` only when `file.parent` is set).
            description: "dynamically loaded".to_string(),
            exists: true,
            path: entry.path,
        });
    }
    tiers
}

/// Read a tier file's body through the M3 loader. A missing file is NOT an
/// error here — it yields an empty buffer so the editor can create a "new"
/// tier. Oversized / unreadable files surface as `Err`.
///
/// # Errors
/// Returns the loader's I/O error string for files that exist but cannot be
/// read, or a "too large" message for files over the 10 MB cap.
pub fn load_tier_body(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    match load_file(path, None) {
        Ok(loaded) => Ok(loaded.body),
        Err(LoaderError::FileTooLarge { bytes, .. }) => {
            Err(format!("memory file too large: {bytes} bytes"))
        }
        Err(LoaderError::Io(e)) => Err(e),
    }
}

/// Write `body` back to the tier file (the M3 store target). Atomic:
/// writes to a `.lingxi-tmp` sibling then renames over the target, so a
/// crash mid-write never truncates the existing file. Creates parent dirs.
///
/// This is the ONLY write path — it targets exactly the `HierarchyEntry`
/// path the M3 walker/loader resolved (§4 R7: no new persistence layer).
///
/// # Errors
/// Returns the I/O error string on any filesystem failure.
pub fn save_tier_body(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e: io::Error| e.to_string())?;
    }
    let tmp = path.with_extension("md.lingxi-tmp");
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Outcome the caller (`handle_screen_key`) acts on after routing a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryAction {
    /// Nothing for the caller to do (state mutated in place).
    None,
    /// Close the Memory screen (clear `active_screen`).
    CloseScreen,
    /// Leave the editor, return to the tier selector (stay on the screen).
    BackToSelector,
    /// Persist the current buffer to `path`. The caller calls
    /// [`save_tier_body`] and reports success/failure.
    Save {
        /// Target tier path.
        path: PathBuf,
        /// Buffer to write.
        body: String,
    },
}

/// Per-screen state for the Memory editor. Carried inside the
/// `Screen::Memory` variant (M7-11 review: per-screen state lives in the
/// variant, not a parallel `AppState` field).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryScreenState {
    /// Selected tier index in the selector list.
    pub selected: usize,
    /// `true` once a tier is opened for editing.
    pub editing: bool,
    /// Path of the tier being edited (`None` while in the selector).
    pub editing_path: Option<PathBuf>,
    /// Edit buffer (the tier body).
    pub buffer: String,
    /// `true` when the buffer differs from the loaded body (unsaved).
    pub dirty: bool,
    /// Transient status line (e.g. last save result), shown in the footer.
    pub status: Option<String>,
}

impl MemoryScreenState {
    /// Enter the editor on `tier` with `body` as the initial buffer.
    pub fn open_editor(&mut self, tier: &MemoryTierEntry, body: String) {
        self.editing = true;
        self.editing_path = Some(tier.path.clone());
        self.buffer = body;
        self.dirty = false;
        self.status = None;
    }

    /// Return to the selector list (discarding editor focus, keeping the
    /// buffer untouched on disk — Esc never writes).
    pub fn back_to_selector(&mut self) {
        self.editing = false;
        self.editing_path = None;
        self.buffer.clear();
        self.dirty = false;
    }
}

/// Route one key into the Memory screen state. Returns a [`MemoryAction`]
/// the caller acts on. `tiers` is the current selector list (re-resolved by
/// the caller each frame). Esc semantics: in the editor → back to selector;
/// in the selector → close the screen.
pub fn handle_memory_key(
    st: &mut MemoryScreenState,
    tiers: &[MemoryTierEntry],
    key: KeyEvent,
) -> MemoryAction {
    if st.editing {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                st.back_to_selector();
                return MemoryAction::BackToSelector;
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                if let Some(path) = st.editing_path.clone() {
                    return MemoryAction::Save {
                        path,
                        body: st.buffer.clone(),
                    };
                }
                return MemoryAction::None;
            }
            (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
                st.buffer.push(c);
                st.dirty = true;
            }
            (KeyCode::Enter, _) => {
                st.buffer.push('\n');
                st.dirty = true;
            }
            (KeyCode::Backspace, _) => {
                st.buffer.pop();
                st.dirty = true;
            }
            _ => {}
        }
        return MemoryAction::None;
    }
    // Selector mode.
    match key.code {
        KeyCode::Esc => return MemoryAction::CloseScreen,
        KeyCode::Up => st.selected = st.selected.saturating_sub(1),
        KeyCode::Down => {
            let last = tiers.len().saturating_sub(1);
            st.selected = (st.selected + 1).min(last);
        }
        KeyCode::Enter => {
            if let Some(tier) = tiers.get(st.selected) {
                let body = load_tier_body(&tier.path).unwrap_or_default();
                st.open_editor(tier, body);
            }
        }
        _ => {}
    }
    MemoryAction::None
}

/// Props for [`MemoryScreen`]. The tier list + state are cloned from the
/// active `Screen::Memory` variant each frame (iocraft re-renders on the
/// tick).
#[derive(Default, Props)]
pub struct MemoryScreenProps {
    /// Resolved tier list (selector rows).
    pub tiers: Vec<MemoryTierEntry>,
    /// Selected index in the selector.
    pub selected: usize,
    /// `true` when the editor (not the selector) is showing.
    pub editing: bool,
    /// Editor buffer body.
    pub buffer: String,
    /// `true` when there are unsaved changes.
    pub dirty: bool,
    /// Transient footer status (last save result).
    pub status: Option<String>,
}

/// Memory editor screen. Selector list, or the edit view once a tier is
/// opened. Title + footer hints are literal-locked to claude-code.
#[component]
pub fn MemoryScreen(props: &MemoryScreenProps) -> impl Into<AnyElement<'static>> {
    if props.editing {
        let body = props.buffer.clone();
        let dirty_mark = if props.dirty { " *" } else { "" };
        let status = props.status.clone().unwrap_or_default();
        return element! {
            View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
                Text(content: format!("Edit memory{dirty_mark}"), weight: Weight::Bold)
                View(flex_grow: 1.0) { Text(content: body) }
                Text(content: format!("Ctrl-S save · Esc back   {status}"))
            }
        }
        .into_any();
    }
    let selected = props.selected;
    let rows: Vec<AnyElement<'static>> = props
        .tiers
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if i == selected { "❯ " } else { "  " };
            let new_suffix = if t.exists { "" } else { " (new)" };
            let line = format!("{marker}{}{new_suffix}", t.label);
            // Render the literal-locked tier description dimmed beside the
            // label (claude-code MemoryFileSelector layout intent).
            let description = t.description.clone();
            element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: line)
                    Text(content: format!("  {description}"), color: Color::DarkGrey)
                }
            }
            .into_any()
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            Text(content: "Memory files", weight: Weight::Bold)
            #(rows)
            Text(content: "↑/↓ select · Enter edit · Esc close")
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn lists_project_and_user_tiers_with_labels() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(home.join(".lingxi")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("LINGXI.md"), b"# project notes\n").unwrap();
        // user LINGXI.md does NOT exist → still listed, marked (new).

        let tiers = memory_tiers(&cwd, &home);
        // Project (exists) + User (new) — innermost first.
        let proj = tiers.iter().find(|t| t.label == "Project memory").unwrap();
        assert!(proj.exists);
        assert_eq!(proj.path, cwd.join("LINGXI.md"));
        let user = tiers.iter().find(|t| t.label == "User memory").unwrap();
        assert!(!user.exists);
        assert_eq!(user.path, home.join(".lingxi").join("LINGXI.md"));
    }

    #[test]
    fn discovered_parent_lingxi_md_says_dynamically_loaded_not_imported() {
        // (MEM-2) An ancestor-directory LINGXI.md (not cwd's own, not an
        // `@import`) gets claude-code's "dynamically loaded" description —
        // "@-imported" is reserved for `@import`-following (file.parent set).
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo").join("sub");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(home.join(".lingxi")).unwrap();
        let ancestor_md = tmp.path().join("repo").join("LINGXI.md");
        fs::write(&ancestor_md, b"# ancestor notes\n").unwrap();
        fs::write(cwd.join("LINGXI.md"), b"# project notes\n").unwrap();

        let tiers = memory_tiers(&cwd, &home);
        let discovered = tiers
            .iter()
            .find(|t| t.path == ancestor_md)
            .expect("ancestor LINGXI.md discovered as its own tier row");
        assert_eq!(discovered.description, "dynamically loaded");
    }

    #[test]
    fn project_description_is_git_conditional() {
        // (MEM-1) `.git` present at cwd → "Checked in at"; absent → "Saved in".
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        let proj_desc = |tiers: &[MemoryTierEntry]| {
            tiers
                .iter()
                .find(|t| t.label == "Project memory")
                .unwrap()
                .description
                .clone()
        };
        // No `.git` anywhere under the fresh tempdir → "Saved in".
        assert_eq!(proj_desc(&memory_tiers(&cwd, &home)), "Saved in ./LINGXI.md");
        // Add a `.git` dir at cwd → "Checked in at".
        fs::create_dir_all(cwd.join(".git")).unwrap();
        assert_eq!(
            proj_desc(&memory_tiers(&cwd, &home)),
            "Checked in at ./LINGXI.md"
        );
    }

    #[test]
    fn empty_dirs_still_offer_creatable_project_and_user_tiers() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let tiers = memory_tiers(&cwd, &home);
        assert!(tiers
            .iter()
            .any(|t| t.label == "Project memory" && !t.exists));
        assert!(tiers.iter().any(|t| t.label == "User memory" && !t.exists));
    }

    #[test]
    fn load_returns_empty_for_missing_then_save_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("LINGXI.md");
        // Missing file → empty editable buffer (a "new" tier).
        assert_eq!(load_tier_body(&path).unwrap(), "");
        save_tier_body(&path, "# hello\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# hello\n");
        // Re-load returns the saved body.
        assert_eq!(load_tier_body(&path).unwrap(), "# hello\n");
    }

    #[test]
    fn save_creates_parent_dirs_when_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("deeper").join("LINGXI.md");
        save_tier_body(&path, "x\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
    }

    #[test]
    fn save_is_atomic_no_leftover_tmp() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("LINGXI.md");
        save_tier_body(&path, "body\n").unwrap();
        // The temp sibling must be gone after a successful rename.
        let tmp_sibling = path.with_extension("md.lingxi-tmp");
        assert!(!tmp_sibling.exists());
    }

    #[test]
    fn selector_arrows_move_index_and_enter_opens_editor() {
        let tiers = vec![
            MemoryTierEntry {
                label: "Project memory".into(),
                description: String::new(),
                path: "/p/LINGXI.md".into(),
                exists: true,
            },
            MemoryTierEntry {
                label: "User memory".into(),
                description: String::new(),
                path: "/u/LINGXI.md".into(),
                exists: false,
            },
        ];
        let mut st = MemoryScreenState::default();
        assert_eq!(st.selected, 0);
        handle_memory_key(&mut st, &tiers, key(KeyCode::Down));
        assert_eq!(st.selected, 1);
        // Past the end clamps (no wrap).
        handle_memory_key(&mut st, &tiers, key(KeyCode::Down));
        assert_eq!(st.selected, 1);
        // Enter opens the editor on the selected tier.
        handle_memory_key(&mut st, &tiers, key(KeyCode::Enter));
        assert!(st.editing);
        assert_eq!(
            st.editing_path.as_deref(),
            Some(std::path::Path::new("/u/LINGXI.md"))
        );
    }

    #[test]
    fn editor_typing_marks_dirty_and_esc_returns_to_selector() {
        let tiers = vec![MemoryTierEntry {
            label: "Project memory".into(),
            description: String::new(),
            path: "/p/LINGXI.md".into(),
            exists: true,
        }];
        let mut st = MemoryScreenState::default();
        st.open_editor(&tiers[0], "old".to_string());
        assert!(st.editing && !st.dirty);
        handle_memory_key(&mut st, &tiers, key(KeyCode::Char('!')));
        assert_eq!(st.buffer, "old!");
        assert!(st.dirty);
        // Esc from the editor returns to the selector (does NOT close the screen).
        let action = handle_memory_key(&mut st, &tiers, key(KeyCode::Esc));
        assert_eq!(action, MemoryAction::BackToSelector);
        assert!(!st.editing);
    }

    #[test]
    fn esc_from_selector_requests_close() {
        let tiers = vec![MemoryTierEntry {
            label: "Project memory".into(),
            description: String::new(),
            path: "/p/LINGXI.md".into(),
            exists: true,
        }];
        let mut st = MemoryScreenState::default();
        let action = handle_memory_key(&mut st, &tiers, key(KeyCode::Esc));
        assert_eq!(action, MemoryAction::CloseScreen);
    }

    #[test]
    fn ctrl_s_in_editor_requests_save() {
        let tiers = vec![MemoryTierEntry {
            label: "Project memory".into(),
            description: String::new(),
            path: "/p/LINGXI.md".into(),
            exists: true,
        }];
        let mut st = MemoryScreenState::default();
        st.open_editor(&tiers[0], String::new());
        st.buffer = "new body".into();
        st.dirty = true;
        let save = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let action = handle_memory_key(&mut st, &tiers, save);
        assert_eq!(
            action,
            MemoryAction::Save {
                path: "/p/LINGXI.md".into(),
                body: "new body".into()
            }
        );
    }
}
