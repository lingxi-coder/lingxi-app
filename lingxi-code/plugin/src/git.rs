//! Git plugin-source clone (Stage 1 install arm).
//!
//! Unlike the Android `tool-git-mobile` clone (which is anchored to the sandbox
//! workspace root), the plugin cache lives under `~/.claude/plugins`, OUTSIDE any
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
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.message().to_string())?;
    Ok(head.id().to_string())
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
}
