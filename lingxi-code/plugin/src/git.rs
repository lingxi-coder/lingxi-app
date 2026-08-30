//! Git plugin-source clone (Stage 1 install arm).
//!
//! Unlike the Android `tool-git-mobile` clone (which is anchored to the sandbox
//! workspace root), the plugin cache lives under `~/.lingxi/plugins`, OUTSIDE any
//! workspace, so this clone is intentionally NOT workspace-anchored. It is a thin
//! synchronous wrapper over the same vendored libgit2; callers run it on a
//! blocking thread (`spawn_blocking`).

use std::path::Path;

/// Shallow-clone `url` at `ref_` (a branch/tag; empty = the remote's default
/// branch) into `dest`, which must NOT already exist. Returns the resolved HEAD
/// commit id on success.
///
/// # Errors
/// Returns a human-readable message on an unsupported URL protocol or any clone
/// failure (the string is surfaced verbatim as the install error detail).
pub fn clone_plugin_git(url: &str, ref_: &str, dest: &Path) -> Result<String, String> {
    // URL protocol guard: HTTPS / HTTP / file:// and SSH (`git@…`) only.
    let supported = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("file://")
        || url.starts_with("git@")
        || url.starts_with("ssh://");
    if !supported {
        return Err(format!(
            "Invalid git URL protocol: {url}. Only HTTPS, HTTP, file:// and SSH (git@) URLs are supported."
        ));
    }

    let mut fetch_opts = git2::FetchOptions::new();
    // Shallow (`--depth 1`) for remotes; the libgit2 LOCAL transport rejects a
    // shallow fetch, and git itself does a full clone for `file://` anyway, so
    // skip depth for local sources.
    if !url.starts_with("file://") {
        fetch_opts.depth(1);
    }

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    if !ref_.is_empty() {
        builder.branch(ref_);
    }

    let repo = builder
        .clone(url, dest)
        .map_err(|e| format!("Failed to clone repository: {}", e.message()))?;
    // Recurse submodules (the binary clones with `--recurse-submodules`); a
    // plugin whose components live in a submodule would otherwise land an
    // incomplete tree. (`--shallow-submodules` is not expressible via this git2
    // API, so submodules fetch at full depth — more data, never less.)
    update_submodules_recursive(&repo)
        .map_err(|e| format!("Failed to clone repository: {}", e.message()))?;
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.message().to_string())?;
    Ok(head.id().to_string())
}

/// Initialise + update every submodule, recursing into nested submodules
/// (the binary's `git submodule update --init --recursive`).
fn update_submodules_recursive(repo: &git2::Repository) -> Result<(), git2::Error> {
    for mut submodule in repo.submodules()? {
        submodule.update(true, None)?; // init = true
        let sub_repo = submodule.open()?;
        update_submodules_recursive(&sub_repo)?;
    }
    Ok(())
}

/// Mirrors the oracle's `ffe`/`pfe` confusable-URL guard (2.1.251), applied
/// at the official-name check, URL-normalisation credential stripping, the
/// policy allowlist matcher, and marketplace classification. A git-ish
/// locator is "suspicious" when it is shaped so two different parsers could
/// disagree about which host it actually names:
///
/// - `scheme://authority/...` with a "special" scheme (`http`, `https`,
///   `ws`, `wss`, `ftp`) whose RAW authority contains a backslash. Some URL
///   parsers (browsers, `URL`) treat `\` as `/` inside the authority for
///   these schemes only, so `https://evil.com\@good.com/` can resolve to
///   `evil.com` in one parser and `good.com` in another that just splits on
///   `@`.
/// - `scheme://authority/...` with any OTHER scheme (`git:`, `git+ssh:`,
///   `ssh:`, …) whose hostname contains `%`, a control character, DEL, or
///   anything above ASCII (oracle: `/[%\x00-\x1f\x7f-\u{10FFFF}]/u`) — bytes
///   a host has no legitimate reason to carry.
/// - the scp shorthand `[user@]host:path` (no `scheme://` at all) where a
///   `:` appears before the first `@`. A colon has no business inside the
///   user segment of a real scp locator, and its presence there is exactly
///   what lets one parser read the string differently from another.
#[must_use]
pub fn is_suspicious_url(candidate: &str) -> bool {
    if let Some(scheme_end) = candidate.find("://") {
        let scheme = &candidate[..scheme_end];
        let after_scheme = &candidate[scheme_end + 3..];
        let authority_end = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let authority = &after_scheme[..authority_end];
        return if matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ws" | "wss" | "ftp"
        ) {
            authority.contains('\\')
        } else {
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
            host.chars()
                .any(|c| c == '%' || (c as u32) <= 0x1f || (c as u32) >= 0x7f)
        };
    }
    match (candidate.find(':'), candidate.find('@')) {
        (Some(colon), Some(at)) => colon < at,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_protocol() {
        let err = clone_plugin_git("ftp://evil/x", "", Path::new("/tmp/nope")).unwrap_err();
        assert!(err.contains("Invalid git URL protocol"), "{err}");
        assert!(err.contains("ftp://evil/x"), "{err}");
    }

    #[test]
    fn flags_backslash_in_http_authority() {
        // The oracle's own confusable example: some URL parsers treat `\`
        // as `/` inside an http(s) authority, so this can resolve to either
        // `evil.com` or `good.com` depending on which parser reads it.
        assert!(is_suspicious_url(r"https://evil.com\@good.com/"));
        assert!(is_suspicious_url(r"http://evil.com\@good.com/"));
    }

    #[test]
    fn ordinary_urls_are_not_suspicious() {
        assert!(!is_suspicious_url("https://github.com/owner/repo.git"));
        assert!(!is_suspicious_url("git+ssh://git@github.com/owner/repo.git"));
        assert!(!is_suspicious_url("git@github.com:owner/repo.git"));
        assert!(!is_suspicious_url("ssh://git@github.com:22/owner/repo.git"));
    }

    #[test]
    fn flags_control_characters_in_non_http_hostname() {
        // oracle: /[%\x00-\x1f\x7f-\u{10FFFF}]/u applied to the hostname of
        // any scheme outside http/https/ws/wss/ftp.
        assert!(is_suspicious_url("ssh://evil%00host/owner/repo.git"));
        assert!(is_suspicious_url("git+ssh://user@evil\u{0007}host/repo.git"));
    }

    #[test]
    fn flags_colon_before_at_in_scp_form() {
        // A real scp locator never has a colon inside the user segment
        // before the `@`; its presence is the confusion signal itself.
        assert!(is_suspicious_url("evil:trick@host:path"));
        assert!(!is_suspicious_url("git@github.com:owner/repo.git"));
        assert!(!is_suspicious_url("host:path"));
    }
}
