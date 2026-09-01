//! Shared process-wrapper support for CLI self-spawns.
//!
//! Claude Code resolves `processWrapper` from env > managed > `--settings` >
//! user settings. Project and local files are intentionally excluded. The
//! resolved argv is frozen into an internal environment value before daemon,
//! background-worker, and agent self-spawns so every descendant sees one
//! immutable snapshot.

const RESOLVED_WRAPPER_ENV: &str = "LINGXI_RESOLVED_PROCESS_WRAPPER_JSON";

fn process_wrapper_from_environment() -> Option<String> {
    std::env::var("LINGXI_CODE_PROCESS_WRAPPER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CLAUDE_CODE_PROCESS_WRAPPER")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
}

fn parse_wrapper(raw: &str) -> Result<Vec<String>, String> {
    let tokens = shlex::split(raw)
        .ok_or_else(|| "processWrapper contains invalid shell quoting".to_string())?;
    if tokens.is_empty() {
        return Err("processWrapper must not be empty".to_string());
    }
    Ok(tokens)
}

fn parse_flag_settings(raw: Option<&str>) -> Option<lingxi_core::settings::SettingsJson> {
    let raw = raw?.trim();
    let text = if raw.starts_with('{') {
        raw.to_string()
    } else {
        std::fs::read_to_string(raw).ok()?
    };
    serde_json::from_str(&text).ok()
}

/// Resolve and freeze the process wrapper before any self-spawn.
pub(crate) async fn configure(
    flag_settings: Option<&str>,
    project_dir: &std::path::Path,
) -> Result<(), String> {
    let raw = if let Some(raw) = process_wrapper_from_environment() {
        Some(raw)
    } else {
        let managed_layers: Vec<lingxi_core::settings::SettingsJson> =
            engine_desktop::settings_watch::managed_settings_raw_tiers()
                .await
                .into_iter()
                .filter_map(|raw| serde_json::from_str(&raw).ok())
                .collect();
        let cli_layer = parse_flag_settings(flag_settings);
        let empty_env = std::collections::BTreeMap::new();
        lingxi_core::settings::Settings::load_with_layers_from_user_path(
            lingxi_core::settings::LoadInputs {
                env: &empty_env,
                project_dir,
                defaults: lingxi_core::settings::SettingsJson::default(),
            },
            lingxi_core::settings::FileLayerScope {
                include_user: true,
                include_project: false,
                include_local: false,
            },
            lingxi_core::settings::SupplementalLayers {
                cli_layer: cli_layer.as_ref(),
                managed_layers: &managed_layers,
            },
            Some(&crate::run::lingxi_home_dir().join("settings.json")),
        )
        .ok()
        .and_then(|effective| effective.settings.process_wrapper)
        .filter(|raw| !raw.trim().is_empty())
    };

    match raw {
        Some(raw) => {
            let tokens = parse_wrapper(&raw)?;
            let encoded = serde_json::to_string(&tokens)
                .map_err(|error| format!("failed to freeze processWrapper: {error}"))?;
            std::env::set_var(RESOLVED_WRAPPER_ENV, encoded);
        }
        None => std::env::remove_var(RESOLVED_WRAPPER_ENV),
    }
    Ok(())
}

/// Resolve the frozen wrapper, falling back to the public environment variables
/// for embedding hosts that do not call [`configure`].
#[must_use]
pub(crate) fn process_wrapper_tokens_from_env() -> Option<Vec<String>> {
    if let Ok(encoded) = std::env::var(RESOLVED_WRAPPER_ENV) {
        if let Ok(tokens) = serde_json::from_str::<Vec<String>>(&encoded) {
            if !tokens.is_empty() {
                return Some(tokens);
            }
        }
    }
    process_wrapper_from_environment().and_then(|raw| parse_wrapper(&raw).ok())
}

/// Prepend the configured process wrapper, if any, to an argv vector.
#[must_use]
pub(crate) fn wrap_argv(argv: Vec<String>) -> Vec<String> {
    if let Some(mut wrapper) = process_wrapper_tokens_from_env() {
        wrapper.extend(argv);
        wrapper
    } else {
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard<'a> {
        _lock: MutexGuard<'a, ()>,
    }

    impl EnvGuard<'_> {
        fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");
            std::env::remove_var("CLAUDE_CODE_PROCESS_WRAPPER");
            std::env::remove_var(RESOLVED_WRAPPER_ENV);
            Self { _lock: guard }
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");
            std::env::remove_var("CLAUDE_CODE_PROCESS_WRAPPER");
            std::env::remove_var(RESOLVED_WRAPPER_ENV);
        }
    }

    #[test]
    fn no_wrapper_leaves_argv_untouched() {
        let _g = EnvGuard::new();
        let argv = vec!["lingxi-cli".to_string(), "--resume".to_string()];
        assert_eq!(wrap_argv(argv.clone()), argv);
    }

    #[test]
    fn lingxi_wrapper_is_prepended() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "sandbox --net none");
        let argv = vec!["lingxi-cli".to_string(), "--resume".to_string()];
        assert_eq!(
            wrap_argv(argv),
            vec!["sandbox", "--net", "none", "lingxi-cli", "--resume"]
        );
    }

    #[test]
    fn lingxi_takes_precedence_over_claude() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "lingxi-wrap");
        std::env::set_var("CLAUDE_CODE_PROCESS_WRAPPER", "claude-wrap");
        assert_eq!(
            wrap_argv(vec!["exe".to_string()]),
            vec!["lingxi-wrap", "exe"]
        );
    }

    #[test]
    fn claude_wrapper_used_when_lingxi_absent() {
        let _g = EnvGuard::new();
        std::env::set_var("CLAUDE_CODE_PROCESS_WRAPPER", "claude-wrap --flag");
        assert_eq!(
            wrap_argv(vec!["exe".to_string()]),
            vec!["claude-wrap", "--flag", "exe"]
        );
    }

    #[test]
    fn blank_wrapper_is_ignored() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "   ");
        assert_eq!(process_wrapper_tokens_from_env(), None);
    }

    #[test]
    fn quoted_wrapper_path_is_one_argv_token() {
        let _g = EnvGuard::new();
        std::env::set_var(
            "LINGXI_CODE_PROCESS_WRAPPER",
            "'/Applications/My Wrapper/bin/wrap' --mode safe",
        );
        assert_eq!(
            process_wrapper_tokens_from_env(),
            Some(vec![
                "/Applications/My Wrapper/bin/wrap".to_string(),
                "--mode".to_string(),
                "safe".to_string(),
            ])
        );
    }

    #[test]
    fn malformed_quoting_is_rejected() {
        assert!(parse_wrapper("'unterminated").is_err());
    }
}
