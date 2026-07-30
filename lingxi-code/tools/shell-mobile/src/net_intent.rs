//! Network-intent advisory (spec r3 §Shell tool, D10): the Shell tool is
//! deny-net, so a command whose intent is networking is refused up-front with
//! a pointer to the structured Git tool rather than executed to a confusing
//! seccomp EPERM. This is UX guidance, not a security boundary — the boundary
//! is the runner's net-deny seccomp filter.
//!
//! The tokenizer is best-effort (no real shell parse): commands that hide
//! networking via `$(echo cur)l`, `eval`, or similar tricks won't be flagged,
//! but the runner's net-deny seccomp filter is the actual boundary, so the
//! worst case is a confusing EPERM instead of a clean advisory.

/// Shell command heads that always imply network use.
///
/// Includes the classic external clients (curl/ssh/…) AND the network-capable
/// applets in the bundled toybox inventory (advertised to the model as
/// available), so an egress attempt gets this clean advisory instead of a
/// confusing mid-run seccomp EPERM. Local-only network *introspection* applets
/// (ifconfig/netstat) are deliberately excluded — they query the local
/// interface and are not egress.
const NET_HEADS: &[&str] = &[
    // External clients.
    "curl",
    "wget",
    "nc",
    "ncat",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "telnet",
    "ftp",
    // Bundled toybox networking applets.
    "netcat",
    "ping",
    "ping6",
    "ftpget",
    "ftpput",
    "httpd",
    "host",
    "sntp",
    "nbd_client",
    "nbd_server",
];

/// `git` subcommands that always hit the network.
const GIT_NET_SUBCMDS: &[&str] = &["clone", "fetch", "pull", "push", "ls-remote"];

/// `git` subcommands that hit the network ONLY in their `… update` form
/// (`git remote update`, `git submodule update`). Their other forms
/// (`git remote -v`, `git remote add`, `git submodule status`) are local and
/// must NOT be refused.
const GIT_NET_SUBCMDS_UPDATE_ONLY: &[&str] = &["remote", "submodule"];

/// If the command shows network intent, return a human advisory string
/// (for the tool error). `None` = no detected intent (still run deny-net).
#[must_use]
pub fn network_intent(command: &str) -> Option<String> {
    for seg in split_segments(command) {
        let mut words = seg.split_whitespace();
        let Some(head) = words.next() else {
            continue;
        };
        // Strip any leading non-identifier characters (e.g. `$(`, backticks)
        // and path prefix so `/usr/bin/curl` → `curl`.
        let head = head
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '/');
        let base = head.rsplit('/').next().unwrap_or(head);
        if NET_HEADS.contains(&base) {
            return Some(format!(
                "`{base}` needs network access, which the Shell tool does not allow. \
                 Use a structured network tool (e.g. Git) for remote operations."
            ));
        }
        if base == "git" {
            if let Some(sub) = words.next() {
                let is_net = GIT_NET_SUBCMDS.contains(&sub)
                    || (GIT_NET_SUBCMDS_UPDATE_ONLY.contains(&sub)
                        && words.next() == Some("update"));
                if is_net {
                    return Some(format!(
                        "`git {sub}` needs network access, which the Shell tool does not allow. \
                         Use the Git tool for remote git operations; local git (status/diff/commit/log) \
                         works here."
                    ));
                }
            }
        }
    }
    None
}

/// Split a command line into top-level segments on `|`, `&&`, `||`, `;`, `&`,
/// and newlines. Best-effort (ignores quoting/`$()` — the runner's seccomp is
/// the real boundary; this is advisory).
///
/// `&&` and `||` collapse to a single boundary (they are two-char operators
/// and must not produce an empty segment between them). Bare `&` (background)
/// and `|` (pipe) each produce a boundary, as do `;` and `\n`.
pub(crate) fn split_segments(command: &str) -> Vec<String> {
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut segments: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < len {
        let b = bytes[i];
        let (boundary_len, skip_extra) = match b {
            // `&&` → 2-char boundary
            b'&' if i + 1 < len && bytes[i + 1] == b'&' => (2, 0),
            // `||` → 2-char boundary
            b'|' if i + 1 < len && bytes[i + 1] == b'|' => (2, 0),
            // bare `&` or `|`, `;`, newline → 1-char boundary
            b'&' | b'|' | b';' | b'\n' => (1, 0),
            _ => {
                i += 1;
                continue;
            }
        };
        let seg = &command[start..i];
        let trimmed = seg.trim().to_string();
        if !trimmed.is_empty() {
            segments.push(trimmed);
        }
        i += boundary_len + skip_extra;
        start = i;
    }

    // Push trailing segment
    let tail = command[start..].trim();
    if !tail.is_empty() {
        segments.push(tail.to_string());
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_network_command_heads() {
        for c in [
            "git clone https://x",
            "git fetch origin",
            "git push",
            "git ls-remote origin",
            "git remote update",    // `update` form hits the network
            "git submodule update", // fetches submodules
            "curl https://x",
            "wget http://x",
            "nc 10.0.0.1 80",
            "ssh host",
            "scp a b:c",
            "echo hi && git pull", // any segment counts
            "ls | curl x",
        ] {
            assert!(network_intent(c).is_some(), "{c:?} should be flagged");
        }
    }

    #[test]
    fn allows_local_commands() {
        for c in [
            "echo hi",
            "ls -la",
            "git status",
            "git diff",
            "git commit -m x", // local git is allowed (P4 git runs deny-net)
            "git remote -v",   // local: list remotes (NOT the `update` form)
            "git remote add origin url", // local: configures a remote
            "git submodule status", // local: no `update`
            "grep -r foo .",
            "cat file | sed s/a/b/",
        ] {
            assert!(network_intent(c).is_none(), "{c:?} should be allowed");
        }
    }

    #[test]
    fn parse_failure_is_not_a_network_grant() {
        // Unparseable / weird input must NOT silently pass as "local"; the
        // caller treats `None` as "no detected net intent, run deny-net anyway"
        // — which is safe because the runner is deny-net regardless. Document
        // that the advisory is best-effort and the seccomp filter is the real
        // stop. (No assertion beyond: does not panic.)
        let _ = network_intent("$(");
    }
}
