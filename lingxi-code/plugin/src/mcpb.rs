//! `.mcpb` bundle install (Stage 3): unpack a zip plugin bundle with
//! path-traversal + too-many-files + zip-bomb guards, verify its content hash,
//! and normalize it into a loadable plugin directory.
//!
//! A `.mcpb` is a zip archive. The common bundle ships a plugin tree with
//! `.claude-plugin/plugin.json`; an MCP-style bundle roots a `manifest.json`
//! instead, which we translate into a minimal synthetic `plugin.json` so the
//! shared loader can read it.

use std::io::Read;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

/// Hard cap on archive entries (claude-code "Archive contains too many files").
const MAX_FILES: usize = 10_000;
/// Hard cap on total uncompressed bytes — zip-bomb guard.
const MAX_TOTAL_BYTES: u64 = 1 << 30; // 1 GiB

/// Lowercase hex SHA-256 of `bytes` (the bundle's only integrity check —
/// claude-code has no signature verification).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Extract the zip `bytes` into `dest`, enforcing the entry-count, total-size,
/// and path-traversal guards. `dest` must already exist.
///
/// # Errors
/// Returns the byte-faithful failure detail on a malformed archive, traversal
/// attempt, too many files, or zip-bomb.
pub fn unpack_mcpb(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Failed to extract MCPB {}: {e}", dest.display()))?;
    if zip.len() > MAX_FILES {
        return Err(format!("Archive contains too many files: {}", zip.len()));
    }
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Failed to extract MCPB {}: {e}", dest.display()))?;
        total = total.saturating_add(entry.size());
        if total > MAX_TOTAL_BYTES {
            return Err(format!(
                "Archive total size is too large: {total} bytes. This may be a zip bomb."
            ));
        }
        // `enclosed_name` returns None for absolute paths / `..` traversal; the
        // explicit component + containment checks below are belt-and-suspenders.
        let rel = entry
            .enclosed_name()
            .filter(|p| {
                !p.components()
                    .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
            })
            .ok_or_else(|| format!("Path traversal attempt detected: {}", entry.name()))?;
        let out = dest.join(&rel);
        if !out.starts_with(dest) {
            return Err(format!("Path traversal attempt detected: {}", entry.name()));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut buf = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            std::fs::write(&out, &buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Ensure the extracted bundle at `dir` has a `.claude-plugin/plugin.json` the
/// shared loader can read. If it is missing but a root `manifest.json` (MCPB
/// schema) is present, translate the `name`/`version` into a minimal synthetic
/// `plugin.json`.
///
/// # Errors
/// Returns a byte-faithful detail when neither manifest is present/valid.
pub fn ensure_plugin_manifest(dir: &Path) -> Result<(), String> {
    let plugin_json = dir.join(".claude-plugin").join("plugin.json");
    if plugin_json.exists() {
        return Ok(());
    }
    let manifest_path = dir.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|_| format!("MCPB manifest invalid at {}", dir.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Invalid JSON in manifest.json: {e}"))?;
    let name = json
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Manifest validation failed: missing name".to_string())?;
    let version = json
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0.0.0");
    std::fs::create_dir_all(dir.join(".claude-plugin")).map_err(|e| e.to_string())?;
    std::fs::write(
        &plugin_json,
        serde_json::json!({ "name": name, "version": version }).to_string(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
