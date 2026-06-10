//! Linux bwrap sandbox assembly — a 1:1 behavioral port of
//! `linux-sandbox-utils.js` (the dependency check, the `socat` network-namespace
//! bridge lifecycle, `buildSandboxCommand`, the full `wrapCommandWithSandboxLinux`
//! bwrap argv assembly, and mount-point cleanup).
//!
//! # Design divergence from the TS reference (documented, faithful)
//!
//! The TS module keeps **module-global mutable state** — `activeSandboxCount`
//! (a refcount) and a `bwrapMountPoints` `Set` populated as a side effect of
//! `generateFilesystemArgs`. `cleanupBwrapMountPoints()` reads both globals.
//! This port has **no global state**: [`crate::fs_args::generate_filesystem_args`]
//! *returns* the `Vec<PathBuf>` of mount points, [`wrap_command_with_sandbox_linux`]
//! threads it back to the caller, and [`cleanup_bwrap_mount_points`] takes that
//! slice as an explicit argument. Each wrap invocation therefore owns exactly the
//! mount points it created — the refcount/defer dance disappears because there is
//! no shared set to protect. The observable file-cleanup behavior (unlink empty
//! files, rmdir empty dirs, ignore errors) is identical.
//!
//! # Seccomp seam (P7)
//!
//! [`resolve_apply_seccomp_prefix`] always returns `None` here: the
//! `apply-seccomp` binary and its baked-in BPF filter are P7. Consequently
//! [`build_sandbox_command`] always takes the no-seccomp branch (socat listeners
//! plus `eval`), and [`wrap_command_with_sandbox_linux`] never emits a seccomp
//! prefix. The `allow_all_unix_sockets` path skips seccomp entirely anyway.

use std::fs;
use std::path::PathBuf;

/// `isExecutable(p)` (`linux-sandbox-utils.js:285-296`): is `p` executable by
/// the current process (`access(p, X_OK)`).
///
/// On Unix this checks the file exists and has any execute bit. (The TS uses
/// `fs.accessSync(p, X_OK)`; we approximate with a `metadata` + mode check,
/// which matches for the common single-user case the dependency check serves.)
#[must_use]
pub fn is_executable(p: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(p) {
            Ok(m) => m.is_file() && (m.permissions().mode() & 0o111) != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
    }
}

/// `whichSync(cmd)` (`linux-sandbox-utils.js`): resolve `cmd` on `PATH`,
/// returning the absolute path or `None`. Backed by the `which` crate.
#[must_use]
pub fn which_sync(cmd: &str) -> Option<PathBuf> {
    which::which(cmd).ok()
}

/// Options shared by [`get_linux_dependency_status`] and
/// [`check_linux_dependencies`] — the explicit-override paths for the two
/// required binaries. (The TS `opts` also carries `seccompConfig`; seccomp is
/// the P7 seam so it is omitted — `has_seccomp_apply` is always `false`.)
#[derive(Debug, Default, Clone)]
pub struct LinuxDependencyOpts {
    /// Explicit `bwrap` path override. `None` → resolve via `PATH`.
    pub bwrap_path: Option<String>,
    /// Explicit `socat` path override. `None` → resolve via `PATH`.
    pub socat_path: Option<String>,
}

/// Structured dependency status (`getLinuxDependencyStatus`, :297-311).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxDependencyStatus {
    /// `bwrap` is installed/executable.
    pub has_bwrap: bool,
    /// `socat` is installed/executable.
    pub has_socat: bool,
    /// `apply-seccomp` is available. Always `false` here (P7 seam).
    pub has_seccomp_apply: bool,
}

/// `getLinuxDependencyStatus(opts)` (:297-311): probe each dependency. An
/// explicit path is checked with [`is_executable`]; otherwise `PATH` is probed
/// with [`which_sync`]. `has_seccomp_apply` is always `false` (P7 seam).
#[must_use]
pub fn get_linux_dependency_status(opts: &LinuxDependencyOpts) -> LinuxDependencyStatus {
    LinuxDependencyStatus {
        has_bwrap: opts
            .bwrap_path
            .as_deref()
            .map_or_else(|| which_sync("bwrap").is_some(), is_executable),
        has_socat: opts
            .socat_path
            .as_deref()
            .map_or_else(|| which_sync("socat").is_some(), is_executable),
        has_seccomp_apply: false,
    }
}

/// Structured dependency-check result (`checkLinuxDependencies`, :312-363).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxDependencyCheck {
    /// Fatal problems (missing/non-executable `bwrap` or `socat`).
    pub errors: Vec<String>,
    /// Non-fatal problems (here: seccomp unavailable).
    pub warnings: Vec<String>,
}

/// `checkLinuxDependencies(opts)` (:312-363). Error strings are byte-exact with
/// the TS reference. An explicit override path that is not executable is an
/// *error* (a directive, not a hint); a missing `PATH` binary is also an error.
/// Seccomp being unavailable is a warning (P7 seam → always emitted).
#[must_use]
pub fn check_linux_dependencies(opts: &LinuxDependencyOpts) -> LinuxDependencyCheck {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Some(p) = opts.bwrap_path.as_deref() {
        if !is_executable(p) {
            errors.push(format!("bubblewrap (bwrap) not executable at {p}"));
        }
    } else if which_sync("bwrap").is_none() {
        errors.push("bubblewrap (bwrap) not installed".to_string());
    }

    if let Some(p) = opts.socat_path.as_deref() {
        if !is_executable(p) {
            errors.push(format!("socat not executable at {p}"));
        }
    } else if which_sync("socat").is_none() {
        errors.push("socat not installed".to_string());
    }

    // Seccomp is the P7 seam — `apply-seccomp` is never available here, so the
    // warning is always emitted (matching the TS branch when no binary resolves).
    warnings.push("seccomp not available - unix socket access not restricted".to_string());

    LinuxDependencyCheck { errors, warnings }
}

