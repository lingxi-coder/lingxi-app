//! Shared directory-path validation for the Glob and Grep search tools.
//!
//! 1:1 port of claude-code v2.1.183's identical `validateInput({path})` on both
//! the Glob (`T9`, @200997711) and Grep (@200994235) tools:
//! ```js
//! async validateInput({path:e}){
//!   if(e){
//!     let t=jt(), n=Ds(e);
//!     if(n.startsWith("\\\\")||n.startsWith("//")) return{result:!0};   // UNC skip
//!     let r;
//!     try{ r=await t.stat(n) }
//!     catch(o){ if(Pn(o)){                                              // ENOENT
//!       let s=await moe(n), i=`Directory does not exist: ${e}. ${CB} ${Pt()}.`;
//!       if(s) i+=` Did you mean ${s}?`;
//!       return{result:!1, message:i, errorCode:1}
//!     } throw o }
//!     if(!r.isDirectory()) return{result:!1, message:`Path is not a directory: ${e}`, errorCode:2}
//!   }
//!   return{result:!0}
//! }
//! ```
//! where `CB="Note: your current working directory is"` (@193229470), `Ds(e)`
//! resolves a relative path against the cwd, and `Pt()` is the cwd.
//!
//! Divergences (documented): claude-code's `errorCode` 1/2 is not carried —
//! [`ValidationError`] is message-only, so only the model-visible message is
//! reproduced. A non-ENOENT `stat` error (claude-code `throw o`) is allowed to
//! fall through to the tool's own `canonicalize_and_validate` rather than
//! erroring here (the rare permission-on-stat case).

use std::path::{Path, PathBuf};
use tool_api::tool_trait::ValidationError;

/// claude-code `CB` (binary @193229470) — the sentence between the missing path
/// and the cwd in the "Directory does not exist" message.
const CWD_NOTE_PREFIX: &str = "Note: your current working directory is";

