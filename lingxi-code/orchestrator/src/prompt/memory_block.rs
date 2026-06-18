//! Memory-section formatter (claude-code `getClaudeMds`) +
//! `MemoryHierarchyProvider` trait.
//!
//! The trait abstracts the M3-02 `claude_md::walk` + `load_file` pair
//! so the orchestrator can take a `Arc<dyn MemoryHierarchyProvider>`
//! field and tests can substitute a static fixture without touching
//! the filesystem. Production impl: [`RealMemoryHierarchyProvider`].
//! The formatter ([`format`]) emits the preamble + per-file
//! `Contents of …:` blocks (GAP 3 — no enclosing tag, no trailing newline).
#![forbid(unsafe_code)]

use crate::prompt::MemoryFile;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// Loads the CLAUDE.md hierarchy for a given cwd.
///
/// Implementations MUST return the files in claude-code splice order:
/// the Managed tier first (`<managed>/CLAUDE.md` + rules), then User
/// (`~/.claude/CLAUDE.md` + rules), then Project (`<repo>/CLAUDE.md`, …),
/// then Local (`<repo>/CLAUDE.local.md`) — innermost last so it wins the
/// model's recency attention.
///
/// §F: the returned vec contains BOTH unconditional files (`globs == None`)
/// AND conditional rules (`globs == Some(_)`, carrying `paths:` globs). The
/// caller decides what to do with each: [`format`] filters to `globs.is_none()`
/// for the eager system-prompt block, while the orchestrator routes the
/// conditional rules to per-edited-file lazy activation (claudemd.ts
/// `processConditionedMdRules`).
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
/// claude-code splice order (managed → home → repo → local-override),
/// recursively splices each file's `@import` references directly after it
/// (parity with claude-code `processMemoryFile`), and tags each file with its
/// [`memory::claude_md::ClaudeMdTier`]. Conditional (`paths:`-gated) rules are
/// returned WITH their globs intact (§F) — the eager-vs-lazy split is the
/// caller's responsibility (see [`format`] / the orchestrator's
/// `conditional_rules_reminder_message`).
pub struct RealMemoryHierarchyProvider;

#[async_trait]
impl MemoryHierarchyProvider for RealMemoryHierarchyProvider {
    async fn load(&self, cwd: &Path) -> Vec<MemoryFile> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        // Managed tier (`<managed>/CLAUDE.md` + `<managed>/.claude/rules/**`)
        // is always probed (never settings-gated). `managed_path()` consults
        // the `LINGXI_MANAGED_DIR` override and falls back to the platform
        // default.
        let managed = memory::claude_md::hierarchy::managed_path();
        let h = memory::claude_md::hierarchy::walk(cwd, &home, Some(&managed));
        // walk() returns innermost-first; reverse to managed → home → outer →
        // cwd. Within the same dir, the walk emits `CLAUDE.local.md` BEFORE
        // `CLAUDE.md` (so local-override shadows canonical). After reverse()
        // that flips: canonical comes first at each level, local-override LAST
        // — matching claude-code splice order.
        let mut entries = h.entries;
        entries.reverse();

