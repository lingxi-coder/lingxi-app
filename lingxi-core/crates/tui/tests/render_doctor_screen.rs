//! Snapshot test for `DoctorScreen` at a fixed diagnostic state.
//! Locks the section headers (`Diagnostics`, `Updates`) + `└ ` row layout.

use iocraft::prelude::*;
use lingxi_tui::screens::doctor::{DoctorDiagnostics, DoctorScreen};

fn fixed_diag() -> DoctorDiagnostics {
    DoctorDiagnostics {
        cli_version: "lingxi-cli v0.8.0".into(),
        rust_toolchain: "1.82.0".into(),
        claude_home: "/home/u/.claude".into(),
        cwd: "/work/proj".into(),
        mcp_configured: 2,
        mcp_connected: 0,
        auth_state: "unknown".into(),
        truecolor: true,
        term_size: (120, 40),
    }
}

#[test]
fn doctor_screen_fixed_state() {
    let mut element = element! {
        DoctorScreen(diag: fixed_diag())
    };
    let rendered = element.to_string();
    insta::assert_snapshot!("doctor_screen_fixed", rendered);
}
