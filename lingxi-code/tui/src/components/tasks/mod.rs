//! Background-task rendering (M9-04): per-type row renderers, shell progress,
//! status text, duration/exit formatters, and the live output tail. Pure
//! string renderers — the iocraft components + dialog that display them are
//! M9-05.

pub mod format;
pub mod shell_progress;
pub mod status_text;
