//! `.mcpb` bundle install (Stage 3): unpack a zip plugin bundle with
//! path-traversal + too-many-files + zip-bomb guards, verify its content hash,
//! and normalize it into a loadable plugin directory.
//!
//! A `.mcpb` is a zip archive. The common bundle ships a plugin tree with
//! `.lingxi-plugin/plugin.json`; an MCP-style bundle roots a `manifest.json`
//! instead, which we translate into a minimal synthetic `plugin.json` so the
//! shared loader can read it.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path};

use serde::Deserialize;
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
                !p.components().any(|c| {
                    matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
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
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid JSON in manifest.json: {e}"))?;
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

// ---------- §14 row 1: the `mcpServers` MCPB/`.dxt` union arm ----------
//
// A `mcpServers` entry may itself be a relative path or URL ending in
// `.mcpb`/`.dxt` (oracle `st`), naming an MCP-style bundle whose root
// `manifest.json` (a DIFFERENT, richer schema than the plugin-bundle
// `manifest.json` [`ensure_plugin_manifest`] translates — this one is the
// public MCPB/DXT extension manifest, oracle `PMt`/`McpbManifestSchema`)
// describes exactly one MCP server. Byte-source: `getMcpConfigForManifest`
// (oracle `x`, re-exported from the extension-authoring module) plus its
// template-substitution helper `m`, both recovered verbatim from the
// 2.1.251 Mach-O.
//
// What this module implements: parsing a (lenient — see [`McpbManifest`])
// subset of that manifest, applying the `darwin`-only `platform_overrides`
// merge, the `hasRequiredConfigMissing` gate, and the `${__dirname}` /
// `${pathSeparator}` / `${/}` / `${user_config.KEY}` template substitution —
// [`generate_mcp_config`] is a direct, byte-faithful port of oracle `x`+`m`.
//
// What is NOT modelled: a persisted per-MCPB `user_config` store (oracle
// reads one via `dye(repository, manifest.name, …)`; this crate's discovery
// path has no such store wired in), so [`generate_mcp_config`] always
// evaluates the "missing required config" gate against an EMPTY provided
// config — faithful to the oracle's own semantics (`hasRequiredConfigMissing`
// reads only the *provided* value, never the declared `default`) for a
// plugin that has never been interactively configured, and the common case
// (no `user_config`, or only optional/defaulted fields) still resolves in
// full. A bundle with a `required` field is conservatively skipped rather
// than guessed at.

/// A subset of the public MCPB/DXT extension `manifest.json` schema (oracle
/// `PMt`/`McpbManifestSchema`) — only the fields [`generate_mcp_config`]
/// needs. Deliberately more lenient than the oracle schema (which requires
/// `version`/`description`/`author`/`server` and one of `dxt_version`/
/// `manifest_version`): the oracle's own runtime caller re-checks `!pe.server`
/// AFTER its parse succeeds (`uct`, 2.1.251 Mach-O @159489593), meaning the
/// schema's `server` requirement is not actually enforced on this path either
/// — the only field this port treats as load-bearing is `name` (used as the
/// resulting server's map key and in every diagnostic).
#[derive(Debug, Clone, Deserialize)]
pub struct McpbManifest {
    /// The extension's own name — becomes the generated MCP server's map key
    /// (oracle `Pit`: `{[C.manifest.name]: C.mcpConfig}`).
    pub name: String,
    /// The server declaration. Oracle-required but read leniently here (see
    /// above) — absence produces the oracle's own diagnostic rather than a
    /// parse failure.
    #[serde(default)]
    pub server: Option<McpbServer>,
    /// User-configurable fields this extension declares (oracle `fmr`,
    /// keyed by field name). Only `required` and `default` are read.
    #[serde(rename = "user_config", default)]
    pub user_config: HashMap<String, McpbUserConfigOption>,
}

/// `manifest.server` (oracle `cmr`/`McpbManifestServerSchema`): only
/// `mcp_config` is read here — `type`/`entry_point` steer the manifest
/// AUTHORING tools (`bmr`/`Tmr`/scaffold defaults), not config generation.
#[derive(Debug, Clone, Deserialize)]
pub struct McpbServer {
    /// The (possibly `platform_overrides`-carrying) MCP server config,
    /// kept as a raw JSON tree so [`generate_mcp_config`]'s template
    /// substitution can walk it generically exactly like oracle `m()`
    /// does (the schema constrains it to `{command, args, env,
    /// platform_overrides}`, but nothing here depends on that shape beyond
    /// `platform_overrides` itself).
    #[serde(rename = "mcp_config")]
    pub mcp_config: serde_json::Value,
}

/// One `manifest.user_config` entry (oracle `fmr`) — only the two fields
/// [`generate_mcp_config`]'s gate and template-substitution steps read.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpbUserConfigOption {
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// Oracle `d()`/`hasRequiredConfigMissing`: true when any `required`
/// `user_config` field has no PROVIDED value. Oracle reads the value from the
/// caller-supplied/stored config only — a declared `default` is never
/// consulted here (it is applied later, only as a template-substitution
/// value) — so with this port's always-empty provided config, this is
/// equivalent to "does the manifest declare any required field at all".
fn has_required_config_missing(manifest: &McpbManifest) -> bool {
    manifest.user_config.values().any(|field| field.required)
}

/// One template-substitution value (oracle `p`/`g` map entries): either a
/// scalar (already `String`-coerced, matching JS `String(f)` /
/// `f?"true":"false"`) or an array (`f.map(String)`), which only ever
/// participates in the array-splice special case of [`substitute`].
#[derive(Debug, Clone)]
enum TemplateVar {
    Scalar(String),
    Array(Vec<String>),
}

/// JS `String(value)` coercion for a JSON scalar (oracle's `String(f)` /
/// `f?"true":"false"` branch of the `user_config.*` seeding loop).
fn json_scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

impl TemplateVar {
    fn from_user_config_value(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Array(items) => {
                Self::Array(items.iter().map(json_scalar_to_string).collect())
            }
            other => Self::Scalar(json_scalar_to_string(other)),
        }
    }
}

/// A JSON string is exactly one `${user_config.KEY}` token (oracle's
/// `/^\$\{user_config\.[^}]+\}$/` whole-string test) — returns the inner
/// `user_config.KEY` content (oracle's SECOND regex capture,
/// `/^\$\{([^}]+)\}$/`), which doubles as the [`TemplateVar`] map key.
fn as_whole_user_config_token(s: &str) -> Option<&str> {
    let inner = s.strip_prefix("${")?.strip_suffix('}')?;
    (inner.starts_with("user_config.") && inner.len() > "user_config.".len()).then_some(inner)
}

/// Oracle `m(e,i)`: recursively substitute `${VAR}` tokens through a JSON
/// tree. A plain string gets each KNOWN var's token replaced in place (an
/// array-valued var is skipped with a warning, matching oracle's
/// `console.warn` — a `${VAR}` referring to an array value inside a larger
/// string has no defined flattening). An array element whose ENTIRE content
/// is one `${user_config.KEY}` token is spliced instead: an array-valued var
/// flattens its items into place; a scalar-valued var contributes one item;
/// an unrecognized key is kept as the literal token (none of these three
/// go through the generic string-substitution path). Every other array
/// element, and every object value, recurses normally. Numbers/bools/
/// null pass through unchanged.
fn substitute(value: &serde_json::Value, vars: &HashMap<String, TemplateVar>) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(substitute_string(s, vars)),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let serde_json::Value::String(s) = item {
                    if let Some(key) = as_whole_user_config_token(s) {
                        match vars.get(key) {
                            Some(TemplateVar::Array(values)) => out.extend(
                                values.iter().cloned().map(serde_json::Value::String),
                            ),
                            Some(TemplateVar::Scalar(v)) => {
                                out.push(serde_json::Value::String(v.clone()));
                            }
                            None => out.push(item.clone()),
                        }
                        continue;
                    }
                }
                out.push(substitute(item, vars));
            }
            serde_json::Value::Array(out)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), substitute(v, vars));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Replace every occurrence of each KNOWN var's `${name}` token in `s`
