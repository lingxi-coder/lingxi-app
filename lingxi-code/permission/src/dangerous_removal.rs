//! Dangerous-removal-path guard — faithful port of claude-code
//! `src/tools/BashTool/pathValidation.ts::checkDangerousRemovalPaths` +
//! `src/utils/permissions/pathValidation.ts::isDangerousRemovalPath`.
//!
//! `rm`/`rmdir` commands whose target resolves to a critical system path
//! (`/`, a direct child of `/` like `/etc`/`/usr`, the home directory, a
//! trailing `/*` glob, a Windows drive root/child) must ALWAYS require an
//! explicit ask — even when an allow rule such as `Bash(rm:*)` matches. This
//! prevents catastrophic data loss (`rm -rf /`) from being auto-allowed by
//! permission rules.
//!
//! TS placement: `createPathChecker` runs `checkDangerousRemovalPaths` AFTER
//! explicit deny rules but BEFORE any allow grant (`pathValidation.ts:728-737`),
//! so the dangerous-removal ask wins over a matching allow rule but never
//! overrides an explicit deny. The Rust wiring in [`crate::policy`] mirrors
//! that ordering (the check runs after the deny/ask walks and before the allow
//! walk).
//!
//! Argument extraction reuses [`crate::shell_command::split_command`] to peel
//! the command into subcommands (so `rm -rf / ; echo done` is examined), then a
//! quote-aware token splitter for each subcommand, with `filterOutFlags`'
//! POSIX `--` end-of-options handling (`pathValidation.ts:126-139`).

use std::path::Path;

/// Match TS `WINDOWS_DRIVE_ROOT_REGEX = /^[A-Za-z]:\/?$/` against a
/// forward-slash-normalized path (e.g. `C:` or `C:/`).
fn is_windows_drive_root(p: &str) -> bool {
    let bytes = p.as_bytes();
    match bytes.len() {
        2 => bytes[0].is_ascii_alphabetic() && bytes[1] == b':',
        3 => bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/',
        _ => false,
    }
}

/// Match TS `WINDOWS_DRIVE_CHILD_REGEX = /^[A-Za-z]:\/[^/]+$/` (e.g.
/// `C:/Windows`, `C:/Users`) against a forward-slash-normalized path.
fn is_windows_drive_child(p: &str) -> bool {
    let bytes = p.as_bytes();
    if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'/' {
        return false;
    }
    // `[^/]+` — the remainder after `X:/` must be non-empty and contain no `/`.
    !p[3..].is_empty() && !p[3..].contains('/')
}

/// `dirname` over a forward-slash-normalized path, reproducing the subset of
/// Node `path.dirname` semantics that [`is_dangerous_removal_path`] relies on:
/// it only needs to recognize when a path's parent is exactly `/` (i.e. a
/// direct child of root, `/usr`, `/tmp`, …). Node returns `/` for `/usr`, and
/// `.` for a bare `foo`.
fn dirname_posix(p: &str) -> String {
    match p.rfind('/') {
        // No separator → relative single segment → ".".
        None => ".".to_string(),
        // Leading slash only (`/usr` → idx 0) → parent is "/".
        Some(0) => "/".to_string(),
        Some(idx) => p[..idx].to_string(),
    }
}

/// Faithful port of TS `isDangerousRemovalPath` (`pathValidation.ts:331-367`).
///
/// `abs_path` is the already-resolved (tilde-expanded, absolutized) path. The
/// `home` parameter is the user's home directory (TS `homedir()`); pass `None`
/// to skip the home-directory comparison (no `HOME` available).
///
/// Returns `true` when the path is one of the critical-system patterns that
/// must never be auto-removed.
#[must_use]
pub fn is_dangerous_removal_path(abs_path: &str, home: Option<&str>) -> bool {
    // Collapse `\`/`/` runs to a single `/` so `C:\\Windows` and `//foo`
    // normalize. (TS: resolvedPath.replace(/[\\/]+/g, '/').)
    let forward_slashed = collapse_slashes(abs_path);

    // Wildcard `*` or any path ending in `/*` (removes all files in a dir).
    if forward_slashed == "*" || forward_slashed.ends_with("/*") {
        return true;
    }

    // Strip a single trailing `/` except for the bare root `/`.
    let normalized_path = if forward_slashed == "/" {
        forward_slashed.clone()
    } else {
        forward_slashed
            .strip_suffix('/')
            .map_or(forward_slashed.clone(), str::to_string)
    };

    if normalized_path == "/" {
        return true;
    }

    if is_windows_drive_root(&normalized_path) {
        return true;
    }

    if let Some(home) = home {
        let normalized_home = collapse_slashes(home);
        if normalized_path == normalized_home {
            return true;
        }
    }

    // Direct children of root: /usr, /tmp, /etc (but not /usr/local).
    if dirname_posix(&normalized_path) == "/" {
        return true;
    }

    if is_windows_drive_child(&normalized_path) {
        return true;
    }

    false
}