        // `@import` expansion (claude-code processMemoryFile): a single
        // `processed` set is shared across the whole hierarchy load so an
        // imported file is spliced at most once, and each top-level file is
        // expanded at depth 0. Each `@import`'d file becomes its own
        // `MemoryFile` entry, parent before children.
        let mut processed: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in entries {
            // External-include policy (claudemd.ts:826-846): ONLY the User tier
            // gets unconditional external includes. Managed and Project/Local
            // default to local-only (the `hasClaudeMdExternalIncludesApproved`
            // opt-in is not plumbed into this seam, so they use `false`).
            let include_external = e.tier == memory::claude_md::ClaudeMdTier::User;
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
                // §F Gap-2 part-2: conditional (`paths:`-gated) rules are NO
                // LONGER dropped here. They flow through with their `globs`
                // intact so the orchestrator can lazily activate them when an
                // edited/opened file matches (claudemd.ts `processConditionedMdRules`).
                // The *eager* exclusion now lives in [`format`], which filters to
                // `globs.is_none()` so the system-prompt block stays byte-identical
                // (claudemd.ts:773 `conditionalRule:false`).
                out.push(MemoryFile {
                    path: entry.path,
                    body,
                    // Only the hierarchy entry itself can be a
                    // `CLAUDE.local.md`; `@import`'d children are plain files.
                    is_local_override: idx == 0 && e.is_local_override,
                    // `@import`'d children inherit the parent's tier (TS passes
                    // `type` down through processMemoryFile recursion).
                    tier: e.tier,
                    // Carry the `paths:` globs through: `None` = unconditional
                    // (eager); `Some(_)` = conditional (lazy activation only).
                    globs: entry.globs,
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

/// Build a memdir-backed memory prefetcher for the composition root — the P0.1
/// activation of the `relevant_memories` surfacing channel.
///
/// Wires the LLM memory selector (`side_query_client`, Haiku-class) over the
/// user memdir (`<home>/.claude/memdir`) so that, each turn,
/// [`MemoryPrefetch::start`](memory::prefetch::MemoryPrefetch::start) scans the
/// memdir, asks the selector which entries are relevant to the turn query, and
/// surfaces them through
/// [`ConversationOrchestrator::relevant_memory_reminder_message`](crate::ConversationOrchestrator).
/// Hand the returned handle to
/// [`ConversationOrchestrator::with_memory_prefetch`](crate::ConversationOrchestrator).
///
/// Centralised here (not inlined at each composition root) so desktop / bridge /
/// mobile build the prefetch identically and the engine apps need no direct
/// dependency on the `memory` crate's internals.
///
/// GATING: the composition root decides whether to call this — claude-code keeps
/// the feature behind `tengu_moth_copse` (default OFF); the LingXi equivalent is
/// "is a prefetch wired at all". `team_memory.enabled` is left `false` here (the
/// user memdir only), matching the inert default until team memory is configured.
#[must_use]
pub fn build_memdir_prefetch(
    side_query_client: Arc<dyn sidequery::SideQueryClient>,
    runtime: Arc<dyn traits::RuntimeSpawner>,
    home: &std::path::Path,
) -> Arc<memory::prefetch::MemoryPrefetch> {
    let roots = memory::memdir::memdir_path(home, false);
    let selector = Arc::new(memory::selector::MemorySelector::new(side_query_client));
    Arc::new(memory::prefetch::MemoryPrefetch::new(selector, runtime, roots))
}

/// Verbatim preamble that precedes the memory blocks.
///
/// 1:1 with claude-code `MEMORY_INSTRUCTION_PROMPT` (claudemd.ts:89-90).
const MEMORY_INSTRUCTION_PROMPT: &str = "Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.";

/// The per-file injection description for a given tier (claudemd.ts:1168-1186).
/// Includes the leading space, exactly as TS concatenates `${file.path}${description}`.
fn tier_description(tier: memory::claude_md::ClaudeMdTier) -> &'static str {
    use memory::claude_md::ClaudeMdTier;
    match tier {
        ClaudeMdTier::Project => " (project instructions, checked into the codebase)",
        ClaudeMdTier::Local => " (user's private project instructions, not checked in)",
        // Managed and User both use the global-instructions wording.
        ClaudeMdTier::User | ClaudeMdTier::Managed => {
            " (user's private global instructions for all projects)"
        }
    }
}

/// Format the memory section from a slice of loaded files, 1:1 with claude-code
/// `getClaudeMds` (claudemd.ts:1153-1195).
///
/// Shape (NO enclosing tag, NO trailing newline):
/// ```text
/// {MEMORY_INSTRUCTION_PROMPT}
///
/// Contents of {p1}{desc1}:
///
/// {body1}
///
/// Contents of {p2}{desc2}:
///
/// {body2}
/// ```
/// where `{descN}` is the tier description (project / local / global) and each
/// body is `.trim()`med. Blocks are joined by `"\n\n"`. When `files` is empty,
/// returns the EMPTY STRING and the caller MUST elide the section.
///
/// §F: CONDITIONAL rules (`globs.is_some()`) are filtered OUT here — only
/// unconditional files (`CLAUDE.md` + non-`paths:` rules) are eagerly injected,
/// mirroring claude-code's `conditionalRule:false` eager filter (claudemd.ts:773).
/// Conditional rules are activated lazily per edited/opened file by the
/// orchestrator. If `files` contains ONLY conditional rules, this returns the
/// empty string (caller elides the section).
#[must_use]
pub fn format(files: &[MemoryFile]) -> String {
    let blocks: Vec<String> = files
        .iter()
        // §F: eager block = unconditional files only.
        .filter(|f| f.globs.is_none())
        .map(|f| {
            format!(
                "Contents of {}{}:\n\n{}",
                f.path.display(),
                tier_description(f.tier),
                f.body.trim()
            )
        })
        .collect();
    if blocks.is_empty() {
        return String::new();
    }
    format!("{MEMORY_INSTRUCTION_PROMPT}\n\n{}", blocks.join("\n\n"))
}
