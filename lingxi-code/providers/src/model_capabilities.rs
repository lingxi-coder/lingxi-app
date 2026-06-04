//! On-disk model-capabilities cache (`~/.claude/cache/model-capabilities.json`).
//!
//! Ports claude-code `utils/model/modelCapabilities.ts` (1-119):
//! - [`ModelCapability`] mirrors `ModelCapabilitySchema { id, max_input_tokens?,
//!   max_tokens? }`. Internal-only fields (`mycro_deployments` etc.) are
//!   `.strip()`ped — we only ever read/write these three keys.
//! - [`load_cache`] reads the `{ models, timestamp }` cache file; any
//!   read/parse error → empty (the TS `loadCache` returns `null` on error,
//!   surfaced here as an empty `Vec`).
//! - [`sort_for_matching`] orders **longest id first**, then lexicographically
//!   — so a substring match prefers the most specific id.
//! - [`get_model_capability`] does a lowercased **exact** match first, then a
//!   substring match against the sorted list.
//! - [`refresh_model_capabilities`] writes the cache (mode `0o600`) only when
//!   the sorted model set actually changed.
//!
//! ## Eligibility gate (externally a no-op)
//!
//! TS `isModelCapabilitiesEligible()` requires `USER_TYPE === 'ant'` AND
//! first-party provider AND a first-party base URL. For external users
//! `USER_TYPE != 'ant'`, so [`get_model_capability`] / [`refresh_model_capabilities`]
//! are no-ops (`None` / `Ok(false)`). We port the gate faithfully via
//! [`is_model_capabilities_eligible`].
//!
//! ## Divergence (documented)
//!
//! TS `refreshModelCapabilities` paginates `anthropic.models.list({ betas })`
//! through the SDK. This port accepts the already-fetched model list (the
//! caller in api-client hits `GET /v1/models` directly), so the **fetch** is
//! not wire-identical to the SDK paginator — but the cache schema, the
//! longest-id-first sort, the match algorithm, the `0o600` mode, and the
//! write-only-when-changed behaviour are byte-faithful.

use std::path::{Path, PathBuf};

/// One model's token capabilities. Mirrors `ModelCapabilitySchema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapability {
    /// Canonical model id (e.g. `claude-opus-4-5-20251101`).
    pub id: String,
    /// Maximum input context window, if reported.
    pub max_input_tokens: Option<u32>,
    /// Maximum output tokens, if reported.
    pub max_tokens: Option<u32>,
}

impl ModelCapability {
    /// Parse one capability entry from a JSON object, mirroring
    /// `ModelCapabilitySchema().safeParse`. Requires a string `id`; the two
    /// token fields are optional non-negative integers. Returns `None` when
    /// `id` is missing/non-string (the entry is skipped, as in TS).
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let id = v.get("id")?.as_str()?.to_string();
        Some(Self {
            id,
            max_input_tokens: u32_field(v, "max_input_tokens"),
            max_tokens: u32_field(v, "max_tokens"),
        })
    }

    /// Serialize to the `.strip()`ped JSON object shape (`id` + present token
    /// fields only).
    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("id".into(), serde_json::Value::String(self.id.clone()));
        if let Some(n) = self.max_input_tokens {
            map.insert("max_input_tokens".into(), serde_json::Value::from(n));
        }
        if let Some(n) = self.max_tokens {
            map.insert("max_tokens".into(), serde_json::Value::from(n));
        }
        serde_json::Value::Object(map)
    }
}

/// Read an optional non-negative `u32` field. Negative / overflowing / non-numeric
/// values yield `None` (the field is treated as absent, matching the optional
/// Zod number).
fn u32_field(v: &serde_json::Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

/// Resolve the claude config home dir, porting `getClaudeConfigHomeDir()`
/// (`envUtils.ts:7-13`): `CLAUDE_CONFIG_DIR` if set, else `$HOME/.claude`.
fn claude_config_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".claude"))
}

/// `getCacheDir()` — `{configHome}/cache`.
fn cache_dir() -> Option<PathBuf> {
    claude_config_home_dir().map(|d| d.join("cache"))
}

/// `getCachePath()` — `{configHome}/cache/model-capabilities.json`.
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("model-capabilities.json"))
}

