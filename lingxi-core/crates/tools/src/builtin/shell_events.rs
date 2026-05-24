//! Re-export façade for shell-tool telemetry event names.
//!
//! The canonical declarations live in `lingxi_telemetry::tengu::tool`.
//! This façade exists so `builtin/{powershell,repl,sleep}.rs` can
//! `use crate::builtin::shell_events::POWERSHELL_STARTED;` without
//! reaching across crates at every callsite. The string values are
//! locked by `tengu::tool` — DO NOT redeclare here.

pub use lingxi_telemetry::tengu::tool::{
    POWERSHELL_COMPLETED, POWERSHELL_FAILED, POWERSHELL_STARTED, REPL_COMPLETED, REPL_FAILED,
    REPL_STARTED, SLEEP_COMPLETED, SLEEP_FAILED, SLEEP_STARTED,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_match_locked_strings() {
        assert_eq!(POWERSHELL_STARTED, "tengu_tool_powershell_started");
        assert_eq!(POWERSHELL_COMPLETED, "tengu_tool_powershell_completed");
        assert_eq!(POWERSHELL_FAILED, "tengu_tool_powershell_failed");
        assert_eq!(REPL_STARTED, "tengu_tool_repl_started");
        assert_eq!(REPL_COMPLETED, "tengu_tool_repl_completed");
        assert_eq!(REPL_FAILED, "tengu_tool_repl_failed");
        assert_eq!(SLEEP_STARTED, "tengu_tool_sleep_started");
        assert_eq!(SLEEP_COMPLETED, "tengu_tool_sleep_completed");
        assert_eq!(SLEEP_FAILED, "tengu_tool_sleep_failed");
    }
}
