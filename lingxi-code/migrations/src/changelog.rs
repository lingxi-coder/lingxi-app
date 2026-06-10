//! `migrateChangelogFromConfig` (`releaseNotes.ts:55-76`) — move the
//! deprecated `cachedChangelog` config field to
//! `<claude-config-home>/cache/changelog.md`. Fire-and-forget at startup
//! (the caller `tokio::spawn`s this); errors are silent (TS `.catch(() => {})`),
//! retried next startup.

use crate::context::MigrationEnv;
use crate::global_config;
use serde_json::Value;

/// Run the async changelog migration.
pub async fn migrate_changelog_from_config(env: &MigrationEnv) {
    let Ok(cfg) = global_config::read_map(&env.global_config_path) else {
        return;
    };
    let Some(Value::String(changelog)) = cfg.get("cachedChangelog").cloned() else {
        return;
    };

    let cache_dir = env.claude_config_home.join("cache");
    let cache_path = cache_dir.join("changelog.md");
    if tokio::fs::create_dir_all(&cache_dir).await.is_ok() {
        // `wx` flag parity (`releaseNotes.ts:66`): write only if the file
        // doesn't exist; an existing file (or any write error) is silently fine.
        if let Ok(mut f) = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&cache_path)
            .await
        {
            use tokio::io::AsyncWriteExt;
            let _ = f.write_all(changelog.as_bytes()).await;
        }
    }

    // Remove the deprecated field regardless (TS does this after the try).
    let _ = global_config::save_map(&env.global_config_path, |mut m| {
        m.remove("cachedChangelog");
        m
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;

    fn test_env(t: &crate::test_support::TempConfig) -> crate::context::MigrationEnv {
        crate::context::MigrationEnv {
            global_config_path: t.global.clone(),
            claude_config_home: t.home.clone(),
            project_dir: t.project.clone(),
            ctx: crate::context::MigrationContext { first_party: true, subscription_type: None },
            bus: None,
        }
    }

    #[tokio::test]
    async fn moves_cached_changelog_to_file_and_drops_key() {
        let t = temp_config();
        std::fs::write(&t.global, r##"{"cachedChangelog": "# v1", "keep": 1}"##).unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        let cache = t.home.join("cache").join("changelog.md");
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "# v1");
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("cachedChangelog").is_none());
        assert_eq!(m["keep"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn existing_cache_file_is_not_overwritten_but_key_still_dropped() {
        let t = temp_config();
        std::fs::write(&t.global, r##"{"cachedChangelog": "# old"}"##).unwrap();
        let cache_dir = t.home.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("changelog.md"), "# newer").unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        assert_eq!(
            std::fs::read_to_string(cache_dir.join("changelog.md")).unwrap(),
            "# newer"
        );
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("cachedChangelog").is_none());
    }

    #[tokio::test]
    async fn no_key_is_noop() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"a": 1}"#).unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        assert!(!t.home.join("cache").join("changelog.md").exists());
    }
}
