//! Process-global invoked-skill registry — the Rust mirror of the binary's
//! module-level `Pt.invokedSkills` Map.
//!
//! A skill's expanded content is registered here when the model invokes it
//! (`zSr`, from the Skill tool) and consumed after a compaction by
//! [`crate::restore_post_compact_skills`] (`rRg`) so the model re-reads the
//! recently invoked skill instructions across the compact boundary — the skill
//! content survives compaction even though the boundary summary drops the
//! verbatim skill body.
//!
//! Keyed `"{agentId}:{skillName}"` (an empty agent prefix for the main thread),
//! each row stores `{skillName, skillPath, content, invokedAt, agentId}` exactly
//! like the binary. Unlike the per-conversation `readFileState`, the binary's
//! `Pt.invokedSkills` is a true process global, so this is a `Lazy<Mutex<..>>`
//! and is deliberately NOT cleared by post-compact cleanup — skill content must
//! outlive the compaction (see [`crate::run_post_compact_cleanup`]).
//!
//! 1:1 with the binary quartet (`bin/claude.exe`, v2.1.207):
//! - `zSr(e,t,r,n=null)` → [`register`] (stamps `invokedAt = Date.now()`).
//! - `kGo(e)` → [`filter_for_agent`] (rows whose `agentId` matches).
//! - `n_n(e,t)` → [`write_back`] (overwrite an existing row's content; `""`
//!   clears it, a no-op when the key is absent).
//! - `N$t()` → the whole map, exposed here only via the test inspectors.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::post_compact::SkillRestoreCandidate;

/// One registry row — `{skillName, skillPath, content, invokedAt, agentId}`.
#[derive(Debug, Clone)]
struct InvokedSkillEntry {
    skill_name: String,
    skill_path: PathBuf,
    content: String,
    invoked_at_ms: i64,
    agent_id: Option<String>,
}

