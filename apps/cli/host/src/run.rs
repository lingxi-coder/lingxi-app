//! Product-owned interactive resume selectors and TUI remount lifecycle.
mod resume;

pub(crate) use resume::{
    daemon_runtime_dir, drive_background_tui_switch_loop, drive_tui_switch_loop,
    inherited_resume_effort, lingxi_home_dir, load_resume_picker_rows,
    mount_background_resumed_tui, resolve_resume_title,
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
