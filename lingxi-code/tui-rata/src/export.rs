//! `/export` — write the transcript to a plain-text file (plan Phase 8).
//!
//! Filename handling is ported from the iocraft backend's
//! `components/message_selector.rs` export flow: the same default
//! `lingxi-transcript-<unix_secs>.txt` name, the same basename-clamp +
//! forced-`.txt` path-traversal guard, and the same no-silent-clobber
//! posture. The iocraft backend prompted `y/n` to overwrite through its
//! export dialog; this backend has no editable-filename dialog — `/export
//! [filename]` takes the name as the argument instead, and an existing
//! target is a plain error (re-run with another name; the timestamped
//! default never collides in practice).
//!
//! The transcript body itself is composed by the caller
//! ([`crate::chat_widget::ChatWidget::cmd_export`]) from the history cells'
//! copy-friendly [`crate::history_cell::HistoryCell::raw_lines`] — the same
//! text raw scrollback mode shows — so the export stays byte-consistent with
//! the on-screen rendering rather than forking a second formatter.

use std::path::{Path, PathBuf};

/// Default export directory, `~/.lingxi/exports` (iocraft
/// `default_export_dir` parity), falling back to the cwd without a home dir.
/// Home comes from `$HOME`/`%USERPROFILE%` (dependency-free; `tui-rata` does
/// not pull the `dirs` crate).
#[must_use]
pub fn default_export_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match home {
        Some(home) => home.join(".lingxi").join("exports"),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Default export filename, `lingxi-transcript-<unix_secs>.txt`. The
/// timestamp keeps successive exports from colliding.
#[must_use]
pub fn default_export_filename() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("lingxi-transcript-{secs}.txt")
}

/// Resolve a user-typed export filename to a safe `<basename>.txt`,
/// guaranteeing the export lands INSIDE the export dir (path-traversal
/// guard, iocraft parity). The input is clamped to its final path component
/// via [`Path::file_name`] — which drops directory parts and parent refs
/// (`"/etc/passwd"` → `passwd`, `"../../foo"` → `foo`) — so a later
/// `dir.join(...)` can never escape `dir`. Inputs with no usable basename
/// (`""`, `".."`, a trailing `/`) fall back to [`default_export_filename`].
/// The resulting stem then gets the forced `.txt` extension.
#[must_use]
pub fn resolve_export_filename(input: &str) -> String {
    let basename = Path::new(input.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(default_export_filename, str::to_string);
    let stem = basename.rsplit_once('.').map_or(&*basename, |(s, _)| s);
    format!("{stem}.txt")
}

/// Why an export failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// The target file already exists (never silently clobbered).
    Exists(PathBuf),
    /// A filesystem failure (dir creation or write).
    Io(String),
}

/// Write `body` to `<dir>/<filename>` (basename-clamped, forced `.txt`).
/// Creates `dir` if absent; refuses to overwrite an existing target.
///
/// # Errors
/// [`ExportError::Exists`] when the target exists; [`ExportError::Io`] on
/// any filesystem failure.
pub fn write_export(dir: &Path, filename: &str, body: &str) -> Result<PathBuf, ExportError> {
    let target = dir.join(resolve_export_filename(filename));
    if target.exists() {
        return Err(ExportError::Exists(target));
    }
    std::fs::create_dir_all(dir).map_err(|e| ExportError::Io(e.to_string()))?;
    std::fs::write(&target, body.as_bytes()).map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filename_is_timestamped_txt() {
        let name = default_export_filename();
        assert!(name.starts_with("lingxi-transcript-"), "{name}");
        assert!(
            Path::new(&name).extension().is_some_and(|e| e == "txt"),
            "{name}"
        );
    }

    #[test]
    fn resolve_clamps_traversal_and_forces_txt() {
        assert_eq!(resolve_export_filename("notes"), "notes.txt");
        assert_eq!(resolve_export_filename("notes.md"), "notes.txt");
        // Directory parts and parent refs are dropped (traversal guard).
        assert_eq!(resolve_export_filename("/etc/passwd"), "passwd.txt");
        assert_eq!(resolve_export_filename("../../evil"), "evil.txt");
        assert_eq!(resolve_export_filename("a/b/c"), "c.txt");
        // No usable basename → the timestamped default.
        assert!(resolve_export_filename("").starts_with("lingxi-transcript-"));
        assert!(resolve_export_filename("..").starts_with("lingxi-transcript-"));
    }

    #[test]
    fn write_export_creates_refuses_overwrite_and_stays_inside_dir() {
        let dir = std::env::temp_dir().join(format!("tui-rata-export-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let path = write_export(&dir, "conv", "> hi\nreply\n").expect("first write");
        assert_eq!(path, dir.join("conv.txt"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "> hi\nreply\n");

        // Second write to the same name refuses to clobber.
        assert_eq!(
            write_export(&dir, "conv", "other"),
            Err(ExportError::Exists(dir.join("conv.txt")))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "> hi\nreply\n");

        // A traversal-shaped name still lands inside `dir`.
        let clamped = write_export(&dir, "../outside", "x").expect("clamped write");
        assert_eq!(clamped, dir.join("outside.txt"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