/// Port of `isEnvTruthy` (`envUtils.ts:32-37`): truthy iff the lowercased,
/// trimmed value is one of `1`/`true`/`yes`/`on`.
fn is_env_truthy(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| {
        matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on")
    })
}

/// Port of `getAPIProvider() === 'firstParty'` (`model/providers.ts:6-14`):
/// first-party unless one of the Bedrock/Vertex/Foundry env flags is truthy.
fn is_first_party_provider() -> bool {
    !is_env_truthy("CLAUDE_CODE_USE_BEDROCK")
        && !is_env_truthy("CLAUDE_CODE_USE_VERTEX")
        && !is_env_truthy("CLAUDE_CODE_USE_FOUNDRY")
}

/// Port of `isFirstPartyAnthropicBaseUrl()` (`model/providers.ts:25-40`):
/// true when `ANTHROPIC_BASE_URL` is unset or its host is `api.anthropic.com`
/// (plus `api-staging.anthropic.com` for ant users). A malformed URL → false.
fn is_first_party_base_url() -> bool {
    let Some(base) = std::env::var_os("ANTHROPIC_BASE_URL") else {
        return true;
    };
    let base = base.to_string_lossy();
    if base.is_empty() {
        return true;
    }
    // Extract the host the same way `new URL(base).host` would: strip the
    // scheme, take up to the first `/`, drop any userinfo/port suffix.
    let Some(host) = parse_host(&base) else {
        return false;
    };
    if host == "api.anthropic.com" {
        return true;
    }
    is_ant() && host == "api-staging.anthropic.com"
}

