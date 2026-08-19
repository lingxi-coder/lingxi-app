//! Guest→host path translation for permission checks.
//!
//! MOBILE DIVERGENCE. There is no oracle counterpart: claude-code 2.1.235 has
//! a single filesystem coordinate space, so `filesystem.ts` never has to ask
//! "which side of a mount is this path on".
//!
//! On mobile the model names files in GUEST coordinates (`/workspace/<id>/…`,
//! the same spelling the shell and the file tools use) while every root in
//! [`crate::FsRoots`] is a HOST path. `expand_path` keeps an absolute path
//! verbatim, so `path_matches_rule_pattern` relativizes a guest path against a
//! host root, gets a `../`-prefixed string, and bails — no allow rule can ever
//! match and every edit prompts. Measured on device:
//!
//! ```text
//! host_cwd  = /private/var/mobile/Containers/Data/Application/…/LingxiCode
//! model_cwd = /workspace/30c17980-f397-434b-8dba-4c3690a9f60a
//! ```
//!
//! The fix reuses the SAME `translate_model_path` seam the file tools call
//! (`tools/file/src/write.rs`, `edit.rs`), so the gate and the tool can never
//! disagree about which mount a path belongs to.

use std::sync::Arc;

/// Answers "does this model-supplied path have a host twin?" without teaching
/// `permission` about I/O. [`traits::FileSystem::translate_model_path`] is a
/// sync trait method, so this is callable from the sync gate.
pub trait ModelPathTranslator: Send + Sync {
    /// `None` when the path has no host twin, or must not be touched.
    fn to_host(&self, model_path: &str, write: bool) -> Option<String>;
}

/// Production adapter over the session's [`traits::FileSystem`].
pub struct FileSystemPathTranslator(pub Arc<dyn traits::FileSystem>);

impl ModelPathTranslator for FileSystemPathTranslator {
    fn to_host(&self, model_path: &str, write: bool) -> Option<String> {
        // `Err(..)` means the path is inside the model-visible space but must
        // not be touched from the host (iSH fakefs regions, or a read-only
        // mount for `write == true`). Map it to `None`, NEVER to a rewrite and
        // NEVER to a verdict: the raw guest path then flows to the policy
        // exactly as it does today (relativizes to `../…`, matches no allow
        // rule, prompts), and the tool refuses it a moment later with the
        // filesystem's own message. Turning a translation refusal into an
        // allow would authorize a path the host must not reach.
        match self.0.translate_model_path(model_path, write) {
            Ok(Some(host)) => Some(host),
            Ok(None) | Err(_) => None,
        }
    }
}

/// Rewrite ONLY the tool's declared path field. `None` means nothing changed.
///
/// The path field comes from [`crate::filesystem::input_path_field_for_tool`],
/// the same helper the permission check itself uses, so the two can never
/// disagree about which field to read.
pub(crate) fn rewrite_tool_input(
    translator: &dyn ModelPathTranslator,
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<serde_json::Value> {
    let write = match crate::filesystem::file_tool_kind(tool_name) {
        crate::filesystem::FileToolKind::Editor => true,
        crate::filesystem::FileToolKind::Reader => false,
        // Bash / PowerShell / WebFetch / MCP are NEVER touched: a host
        // container path spliced into a guest shell string names a file that
        // does not exist inside the guest.
        crate::filesystem::FileToolKind::NonFile => return None,
    };
    let field = crate::filesystem::input_path_field_for_tool(tool_name);
    let raw = input.get(field)?.as_str()?;
    if !std::path::Path::new(raw).is_absolute() {
        // A relative path needs no rewrite: `expand_path` joins it onto
        // `roots.cwd`, which is already the workspace host root.
        return None;
    }
    let host = translator.to_host(raw, write)?;
    let mut out = input.clone();
    out[field] = serde_json::Value::String(host);
    Some(out)
}