/// Validate the optional `path` argument of a search tool (Glob/Grep). `cwd` is
/// the tool's effective working directory (`Pt()` analogue). Returns the
/// byte-exact claude-code error when the path is missing (errorCode 1) or is a
/// non-directory (errorCode 2); `Ok(())` when the path is absent, a UNC path, a
/// real directory, or fails `stat` for a non-ENOENT reason.
pub(crate) fn validate_search_directory(path: &str, cwd: &Path) -> Result<(), ValidationError> {
    // claude-code `Pt()` (Node `process.cwd()`) is the realpath, and `moe`
    // realpath's the candidate's dirname — so both sides of its containment
    // check are canonical. Canonicalize the cwd here so they agree even when the
    // workspace path traverses a symlink (e.g. macOS `/var` → `/private/var`);
    // the "current working directory" note then also shows the realpath, as
    // claude-code does.
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let cwd = cwd.as_path();
    // `Ds(e)`: resolve a relative path against the cwd.
    let resolved = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    // `n.startsWith("\\\\")||n.startsWith("//")` — UNC / network paths skip
    // validation entirely.
    let resolved_str = resolved.to_string_lossy();
    if resolved_str.starts_with("\\\\") || resolved_str.starts_with("//") {
        return Ok(());
    }
    // `stat` follows symlinks, matching JS `fs.stat`.
    match std::fs::metadata(&resolved) {
        Ok(meta) => {
            if !meta.is_dir() {
                // errorCode 2.
                return Err(ValidationError(format!("Path is not a directory: {path}")));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // errorCode 1 — note the message uses the RAW input `path`, not the
            // resolved one (claude-code `${e}`), and the cwd from `Pt()`.
            let mut message = format!(
                "Directory does not exist: {path}. {CWD_NOTE_PREFIX} {}.",
                cwd.display()
            );
            if let Some(suggestion) = suggest_sibling_dir(&resolved, cwd) {
                message.push_str(&format!(" Did you mean {}?", suggestion.display()));
            }
            Err(ValidationError(message))
        }
        // claude-code `throw o`; here we defer to the tool's own path guard.
        Err(_) => Ok(()),
    }
}

/// 1:1 port of claude-code `moe(e)` (binary @ the search-tool region): a
/// "did you mean" suggestion that detects a path given relative to the cwd's
/// PARENT and points at the same name relative to the cwd. Returns the candidate
/// absolute path iff it exists.
///
/// ```js
/// function moe(e){
///   let t=Pt(), n=pf.dirname(t), r=e;
///   try{ let u=await realpath(pf.dirname(e)); r=pf.join(u, pf.basename(e)) }catch{}
///   let o=n===pf.sep?pf.sep:n+pf.sep;
///   if(!r.startsWith(o) || r.startsWith(t+pf.sep) || r===t) return;
///   let l=pf.relative(n,r), c=pf.join(t,l);
///   try{ return await stat(c), c }catch{ return }
/// }
/// ```
/// (the win32 `toLowerCase` case-fold is dropped — posix.)
fn suggest_sibling_dir(resolved: &Path, cwd: &Path) -> Option<PathBuf> {
    // `n = dirname(t)` — the cwd's parent. No parent (cwd is root) ⇒ no suggestion.
    let cwd_parent = cwd.parent()?;
    // `r = join(realpath(dirname(e)), basename(e))`, falling back to `e` when the
    // dirname can't be realpath'd (claude-code's `catch{}`).
    let resolved_dir = resolved.parent();
    let r = match (
        resolved_dir.and_then(|d| std::fs::canonicalize(d).ok()),
        resolved.file_name(),
    ) {
        (Some(real_parent), Some(base)) => real_parent.join(base),
        _ => resolved.to_path_buf(),
    };
    // Bail unless `r` is strictly under the cwd's parent AND not under the cwd
    // itself AND not the cwd.
    if !r.starts_with(cwd_parent) || r == cwd_parent || r.starts_with(cwd) || r == cwd {
        return None;
    }
    // `l = relative(parent, r); c = join(cwd, l)`.
    let l = r.strip_prefix(cwd_parent).ok()?;
    let candidate = cwd.join(l);
    std::fs::metadata(&candidate).ok().map(|_| candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn absent_path_is_ok() {
        // No path argument is handled by the caller (only calls when present);
        // an existing directory passes.
        let tmp = TempDir::new().unwrap();
        assert!(validate_search_directory(tmp.path().to_str().unwrap(), tmp.path()).is_ok());
    }

    #[test]
    fn nonexistent_directory_message_is_byte_exact() {
        let tmp = TempDir::new().unwrap();
        let err = validate_search_directory("no_such_dir", tmp.path()).unwrap_err();
        // The note shows the canonical cwd (matching claude-code's realpath Pt()).
        let canon_cwd = std::fs::canonicalize(tmp.path()).unwrap();
        let expected = format!(
            "Directory does not exist: no_such_dir. Note: your current working directory is {}.",
            canon_cwd.display()
        );
        // No sibling suggestion for a fresh tmp (its parent has no `no_such_dir`).
        assert_eq!(err.0, expected);
    }

    #[test]
    fn path_is_a_file_message_is_byte_exact() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let err = validate_search_directory(file.to_str().unwrap(), tmp.path()).unwrap_err();
        assert_eq!(
            err.0,
            format!("Path is not a directory: {}", file.display())
        );
    }

    #[test]
    fn unc_path_skips_validation() {
        let tmp = TempDir::new().unwrap();
        assert!(validate_search_directory("//server/share", tmp.path()).is_ok());
    }

    #[test]
    fn did_you_mean_suggests_sibling_relative_to_cwd() {
        // Layout: parent/{cwd, sib}. A query for "../sib" resolves under parent
        // (not under cwd), and `sib` ALSO exists as join(cwd-name?) — actually
        // the heuristic maps parent/sib → cwd/sib. So create cwd/sib too.
        let parent = TempDir::new().unwrap();
        let cwd = parent.path().join("cwd");
        std::fs::create_dir(&cwd).unwrap();
        // The path the user typed, resolving (relative to cwd) to parent/sib:
        std::fs::create_dir(parent.path().join("sib")).unwrap();
        // The candidate the heuristic points at: cwd/sib.
        std::fs::create_dir(cwd.join("sib")).unwrap();
        // User typed "../sib" → resolved parent/sib EXISTS, so validate passes;
        // to exercise the suggestion we query a NON-existent parent/sib name that
        // maps to an existing cwd/<name>. Use "../nope" with cwd/nope present:
        std::fs::create_dir(cwd.join("nope")).unwrap();
        let err = validate_search_directory(parent.path().join("nope").to_str().unwrap(), &cwd)
            .unwrap_err();
        assert!(
            err.0.contains("Did you mean") && err.0.contains("nope"),
            "got: {}",
            err.0
        );
    }
}
