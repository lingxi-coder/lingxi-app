//! Streaming accumulator port of `collapseReadSearchGroups` /
//! `createCollapsedGroup` + `getSearchReadSummaryText`
//! (`utils/collapseReadSearch.ts`). See design doc §4.

use std::collections::BTreeSet;

use serde_json::Value;

use super::classify::classify;

/// A contiguous run of collapsible read/search/list tool uses, folded into one
/// cell. Mutated in the transcript's active slot while streaming, then committed
/// once on a breaker.
#[derive(Debug, Default)]
pub struct CollapseGroup {
    search_count: u64,
    read_file_paths: BTreeSet<String>,
    read_operation_count: u64,
    list_count: u64,
    repl_count: u64,
    entries: Vec<String>,
    latest_hint: Option<String>,
}

impl CollapseGroup {
    /// An empty group.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb a tool-use START. Returns `false` when the use is NOT collapsible
    /// (the caller must finalize this group and process the breaker instead).
    pub fn absorb_start(&mut self, tool: &str, input: &Value) -> bool {
        let info = classify(tool, input);
        if !info.is_collapsible {
            return false;
        }
        self.entries.push(verbose_entry(tool, input));
        // Silent meta-ops (REPL/Snip/ToolSearch) contribute no count.
        if info.is_absorbed_silently {
            if info.is_repl {
                self.repl_count += 1;
            }
            return true;
        }
        if info.is_search {
            self.search_count += 1;
        }
        if info.is_read {
            match read_path(tool, input) {
                Some(path) => {
                    self.latest_hint = Some(display_path(&path));
                    self.read_file_paths.insert(path);
                }
                None => self.read_operation_count += 1,
            }
        }
        if info.is_list {
            self.list_count += 1;
        }
        true
    }

    /// `createCollapsedGroup` read count: unique read file paths, falling back to
    /// the pathless read-operation count when there were no path-based reads (so
    /// `Read(x)` + `Bash(wc -l x)` stays 1, not 2).
    #[must_use]
    pub fn read_count(&self) -> u64 {
        if self.read_file_paths.is_empty() {
            self.read_operation_count
        } else {
            self.read_file_paths.len() as u64
        }
    }

    /// Number of search (Grep/Glob/bash-grep) uses.
    #[must_use]
    pub fn search_count(&self) -> u64 {
        self.search_count
    }

    /// Number of directory listings.
    #[must_use]
    pub fn list_count(&self) -> u64 {
        self.list_count
    }

    /// Number of REPL invocations.
    #[must_use]
    pub fn repl_count(&self) -> u64 {
        self.repl_count
    }

    /// The latest read hint (the `⎿` line while active).
    #[must_use]
    pub fn latest_hint(&self) -> Option<&str> {
        self.latest_hint.as_deref()
    }

    /// The verbose-expansion entries (ctrl+o).
    #[must_use]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// True when the group carries no counted fold yet (only silent absorbs) —
    /// the caller skips committing an empty badge.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.search_count == 0
            && self.list_count == 0
            && self.repl_count == 0
            && self.read_file_paths.is_empty()
            && self.read_operation_count == 0
    }

    /// The comma-joined summary line (present-tense + trailing `…` when
    /// `is_active`, past-tense when finalized).
    #[must_use]
    pub fn summary_text(&self, is_active: bool) -> String {
        search_read_summary_text(
            self.search_count,
            self.read_count(),
            self.list_count,
            self.repl_count,
            is_active,
        )
    }
}

/// The file path a read targets, for the `⎿` hint and unique-file dedup. `None`
/// for pathless bash reads (`cat` in a pipe) — those count as read operations.
fn read_path(tool: &str, input: &Value) -> Option<String> {
    if tool == "Read" {
        return input
            .get("file_path")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    None
}

/// `getDisplayPath` — claude-code strips the cwd prefix; the accumulator only
/// has the raw path, so keep it verbatim (cwd-stripping is a display refinement).
fn display_path(path: &str) -> String {
    path.to_string()
}

/// One verbose-expansion entry (ctrl+o): the tool + truncated JSON input.
fn verbose_entry(tool: &str, input: &Value) -> String {
    let raw = input.to_string();
    let shown: String = raw.chars().take(80).collect();
    format!("{tool} {shown}")
}

/// Port of `getSearchReadSummaryText` (`collapseReadSearch.ts:961`). Ordered
/// parts, comma-joined, first part capitalized, present-tense + trailing `…`
/// while active. Memory/team-memory parts are gated off in the inline TUI.
#[must_use]
pub fn search_read_summary_text(
    search_count: u64,
    read_count: u64,
    list_count: u64,
    repl_count: u64,
    is_active: bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if search_count > 0 {
        let verb = pick_verb(is_active, parts.is_empty(), "Searching for", "Searched for");
        let noun = if search_count == 1 { "pattern" } else { "patterns" };
        parts.push(format!("{verb} {search_count} {noun}"));
    }
    if read_count > 0 {
        let verb = pick_verb(is_active, parts.is_empty(), "Reading", "Read");
        let noun = if read_count == 1 { "file" } else { "files" };
        parts.push(format!("{verb} {read_count} {noun}"));
    }
    if list_count > 0 {
        let verb = pick_verb(is_active, parts.is_empty(), "Listing", "Listed");
        let noun = if list_count == 1 {
            "directory"
        } else {
            "directories"
        };
        parts.push(format!("{verb} {list_count} {noun}"));
    }
    if repl_count > 0 {
        let verb = if is_active { "REPL'ing" } else { "REPL'd" };
        let noun = if repl_count == 1 { "time" } else { "times" };
        parts.push(format!("{verb} {repl_count} {noun}"));
    }

    let text = parts.join(", ");
    if is_active {
        format!("{text}…")
    } else {
        text
    }
}

/// Present vs past verb, capitalized only when it's the first part (matching the
/// reference `parts.length === 0 ? 'Reading' : 'reading'`).
fn pick_verb(is_active: bool, first: bool, active_cap: &str, done_cap: &str) -> String {
    let base = if is_active { active_cap } else { done_cap };
    if first {
        base.to_string()
    } else {
        lowercase_first(base)
    }
}

fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contiguous_reads_dedup_unique_paths() {
        let mut g = CollapseGroup::new();
        assert!(g.absorb_start("Read", &json!({"file_path": "a.rs"})));
        assert!(g.absorb_start("Read", &json!({"file_path": "a.rs"})));
        assert!(g.absorb_start("Read", &json!({"file_path": "b.rs"})));
        assert_eq!(g.read_count(), 2);
        assert_eq!(g.latest_hint(), Some("b.rs"));
    }

    #[test]
    fn pathless_bash_read_falls_back_to_op_count() {
        let mut g = CollapseGroup::new();
        assert!(g.absorb_start("Bash", &json!({"command": "cat a.rs"})));
        assert_eq!(g.read_count(), 1);
    }

    #[test]
    fn non_collapsible_returns_false() {
        let mut g = CollapseGroup::new();
        assert!(!g.absorb_start("Edit", &json!({"file_path": "a"})));
    }

    #[test]
    fn active_present_tense_with_ellipsis() {
        assert_eq!(
            search_read_summary_text(0, 3, 0, 0, true),
            "Reading 3 files…"
        );
    }

    #[test]
    fn finalized_past_tense_comma_join_first_capital() {
        assert_eq!(
            search_read_summary_text(2, 1, 0, 0, false),
            "Searched for 2 patterns, read 1 file"
        );
    }
}

