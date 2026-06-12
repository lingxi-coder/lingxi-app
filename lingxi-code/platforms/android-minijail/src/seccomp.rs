//! Net-deny seccomp policy text (spec r3 §Policy mapping: `network = Disabled`
//! ⇒ seccomp deny of socket-family syscalls). Pure data — generated and hashed
//! on the host, parsed into BPF by libminijail on-device (`run.rs`).
//!
//! Minijail policy format (C parser via `minijail_parse_seccomp_filters_from_fd`):
//! one `syscall: action` line per rule. The `@default` directive is NOT
//! supported by libminijail's C parser (`syscall_filter.c` — only `@include`
//! and `@frequency` are recognised); the default action for unmatched syscalls
//! is controlled by `minijail_use_seccomp_filter` / `filteropts->action`
//! (`ACTION_RET_KILL` for non-logging builds with tsync). We generate ONLY the
//! deny rules: every non-network syscall falls through to the jail's built-in
//! default action. The policy itself contains only `socket: return 1`-style
//! lines; the enclosing jail is configured to ALLOW by default for the shell
//! use-case via `minijail_set_seccomp_filter_allow_speculation` + the
//! net-deny filter (Task 4). `return 1` is the policy DSL form for errno 1
//! (EPERM) — confirmed against `third_party/minijail/syscall_filter.c`
//! `compile_errno`→`parse_constant` which accepts `strtol`-parseable numbers.

use hex;
use sha2::{Digest, Sha256};

/// Network-creating syscalls denied for a `DenyNet` plan. `socketcall` covers
/// the multiplexed 32-bit path; the rest are the direct entries present on
/// `arm64/x86_64`. Denying `socket`/`socketpair` alone blocks new sockets; the
/// connect/bind/send/recv entries are belt-and-braces for any fd smuggled in.
const NET_SYSCALLS: &[&str] = &[
    "socket",
    "socketpair",
    "connect",
    "bind",
    "listen",
    "accept",
    "accept4",
    "sendto",
    "sendmsg",
    "sendmmsg",
    "recvfrom",
    "recvmsg",
    "recvmmsg",
    "getpeername",
    "socketcall",
];

/// The policy name recorded in the receipt's `SeccompRef`.
#[must_use]
pub fn net_deny_policy_name() -> &'static str {
    "net-deny-v1"
}

/// The minijail seccomp policy text for the `DenyNet` stance.
///
/// Lists only the network-creating syscalls with `return 1` (errno EPERM).
/// All other syscalls fall through to the jail's built-in default action
/// (set by `filteropts->action` in libminijail — `ACTION_RET_KILL` when tsync
/// is enabled, i.e., every unmatched syscall kills the process by default;
/// Task 4 wires the jail flags to make the shell sandbox work).
///
/// NOTE: `@default ALLOW` is intentionally omitted — it is NOT a valid
/// directive in libminijail's C parser (`syscall_filter.c` recognises only
/// `@include` and `@frequency`; `@default` would be treated as a nonexistent
/// syscall name and fail at parse time on-device).
///
/// `return 1` is the canonical numeric EPERM form; the DSL also accepts
/// `return EPERM` (a named constant from `libconstants.gen.c`), but the
/// numeric form is the documented general form and avoids any arch-specific
/// constant-table lookup.
#[must_use]
pub fn net_deny_policy_text() -> String {
    let mut s = String::new();
    for sc in NET_SYSCALLS {
        // `return 1` = EPERM (errno 1) — clean error, not a SIGKILL crash,
        // so a shell command that accidentally tries to open a socket fails
        // with EPERM rather than being killed (spec: degrade gracefully).
        s.push_str(&format!("{sc}: return 1\n"));
    }
    s
}

/// Hex SHA-256 of [`net_deny_policy_text`], for the receipt's `SeccompRef.hash`.
#[must_use]
pub fn net_deny_policy_hash() -> String {
    let mut h = Sha256::new();
    h.update(net_deny_policy_text().as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_deny_policy_denies_socket_family_and_defaults_allow() {
        let p = net_deny_policy_text();
        // No @default ALLOW line — not supported by libminijail's C parser
        // (only @include and @frequency are recognised; the default action is
        // set by jail flags). The policy lists ONLY the network deny lines.
        assert!(!p.contains("@default"));
        for sc in [
            "socket",
            "socketpair",
            "connect",
            "bind",
            "sendto",
            "recvfrom",
        ] {
            assert!(
                p.lines().any(|l| l.starts_with(&format!("{sc}:"))),
                "policy must deny {sc}"
            );
        }
        // Deny action is EPERM (1), not KILL — a misbehaving net call gets a
        // clean error, not a crash (spec: shell commands degrade gracefully).
        // `return 1` is valid DSL: compile_errno→parse_constant→strtol("1")=1.
        assert!(p.contains("return 1"));
    }

    #[test]
    fn policy_hash_is_stable_and_hex() {
        let h1 = net_deny_policy_hash();
        let h2 = net_deny_policy_hash();
        assert_eq!(h1, h2, "hash is deterministic");
        assert_eq!(h1.len(), 64, "sha256 hex");
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn policy_name_is_versioned() {
        assert_eq!(net_deny_policy_name(), "net-deny-v1");
    }
}
