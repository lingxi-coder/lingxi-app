//! `/add-dir <path>` path validation — a faithful Rust port of claude-code's
//! `commands/add-dir/validation.ts` (`validateDirectoryForWorkspace` +
//! `addDirHelpMessage`).
//!
//! The command adds a working directory to the session's
//! `permissions.additionalDirectories`. This module ONLY resolves + validates
//! the user-supplied path (expand a leading `~`, make it absolute against the
//! process cwd, normalize away `.`/`..`/trailing-slash, then stat it). The
//! durable settings-file write is driven off-loop through
//! [`crate::bottom_pane::PermissionAction::AddDirectory`] (see
//! `apps/cli/src/mode.rs::run_permission_action`), reusing
//! `permission::persist_workspace_directory` — mirroring how the reference
//! keeps validation pure and defers the persist to `persistPermissionUpdate`.

use std::path::{Component, Path, PathBuf};

/// Outcome of validating a `/add-dir` argument, mirroring claude-code's
/// `AddDirectoryResult` union (minus the `alreadyInWorkingDirectory` arm, which
/// is folded into the idempotent persist step — see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddDirValidation {
    /// No path was supplied.
    EmptyPath,
    /// The resolved path does not exist or is inaccessible (claude-code maps
    /// ENOENT/ENOTDIR/EACCES/EPERM all to "not found").
    PathNotFound { absolute: String },
    /// The resolved path exists but is a file, not a directory.
    NotADirectory { input: String, parent: String },
    /// The path resolved to an existing directory at `absolute`.
    Success { absolute: String },
}

/// Expand a leading `~` / `~/…` against the user's home directory. Any other
/// use of `~` is left verbatim (claude-code `expandPath`).
fn expand_tilde(input: &str) -> PathBuf {
    if input == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(input)
}

/// Lexically normalize `path` (resolve `.`/`..` components and drop the
/// trailing slash) WITHOUT touching the filesystem — mirrors node
/// `path.resolve`'s normalization. Symlinks are intentionally NOT resolved,
/// matching the reference (`resolve`, not `realpath`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `input` to an absolute, normalized path and validate that it exists
/// and is a directory. Relative inputs resolve against the process cwd (the
/// session working directory), matching claude-code `resolve(expandPath(...))`.
#[must_use]
pub fn resolve_and_validate(input: &str) -> AddDirValidation {
    if input.is_empty() {
        return AddDirValidation::EmptyPath;
    }
    let expanded = expand_tilde(input);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    };
    let absolute = normalize(&absolute);
    let absolute_str = absolute.to_string_lossy().to_string();
    match std::fs::metadata(&absolute) {
        Ok(meta) if meta.is_dir() => AddDirValidation::Success {
            absolute: absolute_str,
        },
        Ok(_) => {
            let parent = absolute
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            AddDirValidation::NotADirectory {
                input: input.to_string(),
                parent,
            }
        }
        // Match claude-code: any stat error (ENOENT/ENOTDIR/EACCES/EPERM) reads
        // as "not found" rather than crashing the command.
        Err(_) => AddDirValidation::PathNotFound {
            absolute: absolute_str,
        },
    }
}

/// The user-facing message for a NON-success validation result (claude-code
/// `addDirHelpMessage`, minus the terminal-bold markup).
#[must_use]
pub fn help_message(result: &AddDirValidation) -> String {
    match result {
        AddDirValidation::EmptyPath => "Please provide a directory path.".to_string(),
        AddDirValidation::PathNotFound { absolute } => format!("Path {absolute} was not found."),
        AddDirValidation::NotADirectory { input, parent } => format!(
            "{input} is not a directory. Did you mean to add the parent directory {parent}?"
        ),
        AddDirValidation::Success { absolute } => {
            format!("Added {absolute} as a working directory.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_empty_path() {
        assert_eq!(resolve_and_validate(""), AddDirValidation::EmptyPath);
    }

    #[test]
    fn existing_directory_succeeds_with_absolute_path() {
        // `std::env::temp_dir()` is always an existing directory.
        let dir = std::env::temp_dir();
        let input = dir.to_string_lossy().to_string();
        match resolve_and_validate(&input) {
            AddDirValidation::Success { absolute } => {
                assert!(Path::new(&absolute).is_absolute());
                assert!(Path::new(&absolute).is_dir());
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[test]
    fn trailing_slash_is_normalized_away() {
        let dir = std::env::temp_dir();
        let with_slash = format!("{}/", dir.to_string_lossy().trim_end_matches('/'));
        let no_slash = resolve_and_validate(dir.to_string_lossy().trim_end_matches('/'));
        assert_eq!(resolve_and_validate(&with_slash), no_slash);
    }

    #[test]
    fn missing_path_is_not_found() {
        let missing =
            std::env::temp_dir().join(format!("lingxi-add-dir-missing-{}", std::process::id()));
        let input = missing.to_string_lossy().to_string();
        assert!(matches!(
            resolve_and_validate(&input),
            AddDirValidation::PathNotFound { .. }
        ));
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let file = std::env::temp_dir().join(format!("lingxi-add-dir-file-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let input = file.to_string_lossy().to_string();
        let result = resolve_and_validate(&input);
        std::fs::remove_file(&file).ok();
        assert!(matches!(result, AddDirValidation::NotADirectory { .. }));
    }

    #[test]
    fn help_messages_match_reference_wording() {
        assert_eq!(
            help_message(&AddDirValidation::EmptyPath),
            "Please provide a directory path."
        );
        assert_eq!(
            help_message(&AddDirValidation::PathNotFound {
                absolute: "/x".into()
            }),
            "Path /x was not found."
        );
        assert_eq!(
            help_message(&AddDirValidation::NotADirectory {
                input: "a/b".into(),
                parent: "/root/a".into(),
            }),
            "a/b is not a directory. Did you mean to add the parent directory /root/a?"
        );
    }
}
