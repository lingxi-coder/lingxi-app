//! TUI adapter over the shared workspace directory validation.

pub use command_api::workspace_directory::{help_message, AddDirValidation};
use std::path::Path;

/// Resolve against the live cwd, using the user home for a leading tilde.
#[must_use]
pub fn resolve_and_validate(input: &str, cwd: &Path) -> AddDirValidation {
    command_api::workspace_directory::resolve_and_validate(input, cwd, dirs::home_dir().as_deref())
}

/// `/cd` permits an existing network directory.
#[must_use]
pub fn resolve_existing_directory(input: &str, cwd: &Path) -> AddDirValidation {
    command_api::workspace_directory::resolve_existing_directory(
        input,
        cwd,
        dirs::home_dir().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(input: &str) -> AddDirValidation {
        resolve_and_validate(input, &std::env::temp_dir())
    }

    fn existing(input: &str) -> AddDirValidation {
        resolve_existing_directory(input, &std::env::temp_dir())
    }

    #[test]
    fn relative_directory_uses_supplied_live_cwd() {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("live");
        let child = live.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let expected = AddDirValidation::Success {
            absolute: child.display().to_string(),
        };
        assert_eq!(resolve_and_validate("./child/../child", &live), expected);
        assert_eq!(resolve_existing_directory("child", &live), expected);
    }

    #[test]
    fn network_directory_check_uses_supplied_live_host() {
        let network_cwd = Path::new("/net/lingxi-test-host/workspace");
        // The SDK permits another directory on the session's existing host.
        assert!(matches!(
            resolve_and_validate("./missing-child", network_cwd),
            AddDirValidation::PathNotFound { .. }
        ));
        assert_eq!(
            resolve_and_validate("../../other-host/missing-child", network_cwd),
            AddDirValidation::NetworkPath {
                directory_path: "../../other-host/missing-child".into()
            }
        );
        assert!(matches!(
            resolve_existing_directory("./missing-child", network_cwd),
            AddDirValidation::PathNotFound { .. }
        ));
    }

    #[test]
    fn empty_input_is_empty_path() {
        assert_eq!(validate(""), AddDirValidation::EmptyPath);
    }

    #[test]
    fn existing_directory_succeeds_with_absolute_path() {
        // `std::env::temp_dir()` is always an existing directory.
        let dir = std::env::temp_dir();
        let input = dir.to_string_lossy().to_string();
        match validate(&input) {
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
        let no_slash = validate(dir.to_string_lossy().trim_end_matches('/'));
        assert_eq!(validate(&with_slash), no_slash);
    }

    #[test]
    fn missing_path_is_not_found() {
        let missing =
            std::env::temp_dir().join(format!("lingxi-add-dir-missing-{}", std::process::id()));
        let input = missing.to_string_lossy().to_string();
        assert!(matches!(
            validate(&input),
            AddDirValidation::PathNotFound { .. }
        ));
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let file = std::env::temp_dir().join(format!("lingxi-add-dir-file-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let input = file.to_string_lossy().to_string();
        let result = validate(&input);
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
        assert_eq!(
            help_message(&AddDirValidation::NetworkPath {
                directory_path: "//fileserver/share".into(),
            }),
            "//fileserver/share is a network path, which cannot be added as a working directory. On Windows, map the share to a drive letter and pass it at launch with --add-dir (a drive letter added mid-session does not yet carry remote-read trust)."
        );
    }

    #[test]
    fn unc_and_net_automount_are_refused_before_stat() {
        assert!(matches!(
            validate("//fileserver/share"),
            AddDirValidation::NetworkPath { .. }
        ));
        assert!(matches!(
            validate("/net/somehost/data"),
            AddDirValidation::NetworkPath { .. }
        ));
        let missing =
            std::env::temp_dir().join(format!("lingxi-cd-missing-{}", std::process::id()));
        let input = missing.to_string_lossy().to_string();
        assert!(
            matches!(existing(&input), AddDirValidation::PathNotFound { .. }),
            "/cd still stats local paths rather than using the network gate"
        );
    }
}
