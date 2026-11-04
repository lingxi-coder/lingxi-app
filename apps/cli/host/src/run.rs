//! Product-owned interactive resume selectors and TUI remount lifecycle.
mod resume;

#[cfg(test)]
pub(crate) use resume::create_independent_cli_branch;
pub(crate) use resume::create_independent_cli_branch_from_source;
pub(crate) use resume::{
    daemon_runtime_dir, drive_background_tui_switch_loop, drive_tui_switch_loop, lingxi_home_dir,
    load_resume_catalog_for_cwd, load_resume_rows, map_resume_rows, mount_background_resumed_tui,
    parent_session_id_from_messages, resolve_resume_title_source,
};
pub use resume::{resume_route, run_continue, run_from_pr, run_resume, ResumeRoute};

#[cfg(test)]
use crate::argv::Argv;
#[cfg(test)]
use crate::exit_codes;
#[cfg(test)]
use lingxi_core::host::OrchestratorHandle;
#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
