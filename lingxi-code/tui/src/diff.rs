//! `/diff`: render uncommitted working-tree changes into the transcript as
//! read-only system output.
//!
//! Faithful v1 of claude-code's `DiffDialog` (`src/commands/diff/` +
//! `src/components/diff/DiffDialog.tsx`). The reference opens an interactive
//! list/detail overlay whose current-changes source is backed by
//! `src/utils/gitDiff.ts::fetchGitDiffHunks`, which runs
//! `git --no-optional-locks diff HEAD`. Here we run that same command and print
//! its output. The per-turn-diff source and a scrollable overlay are deferred
//! nice-to-haves; the current working-tree diff is the faithful core.
//!
//! Read-only: no index, ref, or working-tree state is mutated, so this runs
//! synchronously on the ratatui slash path (same class as the local file I/O
//! `/export` performs) without an off-loop `ChatOutcome` effect.

use std::path::Path;
use std::process::Command;

/// Formatted `/diff` result plus whether it should render in the error color.
pub struct DiffOutput {
    /// The system-message body to push into the transcript.
    pub body: String,
    /// Render in the error color (a git failure) rather than as neutral output.
    pub is_error: bool,
}

/// Run `git --no-optional-locks diff HEAD` in `cwd` and format the result.
///
/// Mirrors claude-code `fetchGitDiffHunks` (`--no-optional-locks` avoids taking
/// the index lock for a pure read). An empty diff yields a friendly "no
/// changes" note; a non-zero exit (not a git repo, no commits yet, …) or a
/// spawn failure yields the captured stderr in the error color.
pub fn collect_diff(cwd: &Path) -> DiffOutput {
    match Command::new("git")
        .args(["--no-optional-locks", "diff", "HEAD"])
        .current_dir(cwd)
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                DiffOutput {
                    body: "No uncommitted changes.".to_string(),
                    is_error: false,
                }
            } else {
                DiffOutput {
                    body: trimmed.to_string(),
                    is_error: false,
                }
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = stderr.trim();
            let body = if detail.is_empty() {
                "Failed to compute diff (is this a git repository?)".to_string()
            } else {
                format!("Failed to compute diff: {detail}")
            };
            DiffOutput {
                body,
                is_error: true,
            }
        }
        Err(err) => DiffOutput {
            body: format!("Failed to run git: {err}"),
            is_error: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outside a git repository the command exits non-zero, so the helper
    /// reports an error rather than pretending there are no changes.
    #[test]
    fn non_git_dir_is_reported_as_error() {
        let tmp = std::env::temp_dir().join(format!("lingxi-diff-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let out = collect_diff(&tmp);
        assert!(out.is_error, "non-git dir should surface a git error");
        assert!(out.body.starts_with("Failed"), "body: {}", out.body);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A freshly-`git init`ed repo with a committed HEAD and no edits reports
    /// the empty-diff note (not an error).
    #[test]
    fn clean_repo_reports_no_changes() {
        let tmp = std::env::temp_dir().join(format!("lingxi-diff-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&tmp)
                .output()
                .expect("run git")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(tmp.join("a.txt"), "hello\n").expect("write file");
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        let out = collect_diff(&tmp);
        assert!(!out.is_error, "clean repo should not error: {}", out.body);
        assert_eq!(out.body, "No uncommitted changes.");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
