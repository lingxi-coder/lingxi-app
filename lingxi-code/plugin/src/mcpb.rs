//! `.mcpb` bundle install (Stage 3): unpack a zip plugin bundle with
//! path-traversal + too-many-files + zip-bomb guards, verify its content hash,
//! and normalize it into a loadable plugin directory.
//!
//! A `.mcpb` is a zip archive. The common bundle ships a plugin tree with
//! `.lingxi-plugin/plugin.json`; an MCP-style bundle roots a `manifest.json`
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
    unpack_mcpb_limited(bytes, dest, MAX_FILES, MAX_TOTAL_BYTES)
}

/// [`unpack_mcpb`] with explicit limits (so tests can exercise the guards
/// without building a multi-gigabyte archive).
fn unpack_mcpb_limited(
    bytes: &[u8],
    dest: &Path,
    max_files: usize,
    max_total: u64,
) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Failed to extract MCPB {}: {e}", dest.display()))?;
    if zip.len() > max_files {
        // Binary: `Archive contains too many files: ${fileCount} (max: ${MAX_FILE_COUNT})`.
        return Err(format!(
            "Archive contains too many files: {} (max: {max_files})",
            zip.len()
        ));
    }
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Failed to extract MCPB {}: {e}", dest.display()))?;
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
            // Zip-bomb guard on the ACTUAL decompressed bytes: `entry.size()` is
            // the attacker-controlled central-directory claim and can lie (a 10-
            // byte claim can deflate-expand to gigabytes), so DO NOT trust it.
            // Read through a `take()` bounded at the remaining byte budget so
            // decompression aborts mid-stream instead of materializing the whole
            // payload, and count the real bytes produced.
            let remaining = max_total.saturating_sub(total);
            // Pre-allocate at most the smaller of the claim and the budget so a
            // lying large claim cannot force a huge up-front allocation either.
            let cap = usize::try_from(entry.size().min(remaining)).unwrap_or(0);
            let mut buf = Vec::with_capacity(cap);
            // `remaining + 1` so a payload exactly at the cap reads one extra
            // byte and trips the check below (never silently truncates).
            entry
                .by_ref()
                .take(remaining + 1)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            if buf.len() as u64 > remaining {
                return Err(format!(
                    "Archive total size is too large: more than {max_total} bytes. This may be a zip bomb."
                ));
            }
            total += buf.len() as u64;
            std::fs::write(&out, &buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Ensure the extracted bundle at `dir` has a `.lingxi-plugin/plugin.json` the
/// shared loader can read. If it is missing but a root `manifest.json` (MCPB
/// schema) is present, translate the `name`/`version` into a minimal synthetic
/// `plugin.json`.
///
/// # Errors
/// Returns a byte-faithful detail when neither manifest is present/valid.
pub fn ensure_plugin_manifest(dir: &Path) -> Result<(), String> {
    let plugin_json = dir.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json");
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
    std::fs::create_dir_all(dir.join(branding::PLUGIN_MANIFEST_DIR)).map_err(|e| e.to_string())?;
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

    /// The total-size guard counts ACTUAL decompressed bytes (via a take-bounded
    /// read), so it triggers regardless of what the entry header claims — the
    /// zip-bomb-via-lying-header escape the adversarial verify found is closed.
    #[test]
    fn total_size_guard_counts_real_bytes_not_the_header_claim() {
        use std::io::Write;
        // An HONEST 1000-byte entry; the bounded read counts the real bytes, so a
        // tiny 100-byte cap must reject it (the header claim is irrelevant).
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            w.start_file("big.bin", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(&vec![0u8; 1000]).unwrap();
            w.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let err = unpack_mcpb_limited(&buf, tmp.path(), 10_000, 100).unwrap_err();
        assert!(err.contains("Archive total size is too large"), "got: {err}");
    }

    #[test]
    fn too_many_files_guard_trips() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for i in 0..5 {
                w.start_file(format!("f{i}.txt"), zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let err = unpack_mcpb_limited(&buf, tmp.path(), 2, 1 << 30).unwrap_err();
        assert!(err.contains("Archive contains too many files"), "got: {err}");
    }
}
