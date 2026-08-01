//! Nested memory discovery — the memory that governs a TOUCHED file's location.
//!
//! Ports claude-code `Rop` (2.1.220 @237715260), which runs three passes over a
//! trigger file and returns the memory files that apply to it:
//!
//! ```js
//! let s=await NLu(e,o);                    n.push(...k$o(s,t,e));
//! let {nestedDirs:a,cwdLevelDirs:l}=Aop(e,i);
//! for(let u of a){ let d=await ffo(u,e,o); n.push(...k$o(d,t,e)) }
//! for(let u of l){ let d=await FLu(u,e,o); n.push(...k$o(d,t,e)) }
//! ```
//!
//! | Pass | Oracle | Loads |
//! |---|---|---|
//! | the file itself | `NLu` @230809780 | Managed + User rules, CONDITIONAL only |
//! | each nested dir | `ffo` @230809989 | memory file, dot-dir memory file, local override (all unconditional), then rules (unconditional + conditional) |
//! | each cwd-level dir | `FLu` @230810574 | rules, CONDITIONAL only |
//!
//! Filenames come from [`memory::lingxi_md::hierarchy`]'s `branding` constants —
//! `LINGXI.md` / `.lingxi`, not the oracle's literals.
//!
//! DIVERGENCE(reason): the oracle filters every pass by the
//! `tengu_paper_halyard` gate, dropping Project/Local when it is on. LingXi has
//! no such gate and the oracle's default is OFF, so all tiers load. No gate was
//! invented.

use super::MemoryFile;
use memory::lingxi_md::hierarchy::{self, HierarchyEntry};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Session-scoped dedup state for [`discover`].
///
/// Two distinct sets, matching how the oracle threads its `seen` set separately
/// from the per-file `@import` guard:
///
/// - `probed` stops a directory's memory file being surfaced twice when two
///   trigger files share an ancestor.
/// - `expanded` is the `@import` cycle/dedup guard handed to
///   [`memory::lingxi_md::loader::expand_memory_file`], so a file imported by
///   several rules expands once.
///
/// Both live for the SESSION, not the call — a per-call set would re-surface
/// everything on every turn.
#[derive(Debug, Default)]
pub struct DiscoveryState {
    probed: HashSet<PathBuf>,
    expanded: HashSet<PathBuf>,
}

impl DiscoveryState {
    /// Fresh state for a new session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Expand probed hierarchy entries into [`MemoryFile`]s.
///
/// Mirrors `RealMemoryHierarchyProvider::load`'s mapping so both paths agree on
/// tier inheritance, the local-override flag and the disk-fidelity pair.
/// `include_external` is always `false` here — the oracle passes
/// `includeExternal:!1` on every nested pass.
fn expand(
    entries: Vec<HierarchyEntry>,
    cwd: &Path,
    home: &Path,
    state: &mut DiscoveryState,
) -> Vec<MemoryFile> {
    let mut out = Vec::new();
    for e in entries {
        let expanded = memory::lingxi_md::loader::expand_memory_file(
            &e.path,
            &mut state.expanded,
            false,
            cwd,
            Some(home),
            0,
        );
        for (idx, entry) in expanded.into_iter().enumerate() {
            let body = entry.body.trim().to_string();
            if body.is_empty() {
                continue;
            }
            out.push(MemoryFile {
                path: entry.path,
                body,
                is_local_override: idx == 0 && e.is_local_override,
                tier: e.tier,
                globs: entry.globs,
                raw_content: entry.raw_content,
                content_differs_from_disk: entry.content_differs_from_disk,
            });
        }
    }
    out
}

/// Keep the conditional (`paths:`-gated) files whose globs match `trigger`.
fn matching_conditional(files: Vec<MemoryFile>, trigger: &Path, cwd: &Path) -> Vec<MemoryFile> {
    files
        .into_iter()
        .filter(|f| {
            f.globs.is_some() && super::conditional_rules::rule_matches_touched_file(f, trigger, cwd)
        })
        .collect()
}

/// Discover every memory file that governs `trigger`'s location.
///
/// Returns them in the oracle's emission order. Callers own the "has the model
/// already seen this" question — this function is pure discovery and does NOT
/// consult `read_file_state`.
#[must_use]
pub fn discover(
    trigger: &Path,
    cwd: &Path,
    home: &Path,
    managed_dir: Option<&Path>,
    state: &mut DiscoveryState,
) -> Vec<MemoryFile> {
    let mut out: Vec<MemoryFile> = Vec::new();

    // Pass 1 — `NLu`: Managed + User rules, CONDITIONAL only, matched against
    // the trigger. Unconditional Managed/User memory is already in the eager
    // block, so only the glob-gated half can be news here.
    {
        let mut entries = Vec::new();
        if let Some(managed) = managed_dir {
            hierarchy::probe_managed_rules(managed, &mut entries, &mut state.probed);
        }
        hierarchy::probe_user_rules(home, &mut entries, &mut state.probed);
        let files = expand(entries, cwd, home, state);
        out.extend(matching_conditional(files, trigger, cwd));
    }

    let ancestors = hierarchy::split_ancestors(trigger, cwd);

    // Pass 2 — `ffo` per nested dir. Unconditional files first, then the
    // matching conditional ones: the oracle scans `rules` twice (`ZPt` then
    // `lfo`) and `probe_dir_nested` folds that into one scan, so the split has
    // to happen here to preserve ordering.
    for dir in &ancestors.nested {
        let mut entries = Vec::new();
        hierarchy::probe_dir_nested(dir, &mut entries, &mut state.probed);
        let files = expand(entries, cwd, home, state);
        let (conditional, unconditional): (Vec<_>, Vec<_>) =
            files.into_iter().partition(|f| f.globs.is_some());
        out.extend(unconditional);
        out.extend(matching_conditional(conditional, trigger, cwd));
    }

    // Pass 3 — `FLu` per cwd-level dir: rules only, conditional only.
    for dir in &ancestors.cwd_level {
        let mut entries = Vec::new();
        hierarchy::probe_dir_cwd_level(dir, &mut entries, &mut state.probed);
        let files = expand(entries, cwd, home, state);
        out.extend(matching_conditional(files, trigger, cwd));
    }

    out
}
