//! M7-12: --resume routing. Empty arg + TTY → iocraft screen; empty arg +
//! no-tui/non-TTY → M5-08 stdio picker; concrete id → load-by-id.

use lingxi_cli::argv::Argv;
use lingxi_cli::run::{resume_route, ResumeRoute};

fn argv_resume(arg: &str, no_tui: bool) -> Argv {
    let mut a = Argv::from_iter(["lingxi-cli", "--resume", arg]).unwrap();
    a.no_tui = no_tui;
    a
}

#[test]
fn empty_arg_tty_routes_to_iocraft_screen() {
    let a = argv_resume("", false);
    assert_eq!(
        resume_route(&a, /* is_tty */ true),
        ResumeRoute::IocraftScreen
    );
}

#[test]
fn empty_arg_no_tui_routes_to_stdio_picker() {
    let a = argv_resume("", true);
    assert_eq!(resume_route(&a, true), ResumeRoute::StdioPicker);
}

#[test]
fn empty_arg_non_tty_routes_to_stdio_picker() {
    let a = argv_resume("", false);
    assert_eq!(
        resume_route(&a, /* is_tty */ false),
        ResumeRoute::StdioPicker
    );
}

#[test]
fn concrete_id_routes_to_load_by_id() {
    let id = "11111111-1111-1111-1111-111111111111";
    let a = argv_resume(id, false);
    assert_eq!(resume_route(&a, true), ResumeRoute::LoadById);
}
