//! CLI platform composition for the shared plugin lifecycle implementation.
use std::path::Path;
use std::sync::Arc;
use telemetry::AnalyticsBus;

pub use configuration_admin::plugin_install::*;

pub async fn run_install_secure(
    arg: &str,
    scope: Option<&str>,
    yes: bool,
    config: &[String],
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    run_install_secure_with_bus(arg, scope, yes, config, plugins_dir, home, cwd, None).await
}

pub async fn run_install_secure_with_bus(
    arg: &str,
    scope: Option<&str>,
    yes: bool,
    config: &[String],
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
    analytics_bus: Option<Arc<AnalyticsBus>>,
) -> Result<String, String> {
    run_install_with_credential_factory(
        arg,
        scope,
        yes,
        config,
        plugins_dir,
        home,
        cwd,
        analytics_bus,
        || async {
            harness_runtime::desktop::build_shared_credential_stack(home, false)
                .await
                .map(|stack| stack.credentials)
                .map_err(|error| format!("Failed to initialize plugin credential storage: {error}"))
        },
    )
    .await
}

pub async fn run_uninstall_secure(
    arg: &str,
    scope: Option<&str>,
    keep_data: bool,
    prune: bool,
    yes: bool,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> Result<String, String> {
    run_uninstall_with_credential_factory(
        arg,
        scope,
        keep_data,
        prune,
        yes,
        plugins_dir,
        home,
        cwd,
        analytics_bus,
        || async {
            harness_runtime::desktop::build_shared_credential_stack(home, false)
                .await
                .map(|stack| stack.credentials)
                .map_err(|error| format!("Failed to initialize plugin credential storage: {error}"))
        },
    )
    .await
}
