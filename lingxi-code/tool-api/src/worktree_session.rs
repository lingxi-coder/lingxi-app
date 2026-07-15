//! `WorktreeSession` — the substrate `EnterWorktree` writes and `ExitWorktree`
//! reads/clears (worktree 206 parity plan, Task 8).
//!
//! `EnterWorktreeTool` swaps the shared [`crate::session_cwd::SessionCwd`] cell
//! (Task 2/7) so every FS/Bash tool observes the worktree, but that cell alone
//! carries no memory of WHERE the session came from, or which worktree/branch
//! it just entered — `ExitWorktree`'s 206 `{action, discard_changes}` contract
//! needs both to restore the original cwd and to know what to keep/remove.
//! This module is that missing record: a single, shared, mutable
//! `Option<WorktreeSession>` cell, reachable by both tools through
//! [`crate::builtin_context::BuiltinToolContext::worktree_session`] (mirroring
//! how `session_cwd: Arc<SessionCwd>` is shared).
//!
//! INERT INVARIANT: a fresh cell starts `None`. Until `EnterWorktree` writes a
//! `Some(..)`, the cell stays `None` and `ExitWorktree` takes its no-op path —
//! byte-identical to a session that never touches worktrees at all.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A record of the single active worktree the session entered via
/// `EnterWorktree`, captured at the moment of a successful create/enter —
/// BEFORE the `session_cwd.swap(..)` that actually moves the session into it.
///
/// `ExitWorktree` reads this to restore [`Self::original_cwd`] and to
/// reconstruct the [`traits::worktree::WorktreeHandle`] it needs for
/// `worktree_change_summary`/`remove_worktree`, then clears the cell back to
/// `None` on completion.
#[derive(Debug, Clone)]
pub struct WorktreeSession {
    /// The session's cwd immediately before the `EnterWorktree` call that
    /// created this record — i.e. what `ExitWorktree` restores via
    /// `session_cwd.swap(original_cwd, vec![original_cwd])`. When
    /// `EnterWorktree` is called again while ALREADY inside a worktree (the
    /// `path` mid-session-switch case), this is the PREVIOUS worktree's path,
    /// not the true launch-time cwd — mirrors the 206 prompt's documented
    /// behavior ("the previous worktree is left on disk, untouched, and only
    /// the new one is tracked for exit-time cleanup").
    pub original_cwd: PathBuf,
    /// Absolute path to the worktree the session is currently in.
    pub worktree_path: PathBuf,
    /// Git branch checked out inside the worktree (may be the literal
    /// `"HEAD"` for a detached-HEAD worktree — treated as "no branch" by both
    /// tools' display logic, matching `EnterWorktreeTool`'s `branch_suffix`).
    pub branch_name: String,
    /// The baseline commit `WorktreeHandle::base_commit` captured at
    /// creation/entry, threaded through so `ExitWorktree`'s dirty-state check
    /// can count ahead-commits. `None` when unavailable — the ahead-commit
    /// count then falls to "unknown", matching
    /// `WorktreeManager::worktree_change_summary`'s fail-closed contract.
    pub base_commit: Option<String>,
    /// `true` when `EnterWorktree` ENTERED a pre-existing worktree (`path`
    /// branch), `false` when it CREATED one. 206's `ExitWorktree` refuses a
    /// `remove` on an entered (not owned) worktree (`t.enteredExisting`,
    /// errorCode 4) — this session is not its owner.
    pub entered_existing: bool,
    /// Name of a tmux session attached to this worktree, if any. The port has
    /// no worktree-attached tmux wiring today — `EnterWorktreeTool` always
    /// writes `None` here — so this field is a substrate placeholder for a
    /// future integration; `ExitWorktreeTool`'s tmux-handling branch is gated
    /// on `Some` and is therefore presently unreachable in production. See
    /// the module doc on `tools::worktree::ExitWorktreeTool` for the residual
    /// note.
    pub tmux_session_name: Option<String>,
}

/// Shared, mutable cell holding at most one active [`WorktreeSession`].
/// `None` when no worktree has been entered (or after `ExitWorktree` clears
/// it) — mirrors [`crate::session_cwd::SessionCwd`]'s no-swap-until-called
/// inertness. Held as a plain `Mutex` (not `ArcSwap`): writes are rare
/// (one per `EnterWorktree`/`ExitWorktree` call) and readers need the whole
/// struct, so the extra lock-free-read machinery `SessionCwd` uses for
/// hot-path per-tool-call cwd reads isn't needed here.
pub type WorktreeSessionCell = Arc<Mutex<Option<WorktreeSession>>>;

/// Construct a fresh, empty (`None`) [`WorktreeSessionCell`] — the default
/// every `BuiltinToolContext` construction site wires until an
/// `EnterWorktree` call populates it.
#[must_use]
pub fn new_worktree_session_cell() -> WorktreeSessionCell {
    Arc::new(Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_cell_is_none() {
        let cell = new_worktree_session_cell();
        assert!(cell.lock().unwrap().is_none());
    }

    #[test]
    fn cell_can_be_populated_and_cleared() {
        let cell = new_worktree_session_cell();
        *cell.lock().unwrap() = Some(WorktreeSession {
            original_cwd: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.lingxi/worktrees/feat"),
            branch_name: "worktree-feat".to_string(),
            base_commit: Some("deadbeef".to_string()),
            entered_existing: false,
            tmux_session_name: None,
        });
        assert!(cell.lock().unwrap().is_some());
        *cell.lock().unwrap() = None;
        assert!(cell.lock().unwrap().is_none());
    }

    #[test]
    fn shared_arc_observes_writes_from_either_clone() {
        // Mirrors the SessionCwd sharing pattern: two tool instances holding
        // clones of the SAME cell must observe each other's writes.
        let cell = new_worktree_session_cell();
        let enter_side = Arc::clone(&cell);
        let exit_side = Arc::clone(&cell);

        *enter_side.lock().unwrap() = Some(WorktreeSession {
            original_cwd: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.lingxi/worktrees/feat"),
            branch_name: "worktree-feat".to_string(),
            base_commit: None,
            entered_existing: false,
            tmux_session_name: None,
        });

        let seen = exit_side.lock().unwrap().clone();
        assert_eq!(
            seen.unwrap().worktree_path,
            PathBuf::from("/repo/.lingxi/worktrees/feat")
        );
    }
}
