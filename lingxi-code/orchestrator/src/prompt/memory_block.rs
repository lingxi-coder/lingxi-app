//! `<memory>...</memory>` formatter + `MemoryHierarchyProvider` trait.
//!
//! The trait abstracts the M3-02 `claude_md::walk` + `load_file` pair
//! so the orchestrator can take a `Arc<dyn MemoryHierarchyProvider>`
//! field and tests can substitute a static fixture without touching
//! the filesystem. Production impl: [`RealMemoryHierarchyProvider`].
#![forbid(unsafe_code)]

use crate::prompt::MemoryFile;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// Loads the CLAUDE.md hierarchy for a given cwd.
///
/// Implementations MUST return the files in the spec-locked splice
/// order: `~/.claude/CLAUDE.md` first, then `<repo>/CLAUDE.md`, then
/// `<repo>/CLAUDE.local.md` (innermost last so it wins the model's
/// recency attention).
#[async_trait]
pub trait MemoryHierarchyProvider: Send + Sync {
    /// Load all CLAUDE.md files relevant to `cwd`. May be empty.
    ///
    /// Errors are NOT propagated — unreadable files are skipped
    /// silently (M3-02 already emits telemetry for oversized files
    /// via `loader::emit_file_too_large`).
    async fn load(&self, cwd: &Path) -> Vec<MemoryFile>;
}

/// Production implementation — wraps `memory::claude_md::walk` +
/// `expand_memory_file`. Reverses the walk order so the returned vec is in
/// spec splice order (home → repo → local-override), and recursively splices
/// each file's `@import` references in directly after it (parity with
/// claude-code `processMemoryFile`).
pub struct RealMemoryHierarchyProvider;

#[async_trait]
impl MemoryHierarchyProvider for RealMemoryHierarchyProvider {
    async fn load(&self, cwd: &Path) -> Vec<MemoryFile> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let h = memory::claude_md::hierarchy::walk(cwd, &home);
        // walk() returns innermost-first; reverse to home → outer → cwd.
        // Within the same dir, M3-02 emits `CLAUDE.local.md` BEFORE
        // `CLAUDE.md` (so local-override shadows canonical). After
        // reverse() that flips: canonical comes first at each level,
        // local-override LAST — matching spec §4.3 splice order.
        let mut entries = h.entries;
        entries.reverse();

        // `@import` expansion (claude-code processMemoryFile): a single
        // `processed` set is shared across the whole hierarchy load so an
        // imported file is spliced at most once, and each top-level file is
        // expanded at depth 0. Each `@import`'d file becomes its own
        // `MemoryFile` entry, parent before children.
        let user_dir = home.join(".claude");
        let mut processed: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in entries {
            // User memory (`~/.claude/...`) may include external files;
            // Project/Local default to local-only. The TS
            // `hasClaudeMdExternalIncludesApproved` opt-in is not plumbed
            // into this seam, so Project/Local use the `false` default.
            let include_external = e.path.starts_with(&user_dir);
            let expanded = memory::claude_md::loader::expand_memory_file(
                &e.path,
                &mut processed,
                include_external,
                cwd,
                Some(&home),
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
                    // Only the hierarchy entry itself can be a
                    // `CLAUDE.local.md`; `@import`'d children are plain files.
                    is_local_override: idx == 0 && e.is_local_override,
                });
            }
        }
        out
    }
}

/// Convenience constructor: returns an `Arc<dyn MemoryHierarchyProvider>`
/// wrapping a fresh [`RealMemoryHierarchyProvider`]. Used by the
/// production constructor of `ConversationOrchestrator`.
#[must_use]
pub fn real_provider() -> Arc<dyn MemoryHierarchyProvider> {
    Arc::new(RealMemoryHierarchyProvider)
}

/// Format the `<memory>...</memory>` block from a slice of loaded
/// files. When `files` is empty, returns the EMPTY STRING — caller
/// MUST elide the section (no `<memory></memory>` empty tags emitted).
///
/// Per-entry shape:
/// ```text
/// # {path}
///
/// {body}
///
/// ```
/// Tag wrapping:
/// ```text
/// <memory>
/// # {p1}
///
/// {body1}
///
/// # {p2}
///
/// {body2}
///
/// </memory>
/// ```
#[must_use]
pub fn format(files: &[MemoryFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(1024);
    s.push_str("<memory>\n");
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&format!("# {}\n\n{}\n", f.path.display(), f.body));
    }
    s.push_str("</memory>\n");
    s
}