/// The process-global registry (`Pt.invokedSkills`), keyed `"{agentId}:{name}"`.
static REGISTRY: Lazy<Mutex<HashMap<String, InvokedSkillEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, InvokedSkillEntry>> {
    REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `${agentId ?? ""}:${skillName}` — the registry key. `agent_id = None` (the
/// main thread) yields a leading `:` (empty agent prefix).
#[must_use]
pub fn registry_key(agent_id: Option<&str>, skill_name: &str) -> String {
    format!("{}:{}", agent_id.unwrap_or(""), skill_name)
}

/// `Date.now()` in ms since the Unix epoch (saturating for the pre-1970 edge).
/// Stamps `invokedAt`; restoration sorts descending on it.
fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// `zSr(name, path, content, agentId)` — register (or overwrite) an invoked
/// skill, stamping `invokedAt = Date.now()`.
///
/// The Skill tool calls this at invocation time with the expanded skill content
/// the model receives; `agent_id` is `None` for the main thread (every LingXi
/// orchestrator runs as the main thread → key `":{name}"`).
pub fn register(skill_name: &str, skill_path: &Path, content: &str, agent_id: Option<&str>) {
    let key = registry_key(agent_id, skill_name);
    let entry = InvokedSkillEntry {
        skill_name: skill_name.to_string(),
        skill_path: skill_path.to_path_buf(),
        content: content.to_string(),
        invoked_at_ms: now_ms(),
        agent_id: agent_id.map(str::to_string),
    };
    lock().insert(key, entry);
}

/// `kGo(agentId)` — the registry rows whose `agentId` matches, projected into
/// [`SkillRestoreCandidate`]s (each carrying its registry key) for
/// [`crate::restore_post_compact_skills`]. `agent_id = None` selects the
/// main-thread rows.
#[must_use]
pub fn filter_for_agent(agent_id: Option<&str>) -> Vec<SkillRestoreCandidate> {
    lock()
        .iter()
        .filter(|(_, e)| e.agent_id.as_deref() == agent_id)
        .map(|(key, e)| SkillRestoreCandidate {
            key: key.clone(),
            name: e.skill_name.clone(),
            path: e.skill_path.clone(),
            content: e.content.clone(),
            invoked_at_ms: e.invoked_at_ms,
        })
        .collect()
}

/// `n_n(key, content)` — overwrite an existing entry's content. A no-op when the
/// key is absent (`if(r)…`, exactly like the binary). `content == ""` clears the
/// stored skill body so a later compaction won't re-attach it.
pub fn write_back(key: &str, content: &str) {
    if let Some(e) = lock().get_mut(key) {
        e.content = content.to_string();
    }
}

/// Test-only: clear the whole registry. Exposed (not `cfg(test)`) so downstream
/// crates' tests can isolate their view of this process-global. Not part of the
/// binary surface.
#[doc(hidden)]
pub fn reset_for_test() {
    lock().clear();
}

/// Test-only: the stored content for `key`, or `None` if absent. Lets tests
/// assert the `n_n` write-back / clear behaviour of [`write_back`] and
/// [`crate::restore_post_compact_skills`].
#[doc(hidden)]
#[must_use]
pub fn content_for_test(key: &str) -> Option<String> {
    lock().get(key).map(|e| e.content.clone())
}

/// Test-only serial lock guarding the process-global registry so tests across
/// this crate (here and `post_compact`) that register/write-back real rows don't
/// race. Not part of the binary surface.
#[doc(hidden)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::post_compact::{
        restore_post_compact_skills, AttachedSkillContent, SKILL_TRUNCATION_MARKER,
    };

    // The registry is a process-global; serialize the tests that mutate it so
    // they don't race, and `reset_for_test()` at the top of each for a clean
    // view.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_for_test();
        g
    }

    #[test]
    fn register_sets_key_and_filter_by_agent() {
        let _g = guard();
        register("deploy", Path::new("/skills/deploy"), "deploy body", None);
        register(
            "build",
            Path::new("/skills/build"),
            "build body",
            Some("agent:x"),
        );

        // Main-thread filter (None) sees only the `:deploy` row.
        let main = filter_for_agent(None);
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].key, ":deploy");
        assert_eq!(main[0].name, "deploy");
        assert_eq!(main[0].content, "deploy body");
        assert_eq!(main[0].path, PathBuf::from("/skills/deploy"));

        // Agent filter sees only its own row.
        let agent = filter_for_agent(Some("agent:x"));
        assert_eq!(agent.len(), 1);
        assert_eq!(agent[0].key, "agent:x:build");
    }

    #[test]
    fn register_overwrites_same_key() {
        let _g = guard();
        register("s", Path::new("/s"), "first", None);
        register("s", Path::new("/s"), "second", None);
        let rows = filter_for_agent(None);
        assert_eq!(rows.len(), 1, "same key overwrites, not appends");
        assert_eq!(rows[0].content, "second");
    }

    #[test]
    fn write_back_overwrites_and_clears() {
        let _g = guard();
        register("s", Path::new("/s"), "body", None);
        write_back(":s", "truncated");
        assert_eq!(content_for_test(":s").as_deref(), Some("truncated"));
        // "" clears the content.
        write_back(":s", "");
        assert_eq!(content_for_test(":s").as_deref(), Some(""));
    }

    #[test]
    fn write_back_absent_key_is_noop() {
        let _g = guard();
        write_back(":missing", "x");
        assert!(content_for_test(":missing").is_none());
    }

    #[test]
    fn registry_key_uses_empty_prefix_for_main_thread() {
        assert_eq!(registry_key(None, "foo"), ":foo");
        assert_eq!(registry_key(Some("agent:a"), "foo"), "agent:a:foo");
    }

    // --- rRg write-back semantics (n_n) via restore_post_compact_skills ----- //

    #[test]
    fn restore_writes_back_truncated_content() {
        // A kept-and-truncated candidate persists the truncated content back to
        // the registry (`if(!c&&u!==a.content)n_n(s,u)`).
        let _g = guard();
        let big = "x".repeat(40_000); // caps to ~5_000 tokens
        register("trunc", Path::new("/t"), &big, None);
        let restored = restore_post_compact_skills(filter_for_agent(None), &[]);
        assert_eq!(restored.len(), 1);
        let stored = content_for_test(":trunc").expect("row present");
        assert!(
            stored.ends_with(SKILL_TRUNCATION_MARKER),
            "truncated content persisted to the registry"
        );
        assert_eq!(stored, restored[0].content);
        assert_ne!(stored, big, "the full body no longer occupies the row");
    }

    #[test]
    fn restore_does_not_write_back_short_content() {
        // Short content is returned verbatim (`u===a.content`) → no write-back.
        let _g = guard();
        register("short", Path::new("/s"), "tiny body", None);
        let restored = restore_post_compact_skills(filter_for_agent(None), &[]);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content, "tiny body");
        assert_eq!(content_for_test(":short").as_deref(), Some("tiny body"));
    }

    #[test]
    fn restore_body_match_counts_but_does_not_write_back() {
        // A body match (`c=true`) restores + counts toward the budget but never
        // writes back — the registry keeps the FULL (untruncated) body.
        let _g = guard();
        let big = "x".repeat(40_000);
        register("bm", Path::new("/b"), &big, None);
        let already = [AttachedSkillContent::Body(big.clone())];
        let restored = restore_post_compact_skills(filter_for_agent(None), &already);
        assert_eq!(restored.len(), 1);
        assert!(restored[0].content.ends_with(SKILL_TRUNCATION_MARKER));
        assert_eq!(
            content_for_test(":bm").as_deref(),
            Some(big.as_str()),
            "body match → no write-back; registry unchanged"
        );
    }

    #[test]
    fn restore_clears_registry_on_budget_overflow() {
        // Six equal ~5_000-token skills: exactly 5 fit (25_000) and 1 overflows;
        // the overflowed row's content is CLEARED (`n_n(s,"")`).
        let _g = guard();
        for i in 0..6 {
            register(&format!("s{i}"), Path::new("/s"), &"x".repeat(40_000), None);
        }
        let restored = restore_post_compact_skills(filter_for_agent(None), &[]);
        assert_eq!(restored.len(), 5, "5 fit, 6th overflows");
        let cleared: Vec<String> = (0..6)
            .map(|i| format!(":s{i}"))
            .filter(|k| content_for_test(k).as_deref() == Some(""))
            .collect();
        assert_eq!(cleared.len(), 1, "exactly the overflowed row is cleared");
    }

    #[test]
    fn restore_sorts_by_invoked_at_desc() {
        // DESC by invokedAt: the most recently invoked skill leads the result.
        let _g = guard();
        register("old", Path::new("/o"), "old body", None);
        std::thread::sleep(std::time::Duration::from_millis(3));
        register("new", Path::new("/n"), "new body", None);
        let restored = restore_post_compact_skills(filter_for_agent(None), &[]);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].name, "new", "most recent first");
        assert_eq!(restored[1].name, "old");
    }
}