/// `resolveApplySeccompPrefix(...)` (:485-498) — **P7 seam**: always returns
/// `None`. The `apply-seccomp` binary/BPF filter is implemented in P7; until
/// then there is no seccomp prefix, so [`build_sandbox_command`] takes the
/// `eval` branch and [`wrap_command_with_sandbox_linux`] emits no prefix.
#[must_use]
pub fn resolve_apply_seccomp_prefix() -> Option<String> {
    None
}

/// `cleanupBwrapMountPoints(mountPoints)` (:247-283), refactored to take the
/// mount-point slice explicitly (see the module-level design note — no global
/// `activeSandboxCount`/`bwrapMountPoints`).
///
/// For each mount point: if it is still the empty (size-0) **file** bwrap
/// created, unlink it; if it is an empty **directory** (an intermediate-component
/// mount point), rmdir it. Anything with real content is left alone. All errors
/// are ignored — the file may already be gone.
pub fn cleanup_bwrap_mount_points(mount_points: &[PathBuf]) {
    for mount_point in mount_points {
        let Ok(stat) = fs::symlink_metadata(mount_point) else {
            // Ignore cleanup errors — the file may have already been removed.
            continue;
        };
        if stat.is_file() && stat.len() == 0 {
            let _ = fs::remove_file(mount_point);
        } else if stat.is_dir() {
            // Only remove if still empty (intermediate-component mount point).
            if let Ok(mut entries) = fs::read_dir(mount_point) {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(mount_point);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_check_explicit_paths_not_executable_are_errors() {
        let opts = LinuxDependencyOpts {
            bwrap_path: Some("/nonexistent/bwrap".to_string()),
            socat_path: Some("/nonexistent/socat".to_string()),
        };
        let res = check_linux_dependencies(&opts);
        assert!(res
            .errors
            .contains(&"bubblewrap (bwrap) not executable at /nonexistent/bwrap".to_string()));
        assert!(res
            .errors
            .contains(&"socat not executable at /nonexistent/socat".to_string()));
        // Seccomp warning always present (P7 seam).
        assert!(res
            .warnings
            .contains(&"seccomp not available - unix socket access not restricted".to_string()));
    }

    #[test]
    fn dep_check_no_overrides_path_lookup() {
        // No explicit overrides → the PATH branch runs. The error set is exactly
        // the not-installed messages for whichever binary is absent on this host.
        // (On a host where both exist, errors is empty.) We assert the strings the
        // PATH-miss branch produces match the byte-exact TS constants.
        let opts = LinuxDependencyOpts::default();
        let res = check_linux_dependencies(&opts);
        for err in &res.errors {
            assert!(
                err == "bubblewrap (bwrap) not installed" || err == "socat not installed",
                "unexpected PATH-branch error: {err}"
            );
        }
        // bwrap absence → exactly this string; socat absence → exactly that one.
        if which_sync("bwrap").is_none() {
            assert!(res
                .errors
                .contains(&"bubblewrap (bwrap) not installed".to_string()));
        }
        if which_sync("socat").is_none() {
            assert!(res.errors.contains(&"socat not installed".to_string()));
        }
    }

    #[test]
    fn dep_status_explicit_nonexistent_is_false() {
        let opts = LinuxDependencyOpts {
            bwrap_path: Some("/nonexistent/bwrap".to_string()),
            socat_path: Some("/nonexistent/socat".to_string()),
        };
        let st = get_linux_dependency_status(&opts);
        assert!(!st.has_bwrap);
        assert!(!st.has_socat);
        assert!(!st.has_seccomp_apply);
    }

    #[test]
    fn seccomp_prefix_is_none_p7_seam() {
        assert!(resolve_apply_seccomp_prefix().is_none());
    }

    #[test]
    fn cleanup_removes_empty_file_and_empty_dir_keeps_nonempty() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_file = tmp.path().join("empty_mount");
        fs::write(&empty_file, b"").unwrap();
        let empty_dir = tmp.path().join("empty_dir");
        fs::create_dir(&empty_dir).unwrap();
        let nonempty_file = tmp.path().join("nonempty");
        fs::write(&nonempty_file, b"data").unwrap();
        let nonempty_dir = tmp.path().join("nonempty_dir");
        fs::create_dir(&nonempty_dir).unwrap();
        fs::write(nonempty_dir.join("child"), b"x").unwrap();

        let points = vec![
            empty_file.clone(),
            empty_dir.clone(),
            nonempty_file.clone(),
            nonempty_dir.clone(),
        ];
        cleanup_bwrap_mount_points(&points);

        assert!(!empty_file.exists(), "empty file should be unlinked");
        assert!(!empty_dir.exists(), "empty dir should be rmdir'd");
        assert!(nonempty_file.exists(), "non-empty file kept");
        assert!(nonempty_dir.exists(), "non-empty dir kept");
    }

    #[test]
    fn cleanup_ignores_missing_paths() {
        let missing = PathBuf::from("/nonexistent/path/xyz");
        cleanup_bwrap_mount_points(&[missing]); // must not panic
    }

    #[test]
    fn is_executable_false_for_missing() {
        assert!(!is_executable("/nonexistent/binary/xyz"));
    }
}