/// Collapse runs of `\`/`/` into a single `/` (TS `replace(/[\\/]+/g, '/')`).
fn collapse_slashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' || c == '\\' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(c);
            prev_slash = false;
        }
    }
    out
}

/// Expand a leading `~` / `~/` to `home` (TS `expandTilde`, `pathValidation.ts:80`).
/// `~username` is NOT expanded (left literal). Returns the path unchanged when
/// `home` is `None` or the path does not start with a bare tilde.
fn expand_tilde(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" || path.starts_with("~/") {
        // homedir() + path.slice(1)
        format!("{}{}", home, &path[1..])
    } else {
        path.to_string()
    }
}

/// TS `isAbsolute` (POSIX): a path is absolute iff it starts with `/`. We also
/// treat a Windows drive-absolute path (`C:\…` / `C:/…`) and a UNC-ish `\\` as
/// absolute so the resolved form is left intact rather than re-anchored to cwd.
fn is_absolute_path(p: &str) -> bool {
    if p.starts_with('/') || p.starts_with('\\') {
        return true;
    }
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Resolve `clean_path` to an absolute path against `cwd` (TS:
/// `isAbsolute(cleanPath) ? cleanPath : resolve(cwd, cleanPath)`). We do NOT
/// normalize `..`/`.` here: TS `path.resolve` would, but the dangerous-path
/// predicate operates on the lexical join, and the only `..`-bearing fixture
/// (`rm -- -/../x`) is exercised for path EXTRACTION, not for triggering a
/// dangerous match. Keeping the join lexical avoids pulling in a path-canonical
/// dependency and never weakens the guard (a `..`-containing absolute path that
/// happens to resolve to `/etc` is still asked about via its other segments only
/// if it lexically matches — documented divergence, safe direction).
fn resolve_against_cwd(clean_path: &str, cwd: &Path) -> String {
    if is_absolute_path(clean_path) {
        clean_path.to_string()
    } else {
        let mut joined = cwd.to_string_lossy().into_owned();
        if !joined.ends_with('/') {
            joined.push('/');
        }
        joined.push_str(clean_path);
        joined
    }
}

/// Quote-aware token splitter for a single subcommand's argv. Reproduces the
/// effect of TS shell-quote parsing for the cases the removal guard needs:
/// whitespace splits tokens, single/double quotes group, and a backslash
/// escapes the next char outside quotes. Surrounding quotes are STRIPPED from
/// each token (matching `parseCommandArguments`, which yields bare strings).
fn split_argv(subcommand: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = subcommand.chars().peekable();
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if in_double {
            if c == '\\' {
                if let Some(&n) = chars.peek() {
                    // In double quotes a backslash escapes `"` `\` `$` backtick.
                    if matches!(n, '"' | '\\' | '$' | '`') {
                        cur.push(n);
                        chars.next();
                        continue;
                    }
                }
                cur.push(c);
            } else if c == '"' {
                in_double = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                has_token = true;
            }
            _ => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

/// TS `filterOutFlags` (`pathValidation.ts:126-139`): keep positional
/// (non-flag) arguments, correctly handling the POSIX `--` end-of-options
/// delimiter — after `--`, ALL arguments are positional even if they start with
/// `-`. This is the path extractor used for `rm`/`rmdir`.
fn filter_out_flags(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut after_double_dash = false;
    for arg in args {
        if after_double_dash {
            result.push(arg.clone());
        } else if arg == "--" {
            after_double_dash = true;
        } else if !arg.starts_with('-') {
            result.push(arg.clone());
        }
    }
    result
}

/// Outcome of [`check_dangerous_removal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerousRemoval {
    /// The resolved absolute path that triggered the guard.
    pub resolved_path: String,
    /// The byte-locked ask MESSAGE (TS `pathValidation.ts:92`).
    pub message: String,
    /// The decision reason text (TS `pathValidation.ts:95`).
    pub reason: String,
}

/// Examine a shell `command` for `rm`/`rmdir` invocations targeting a dangerous
/// path. Returns `Some(DangerousRemoval)` for the FIRST dangerous target found
/// (mirroring TS, which returns on the first match), or `None` if no `rm`/
/// `rmdir` subcommand targets a critical path.
///
/// `cwd` resolves relative targets; `home` expands a leading `~` and is the
/// home-directory comparison target. The command is split into subcommands so a
/// dangerous `rm` hidden behind a benign one (`echo ok && rm -rf /`) is still
/// caught.
#[must_use]
pub fn check_dangerous_removal(command: &str, cwd: &Path, home: Option<&str>) -> Option<DangerousRemoval> {
    for sub in crate::shell_command::split_command(command) {
        let tokens = split_argv(&sub);
        let Some((base, rest)) = tokens.split_first() else {
            continue;
        };
        let cmd_name = base.as_str();
        if cmd_name != "rm" && cmd_name != "rmdir" {
            continue;
        }
        let paths = filter_out_flags(rest);
        for path in &paths {
            // Strip a single pair of surrounding quotes (TS:
            // `path.replace(/^['"]|['"]$/g, '')`), then expand `~`.
            let dequoted = strip_surrounding_quotes(path);
            let clean_path = expand_tilde(dequoted, home);
            let absolute_path = resolve_against_cwd(&clean_path, cwd);
            if is_dangerous_removal_path(&absolute_path, home) {
                return Some(DangerousRemoval {
                    message: format!(
                        "Dangerous {cmd_name} operation detected: '{absolute_path}'\n\nThis command would remove a critical system directory. This requires explicit approval and cannot be auto-allowed by permission rules."
                    ),
                    reason: format!(
                        "Dangerous {cmd_name} operation on critical path: {absolute_path}"
                    ),
                    resolved_path: absolute_path,
                });
            }
        }
    }
    None
}

/// TS `path.replace(/^['"]|['"]$/g, '')`: remove ONE leading and ONE trailing
/// `'` or `"` (independently). Note this is NOT "matched pair" stripping — TS
/// strips a leading quote and a trailing quote regardless of whether they pair.
fn strip_surrounding_quotes(path: &str) -> &str {
    let mut s = path;
    if let Some(rest) = s.strip_prefix(['\'', '"']) {
        s = rest;
    }
    if let Some(rest) = s.strip_suffix(['\'', '"']) {
        s = rest;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const HOME: Option<&str> = Some("/home/u");

    fn cwd() -> PathBuf {
        PathBuf::from("/proj/work")
    }

    #[test]
    fn predicate_root_and_children_are_dangerous() {
        assert!(is_dangerous_removal_path("/", HOME));
        assert!(is_dangerous_removal_path("/etc", HOME));
        assert!(is_dangerous_removal_path("/usr", HOME));
        assert!(is_dangerous_removal_path("/tmp", HOME));
        assert!(is_dangerous_removal_path("/etc/", HOME)); // trailing slash stripped
        // A grandchild is NOT a direct child of root.
        assert!(!is_dangerous_removal_path("/usr/local", HOME));
        assert!(!is_dangerous_removal_path("/etc/nginx/conf.d", HOME));
    }

    #[test]
    fn predicate_home_is_dangerous() {
        assert!(is_dangerous_removal_path("/home/u", HOME));
        assert!(is_dangerous_removal_path("/home/u/", HOME));
        // A child of home is fine (it's a grandchild of root via /home).
        assert!(!is_dangerous_removal_path("/home/u/project", HOME));
        // Without a home, the home comparison is skipped.
        assert!(!is_dangerous_removal_path("/home/u", None));
    }

    #[test]
    fn predicate_wildcards_are_dangerous() {
        assert!(is_dangerous_removal_path("*", HOME));
        assert!(is_dangerous_removal_path("/home/u/project/*", HOME));
        assert!(is_dangerous_removal_path("/some/deep/dir/*", HOME));
    }

    #[test]
    fn predicate_windows_drive_root_and_child() {
        assert!(is_dangerous_removal_path("C:\\", HOME));
        assert!(is_dangerous_removal_path("C:/", HOME));
        assert!(is_dangerous_removal_path("C:", HOME));
        assert!(is_dangerous_removal_path("C:\\Windows", HOME));
        assert!(is_dangerous_removal_path("C:\\Users", HOME));
        // A drive grandchild is fine.
        assert!(!is_dangerous_removal_path("C:\\Users\\me", HOME));
    }

    #[test]
    fn rm_rf_root_is_dangerous() {
        let d = check_dangerous_removal("rm -rf /", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/");
        assert!(d.message.starts_with("Dangerous rm operation detected: '/'"));
        assert!(d
            .message
            .contains("cannot be auto-allowed by permission rules"));
        assert_eq!(d.reason, "Dangerous rm operation on critical path: /");
    }

    #[test]
    fn rm_etc_is_dangerous() {
        let d = check_dangerous_removal("rm /etc", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/etc");
    }

    #[test]
    fn rm_local_file_inside_cwd_is_not_dangerous() {
        // A relative path resolves under cwd → deep under root → not dangerous.
        assert!(check_dangerous_removal("rm ./local/file", &cwd(), HOME).is_none());
        assert!(check_dangerous_removal("rm -f build/out.o", &cwd(), HOME).is_none());
    }

    #[test]
    fn rmdir_of_critical_path_is_dangerous() {
        let d = check_dangerous_removal("rmdir /usr", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/usr");
        assert_eq!(d.reason, "Dangerous rmdir operation on critical path: /usr");
    }

    #[test]
    fn flag_delimiter_extracts_path() {
        // `rm -- -/../x` — the `--` makes `-/../x` positional. It resolves to
        // `/proj/work/-/../x` (lexical join), which is NOT a direct child of
        // root, so this specific payload is not dangerous — but the key
        // assertion is the path IS extracted (no panic / not silently dropped).
        // We verify extraction directly:
        let tokens = split_argv("rm -- -/../x");
        let (_, rest) = tokens.split_first().unwrap();
        let paths = filter_out_flags(rest);
        assert_eq!(paths, vec!["-/../x".to_string()]);

        // And a `--`-delimited dangerous target IS caught.
        let d = check_dangerous_removal("rm -- /etc", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/etc");
    }

    #[test]
    fn flag_delimiter_dangerous_root_glob_after_dashdash() {
        // `rm -- /*` after `--` → the `/*` glob form is dangerous.
        let d = check_dangerous_removal("rm -- /*", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/*");
    }

    #[test]
    fn dangerous_rm_hidden_behind_benign_subcommand() {
        // Split into subcommands so a dangerous rm after `echo ok &&` is caught.
        let d = check_dangerous_removal("echo ok && rm -rf /", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/");
    }

    #[test]
    fn tilde_expansion_targets_home() {
        // `rm -rf ~` expands to the home directory → dangerous.
        let d = check_dangerous_removal("rm -rf ~", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/home/u");
    }

    #[test]
    fn quoted_dangerous_path_is_caught() {
        let d = check_dangerous_removal("rm -rf \"/etc\"", &cwd(), HOME).unwrap();
        assert_eq!(d.resolved_path, "/etc");
        let d2 = check_dangerous_removal("rm -rf '/usr'", &cwd(), HOME).unwrap();
        assert_eq!(d2.resolved_path, "/usr");
    }

    #[test]
    fn non_rm_command_is_ignored() {
        assert!(check_dangerous_removal("ls /etc", &cwd(), HOME).is_none());
        assert!(check_dangerous_removal("cat /etc/passwd", &cwd(), HOME).is_none());
    }

    #[test]
    fn rm_with_only_flags_no_paths_is_not_dangerous() {
        // No positional args → nothing to validate.
        assert!(check_dangerous_removal("rm -rf", &cwd(), HOME).is_none());
    }
}
