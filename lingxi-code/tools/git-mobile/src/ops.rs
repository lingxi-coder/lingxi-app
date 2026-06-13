//! Git operations — operation enum → `git2` calls (filled by Tasks 4-6/8).
//!
//! This module hosts the host-testable, deterministic `git2`-backed
//! implementations of each supported operation:
//!
//! - Task 4: workspace-anchored `open_repo` + path-escape validation +
//!   `GitOpError`.
//! - Task 5: read operations (status / diff / log / show / `branch_list`).
//! - Task 6: local write operations (add / commit / `branch_create` / checkout
//!   / merge fast-forward).
//! - Task 8: network operations (clone / fetch / pull) with the in-process
//!   credential callback + CA wiring.
//!
//! For Task 3 (the tool shell) this module is intentionally empty: the
//! `GitTool::call` dispatch returns a "not yet implemented" `InvalidInput`
//! error for every operation until the `ops::` functions land.