/// (oracle's per-string loop over `Object.entries(i)`, `String.replace`
/// with the JS `g`/global flag — `str::replace` already replaces every
/// match of a literal needle, so no regex is needed for this direction).
/// An array-valued var is left untouched wherever its token appears in a
/// string context (oracle: array values are not string-replaceable).
fn substitute_string(s: &str, vars: &HashMap<String, TemplateVar>) -> String {
    let mut out = s.to_string();
    for (key, val) in vars {
        let token = format!("${{{key}}}");
        if let TemplateVar::Scalar(v) = val {
            if out.contains(&token) {
                out = out.replace(&token, v);
            }
        }
    }
    out
}

/// Oracle `x()`/`getMcpConfigForManifest`: generate the MCP server config an
/// MCPB `manifest.server.mcp_config` resolves to once extracted at
/// `extension_path`. Returns `None` when the manifest has no `server` (or no
/// `mcp_config`) at all, or when a `required` `user_config` field has no
/// provided value (see module docs for why the provided config is always
/// empty in this port) — both cases the caller should treat as "this MCPB
/// source contributes no server", matching the oracle's own skip-with-warning
/// behaviour rather than an error.
#[must_use]
pub fn generate_mcp_config(
    manifest: &McpbManifest,
    extension_path: &Path,
) -> Option<serde_json::Value> {
    let server = manifest.server.as_ref()?;
    let mut config = server.mcp_config.clone();

    // Oracle: only a `darwin` platform override is ever applied at load
    // time (`win32`/`linux` keys are read by the manifest-AUTHORING prompts
    // only) — `cfg!` reads the actual build target, matching the oracle's
    // own `process.platform` runtime check for a native (non-cross-run)
    // binary.
    if cfg!(target_os = "macos") {
        if let Some(darwin) = config
            .get("platform_overrides")
            .and_then(|overrides| overrides.get("darwin"))
            .cloned()
        {
            if let Some(obj) = config.as_object_mut() {
                for key in ["command", "args", "env"] {
                    if let Some(v) = darwin.get(key) {
                        obj.insert(key.to_string(), v.clone());
                    }
                }
            }
        }
    }

    if has_required_config_missing(manifest) {
        return None;
    }

    let mut vars: HashMap<String, TemplateVar> = HashMap::new();
    vars.insert(
        "__dirname".to_string(),
        TemplateVar::Scalar(extension_path.to_string_lossy().into_owned()),
    );
    // Oracle hardcodes `pathSeparator:"/"` at the ONE call site
    // (`uct`→`MY`) regardless of host OS — MCPB bundles are authored
    // portably and always use `/` here, never `std::path::MAIN_SEPARATOR`.
    vars.insert("pathSeparator".to_string(), TemplateVar::Scalar("/".to_string()));
    vars.insert("/".to_string(), TemplateVar::Scalar("/".to_string()));
    for (key, field) in &manifest.user_config {
        if let Some(default) = &field.default {
            vars.insert(
                format!("user_config.{key}"),
                TemplateVar::from_user_config_value(default),
            );
        }
    }

    Some(substitute(&config, &vars))
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
        assert!(
            err.contains("Archive total size is too large"),
            "got: {err}"
        );
    }

    #[test]
    fn too_many_files_guard_trips() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for i in 0..5 {
                w.start_file(
                    format!("f{i}.txt"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let err = unpack_mcpb_limited(&buf, tmp.path(), 2, 1 << 30).unwrap_err();
        assert!(
            err.contains("Archive contains too many files"),
            "got: {err}"
        );
    }

    fn manifest_from(json: serde_json::Value) -> McpbManifest {
        serde_json::from_value(json).unwrap()
    }

    /// `${__dirname}` and the hardcoded `${pathSeparator}`/`${/}` tokens are
    /// substituted into `command`/`args`/`env` (oracle `x`+`m`).
    #[test]
    fn dirname_and_path_separator_tokens_are_substituted() {
        let manifest = manifest_from(serde_json::json!({
            "name": "demo",
            "server": {
                "mcp_config": {
                    "command": "node",
                    "args": ["${__dirname}/server/index.js"],
                    "env": {"SEP": "${pathSeparator}", "SLASH": "${/}"}
                }
            }
        }));
        let generated =
            generate_mcp_config(&manifest, Path::new("/plugins/demo/.mcpb-cache/abc")).unwrap();
        assert_eq!(
            generated["args"][0],
            "/plugins/demo/.mcpb-cache/abc/server/index.js"
        );
        assert_eq!(generated["env"]["SEP"], "/");
        assert_eq!(generated["env"]["SLASH"], "/");
    }

    /// A `required` `user_config` field with no provided value (this port
    /// never has a persisted store to provide one) skips config generation
    /// entirely — even when the field also declares a `default` (oracle
    /// `hasRequiredConfigMissing` reads only the provided value, never the
    /// default).
    #[test]
    fn required_user_config_with_no_provided_value_skips_generation() {
        let manifest = manifest_from(serde_json::json!({
            "name": "demo",
            "server": {"mcp_config": {"command": "node", "args": []}},
            "user_config": {"apiKey": {"required": true, "default": "unused"}}
        }));
        assert!(
            generate_mcp_config(&manifest, Path::new("/x")).is_none(),
            "a required field with no provided value must skip generation, default or not"
        );
    }

    /// An OPTIONAL `user_config` field's `default` still seeds the
    /// `${user_config.KEY}` substitution map, so a manifest with only
    /// optional/defaulted fields resolves in full without a store.
    #[test]
    fn optional_user_config_default_seeds_template_substitution() {
        let manifest = manifest_from(serde_json::json!({
            "name": "demo",
            "server": {
                "mcp_config": {
                    "command": "node",
                    "args": ["--root=${user_config.workspace}"]
                }
            },
            "user_config": {"workspace": {"required": false, "default": "/tmp/ws"}}
        }));
        let generated = generate_mcp_config(&manifest, Path::new("/x")).unwrap();
        assert_eq!(generated["args"][0], "--root=/tmp/ws");
    }

    /// An array element that is EXACTLY one `${user_config.KEY}` token
    /// splices an array-valued default in place (oracle `m`'s array-context
    /// special case), rather than stringifying the whole array into that one
    /// slot.
    #[test]
    fn whole_token_array_element_splices_an_array_valued_default() {
        let manifest = manifest_from(serde_json::json!({
            "name": "demo",
            "server": {
                "mcp_config": {
                    "command": "node",
                    "args": ["serve", "${user_config.extraFlags}", "--done"]
                }
            },
            "user_config": {"extraFlags": {"default": ["--verbose", "--json"]}}
        }));
        let generated = generate_mcp_config(&manifest, Path::new("/x")).unwrap();
        assert_eq!(
            generated["args"],
            serde_json::json!(["serve", "--verbose", "--json", "--done"])
        );
    }

    /// A manifest with no `server` at all (the oracle's own defensive
    /// `if(!pe.server)` runtime guard) yields `None`, not a parse error.
    #[test]
    fn manifest_with_no_server_yields_none() {
        let manifest = manifest_from(serde_json::json!({"name": "demo"}));
        assert!(generate_mcp_config(&manifest, Path::new("/x")).is_none());
    }

    /// darwin `platform_overrides` replace `command`/`args`/`env` on macOS
    /// only (oracle: `win32`/`linux` overrides are never applied at load
    /// time, only read by the manifest-authoring prompts).
    #[test]
    fn darwin_platform_override_replaces_command_on_macos_only() {
        let manifest = manifest_from(serde_json::json!({
            "name": "demo",
            "server": {
                "mcp_config": {
                    "command": "node",
                    "args": ["server/index.js"],
                    "platform_overrides": {
                        "darwin": {"command": "node-darwin", "args": ["mac-server.js"]}
                    }
                }
            }
        }));
        let generated = generate_mcp_config(&manifest, Path::new("/x")).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(generated["command"], "node-darwin");
            assert_eq!(generated["args"], serde_json::json!(["mac-server.js"]));
        } else {
            assert_eq!(generated["command"], "node");
            assert_eq!(generated["args"], serde_json::json!(["server/index.js"]));
        }
    }
}