/// Best-effort host extraction matching the subset of WHATWG-URL parsing the
/// TS gate relies on. Returns `None` for inputs without a scheme separator
/// (which `new URL()` would throw on → the TS `catch` returns false).
fn parse_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Drop userinfo (`user:pass@host`).
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Drop the port. (IPv6 literals aren't expected for these hosts.)
    let host = host_port.split_once(':').map_or(host_port, |(h, _)| h);
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// `process.env.USER_TYPE === 'ant'`.
fn is_ant() -> bool {
    std::env::var("USER_TYPE").is_ok_and(|v| v == "ant")
}

/// Port of `isModelCapabilitiesEligible()` (`modelCapabilities.ts:46-51`):
/// ant user AND first-party provider AND first-party base URL. For external
/// users this is always `false`, making the public accessors no-ops.
#[must_use]
pub fn is_model_capabilities_eligible() -> bool {
    is_ant() && is_first_party_provider() && is_first_party_base_url()
}

/// Sort longest-id-first, secondary lexicographic — `sortForMatching`
/// (`modelCapabilities.ts:54-58`). Stable secondary key so equality checks
/// (the write-skip comparison) are order-independent.
#[must_use]
pub fn sort_for_matching(models: &[ModelCapability]) -> Vec<ModelCapability> {
    let mut out = models.to_vec();
    out.sort_by(|a, b| {
        b.id.len()
            .cmp(&a.id.len())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Read and parse the cache file at `path` — `loadCache`
/// (`modelCapabilities.ts:61-73`). Any error (missing file, bad JSON, schema
/// mismatch) yields an empty `Vec` (TS returns `null`, which all callers treat
/// as "no cache"). The returned models are NOT re-sorted (the file already
/// stores them sorted).
#[must_use]
pub fn load_cache(path: &Path) -> Vec<ModelCapability> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(models) = value.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    // `CacheFileSchema` requires a numeric `timestamp`; reject the file if it's
    // absent (parity with the Zod `.safeParse` failing → null).
    if value.get("timestamp").and_then(serde_json::Value::as_f64).is_none() {
        return Vec::new();
    }
    models.iter().filter_map(ModelCapability::from_json).collect()
}

/// Look up a model's capabilities — `getModelCapability`
/// (`modelCapabilities.ts:75-83`). Returns `None` when ineligible (external
/// no-op), the cache is empty, or no id matches. Match order: lowercased
/// **exact** id, then the first sorted entry whose (lowercased) id is a
/// substring of the requested model — longest id wins by construction.
#[must_use]
pub fn get_model_capability(model: &str) -> Option<ModelCapability> {
    if !is_model_capabilities_eligible() {
        return None;
    }
    let path = cache_path()?;
    let cached = load_cache(&path);
    get_model_capability_in(model, &cached)
}

/// The pure matching core, testable without the eligibility gate / filesystem.
/// `cached` is expected to be sorted longest-id-first (as written by
/// [`refresh_model_capabilities`]); callers that pass an unsorted slice get the
/// same exact-match behaviour but substring ties resolve by slice order.
#[must_use]
pub fn get_model_capability_in(
    model: &str,
    cached: &[ModelCapability],
) -> Option<ModelCapability> {
    if cached.is_empty() {
        return None;
    }
    let m = model.to_lowercase();
    if let Some(exact) = cached.iter().find(|c| c.id.to_lowercase() == m) {
        return Some(exact.clone());
    }
    cached
        .iter()
        .find(|c| m.contains(&c.id.to_lowercase()))
        .cloned()
}

/// Persist a freshly-fetched model list to the cache, writing only when the
/// sorted set changed — `refreshModelCapabilities` (`modelCapabilities.ts:85-118`),
/// minus the network fetch (the caller supplies `fetched`).
///
/// Returns `Ok(true)` if a write happened, `Ok(false)` if skipped (ineligible,
/// empty input, or unchanged). The file is written with mode `0o600` on Unix.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] if creating the cache directory or
/// writing the file fails.
pub fn refresh_model_capabilities(fetched: &[ModelCapability]) -> std::io::Result<bool> {
    if !is_model_capabilities_eligible() {
        return Ok(false);
    }
    if fetched.is_empty() {
        return Ok(false);
    }
    let Some(path) = cache_path() else {
        return Ok(false);
    };
    let models = sort_for_matching(fetched);
    // `isEqual(loadCache(path), models)` — skip when unchanged.
    if load_cache(&path) == models {
        return Ok(false);
    }
    write_cache(&path, &models)?;
    Ok(true)
}

/// Write the `{ models, timestamp }` cache file (mode `0o600` on Unix),
/// creating the cache dir if needed. Separated for direct testing without the
/// eligibility gate.
///
/// # Errors
/// Propagates I/O errors from directory creation or file write.
pub fn write_cache(path: &Path, models: &[ModelCapability]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let payload = serde_json::json!({
        "models": models.iter().map(ModelCapability::to_json).collect::<Vec<_>>(),
        "timestamp": timestamp,
    });
    let body = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    write_with_mode_0600(path, body.as_bytes())
}

#[cfg(unix)]
fn write_with_mode_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_with_mode_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Non-Unix platforms can't set POSIX file modes; write plainly (TS's
    // `mode: 0o600` is silently ignored on Windows too).
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(id: &str, input: Option<u32>, max: Option<u32>) -> ModelCapability {
        ModelCapability {
            id: id.to_string(),
            max_input_tokens: input,
            max_tokens: max,
        }
    }

    #[test]
    fn sort_is_longest_id_first_then_lexicographic() {
        let models = vec![
            cap("claude-opus-4", None, None),
            cap("claude-opus-4-5", None, None),
            cap("claude-haiku-4", None, None),
        ];
        let sorted = sort_for_matching(&models);
        // claude-opus-4-5 (15) > claude-haiku-4 (14) == claude-opus-4 (13)?
        // lengths: "claude-opus-4-5"=15, "claude-haiku-4"=14, "claude-opus-4"=13
        assert_eq!(sorted[0].id, "claude-opus-4-5");
        assert_eq!(sorted[1].id, "claude-haiku-4");
        assert_eq!(sorted[2].id, "claude-opus-4");
    }

    #[test]
    fn substring_match_prefers_longest_id() {
        let models = sort_for_matching(&[
            cap("claude-opus-4", Some(200_000), Some(8192)),
            cap("claude-opus-4-5", Some(500_000), Some(64_000)),
        ]);
        let hit = get_model_capability_in("claude-opus-4-5-20251101", &models)
            .expect("substring match");
        assert_eq!(hit.id, "claude-opus-4-5");
        assert_eq!(hit.max_input_tokens, Some(500_000));
    }

    #[test]
    fn exact_match_beats_substring() {
        let models = sort_for_matching(&[
            cap("claude-opus-4", Some(1), None),
            cap("claude-opus-4-5", Some(2), None),
        ]);
        let hit = get_model_capability_in("claude-opus-4", &models).unwrap();
        assert_eq!(hit.id, "claude-opus-4");
        assert_eq!(hit.max_input_tokens, Some(1));
    }

    #[test]
    fn match_is_case_insensitive() {
        let models = sort_for_matching(&[cap("Claude-Opus-4-5", Some(9), None)]);
        let hit = get_model_capability_in("CLAUDE-OPUS-4-5-20251101", &models).unwrap();
        assert_eq!(hit.id, "Claude-Opus-4-5");
    }

    #[test]
    fn no_match_returns_none() {
        let models = sort_for_matching(&[cap("claude-opus-4-5", None, None)]);
        assert!(get_model_capability_in("gpt-4o", &models).is_none());
    }

    #[test]
    fn empty_cache_returns_none() {
        assert!(get_model_capability_in("anything", &[]).is_none());
    }

    #[test]
    fn missing_file_loads_empty() {
        let path = std::env::temp_dir().join("lingxi-mc-does-not-exist-xyz.json");
        let _ = std::fs::remove_file(&path);
        assert!(load_cache(&path).is_empty());
    }

    #[test]
    fn malformed_file_loads_empty() {
        let dir = std::env::temp_dir().join(format!("lingxi-mc-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-capabilities.json");
        std::fs::write(&path, "this is not json").unwrap();
        assert!(load_cache(&path).is_empty());
        // Missing timestamp → empty too.
        std::fs::write(&path, r#"{"models":[{"id":"x"}]}"#).unwrap();
        assert!(load_cache(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_load_round_trips_and_strips_extra_fields() {
        let dir = std::env::temp_dir().join(format!("lingxi-mc-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-capabilities.json");
        let models = sort_for_matching(&[
            cap("claude-opus-4-5", Some(500_000), Some(64_000)),
            cap("claude-opus-4", Some(200_000), None),
        ]);
        write_cache(&path, &models).unwrap();
        let loaded = load_cache(&path);
        assert_eq!(loaded, models, "round-trip preserves sorted models");

        // The on-disk file should not carry any extra keys per model.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("mycro"), "internal fields stripped");
        assert!(raw.contains("\"timestamp\""));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode 0o600");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_ignores_unknown_per_model_fields() {
        let dir = std::env::temp_dir().join(format!("lingxi-mc-strip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-capabilities.json");
        std::fs::write(
            &path,
            r#"{"models":[{"id":"claude-opus-4-5","max_input_tokens":500000,
                "max_tokens":64000,"mycro_deployments":["x"]}],"timestamp":1}"#,
        )
        .unwrap();
        let loaded = load_cache(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "claude-opus-4-5");
        assert_eq!(loaded[0].max_input_tokens, Some(500_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_skips_when_unchanged_via_refresh_core() {
        // Exercise the write-skip comparison directly: a load that equals the
        // sorted set means refresh would skip. We test the comparison the same
        // way refresh does, without the eligibility gate.
        let dir = std::env::temp_dir().join(format!("lingxi-mc-skip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-capabilities.json");
        let models = sort_for_matching(&[cap("claude-opus-4-5", Some(500_000), Some(64_000))]);
        write_cache(&path, &models).unwrap();
        // Equal → would skip.
        assert_eq!(load_cache(&path), models);
        // Changed → not equal → would write.
        let changed = sort_for_matching(&[cap("claude-opus-4-5", Some(999), Some(64_000))]);
        assert_ne!(load_cache(&path), changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_is_noop_when_ineligible_external() {
        // External users (USER_TYPE != 'ant') → ineligible → Ok(false), no write.
        // We don't mutate process env (parallel tests), so we just assert the
        // gate is false in this test process and refresh returns false.
        if !is_model_capabilities_eligible() {
            let models = vec![cap("claude-opus-4-5", Some(1), None)];
            assert!(!refresh_model_capabilities(&models).unwrap());
        }
        // get_model_capability is likewise a no-op when ineligible.
        if !is_model_capabilities_eligible() {
            assert!(get_model_capability("claude-opus-4-5-20251101").is_none());
        }
    }

    #[test]
    fn host_parsing_matches_first_party_rules() {
        assert_eq!(parse_host("https://api.anthropic.com").as_deref(), Some("api.anthropic.com"));
        assert_eq!(
            parse_host("https://user:pw@api.anthropic.com:443/v1").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(parse_host("https://evil.example.com/v1").as_deref(), Some("evil.example.com"));
        // No scheme separator → None (TS `new URL()` throws → catch returns false).
        assert!(parse_host("not-a-url").is_none());
    }
}
