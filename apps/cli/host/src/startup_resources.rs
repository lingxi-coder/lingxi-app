//! Pre-runtime plugin and exact-account provider file materialisation.

use crate::argv::Argv;
use llm_runtime::services::{sdk, ProviderServices};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Owns temporary resources for the full CLI session.
pub(crate) struct StartupResourceGuard {
    roots: Vec<PathBuf>,
}

impl Drop for StartupResourceGuard {
    fn drop(&mut self) {
        for root in &self.roots {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

pub(crate) async fn prepare(argv: &mut Argv) -> Result<StartupResourceGuard, String> {
    let mut guard = StartupResourceGuard { roots: Vec::new() };
    if !argv.plugin_url.is_empty() {
        let root = owner_only_temp_dir("lingxi-plugin-url")?;
        let paths = match download_plugin_urls(&argv.plugin_url, &root).await {
            Ok(paths) => paths,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&root);
                return Err(e);
            }
        };
        argv.plugin_dir.extend(paths);
        guard.roots.push(root);
    }
    if let Some(specs) = argv.file.clone() {
        materialize_files(argv, &specs).await?;
    }
    Ok(guard)
}

fn owner_only_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let root = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(|e| format!("create session resource directory: {e}"))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&root).map_err(|e| format!("create session resource directory: {e}"))?;
    Ok(root)
}

use configuration_admin::plugin_download::download_plugin_urls;

fn validate_relative_path(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if path.as_os_str().is_empty()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("invalid --file relative path `{raw}`"));
    }
    Ok(path)
}

fn ensure_no_symlink_components(root: &Path, relative: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("refusing symlink path `{}`", current.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("inspect `{}`: {e}", current.display())),
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartupFileSpec {
    resource: sdk::files::ProviderFileRef,
    relative_path: String,
}

fn parse_file_specs(specs: &[String]) -> Result<Vec<StartupFileSpec>, String> {
    if specs.is_empty() || specs.len() > 64 {
        return Err("--file requires between 1 and 64 account-scoped references".into());
    }
    specs.iter().map(|spec| {
        if spec.len() > 64 * 1024 { return Err("--file reference is too large".into()); }
        let spec: StartupFileSpec = serde_json::from_str(spec)
            .map_err(|_| "--file requires JSON {resource:<SDK ProviderFileRef>,relativePath}; bare file IDs are not supported")?;
        let scope = spec.resource.account_scope.as_deref()
            .ok_or("--file requires an exact managed account scope")?;
        let identity: llm_runtime::accounts::AccountResourceScope = serde_json::from_str(scope)
            .map_err(|_| "--file account scope is invalid")?;
        if identity.store_scope.is_empty() || identity.identity_generation == 0
            || spec.resource.profile_name != format!("account.{}", identity.account_id)
            || spec.resource.file_id.trim().is_empty() || spec.resource.file_id.chars().any(char::is_control) {
            return Err("--file reference has an incomplete or mismatched account identity".into());
        }
        validate_relative_path(&spec.relative_path)?;
        Ok(spec)
    }).collect()
}

async fn materialize_files(argv: &Argv, input: &[String]) -> Result<(), String> {
    // Parse and reject unscoped inputs before opening the native credential domain.
    let specs = parse_file_specs(input)?;
    let config = crate::init::resolve_desktop_config(argv, permission::PermissionMode::Default);
    let stack = harness_runtime::desktop::resolve_llm_stack(&config)
        .await
        .map_err(|_| "--file account authority is unavailable")?;
    let transport =
        http_client::provider_transport().map_err(|_| "--file transport is unavailable")?;
    let services = stack
        .llm_runtime
        .provider_services(stack.provider_region, Arc::new(transport))
        .map_err(|_| "--file account catalog is unavailable")?;
    let cwd = std::env::current_dir().map_err(|_| "--file working directory is unavailable")?;
    let prepared = download_files(&services, specs, &cwd).await?;
    install_files(&cwd, prepared)
}

async fn download_files(
    services: &ProviderServices,
    specs: Vec<StartupFileSpec>,
    cwd: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    // Check all destinations before loading a credential or downloading bytes.
    let mut destinations = Vec::new();
    for spec in &specs {
        let relative = validate_relative_path(&spec.relative_path)?;
        ensure_no_symlink_components(cwd, &relative)?;
        let target = cwd.join(&relative);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => return Err("--file target already exists".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("--file target is unavailable".into()),
        }
        if destinations.contains(&relative) {
            return Err("--file destinations must be unique".into());
        }
        destinations.push(relative);
    }
    let mut prepared = Vec::new();
    let mut total_bytes = 0usize;
    for (spec, relative) in specs.into_iter().zip(destinations) {
        let mut selected = services
            .resolve_file_account(&spec.resource)
            .await
            .map_err(|_| "--file account or resource binding is unavailable")?;
        let result = {
            let snapshot = selected.services.snapshot();
            let options = sdk::RequestOptions {
                credential: Some(selected.credential.clone()),
                account_scope: Some(selected.account_scope.clone()),
                total_timeout: Some(std::time::Duration::from_secs(120)),
                ..Default::default()
            };
            let files = sdk::files::FileService::from_snapshot(
                &snapshot,
                &selected.actual_profile,
                &options,
            )
            .map_err(|_| "--file account service binding is unavailable")?;
            files.download(&spec.resource).await
        };
        let content = match result {
            Ok(content) => {
                selected.succeeded().await;
                content
            }
            Err(error) => {
                selected.failed_sdk(&error).await;
                return Err("--file provider download failed".into());
            }
        };
        total_bytes = total_bytes
            .checked_add(content.bytes.len())
            .ok_or("--file downloads exceed the 64 MiB limit")?;
        if total_bytes > sdk::files::MAX_PROVIDER_FILE_DOWNLOAD_BYTES {
            return Err("--file downloads exceed the 64 MiB limit".into());
        }
        prepared.push((relative, content.bytes.to_vec()));
    }
    Ok(prepared)
}

fn install_files(cwd: &Path, prepared: Vec<(PathBuf, Vec<u8>)>) -> Result<(), String> {
    let mut committed: Vec<PathBuf> = Vec::new();
    for (relative, bytes) in prepared {
        let options = lingxi_core::host::rooted_fs::AtomicWriteOptions {
            overwrite: false,
            create_parents: true,
            ..lingxi_core::host::rooted_fs::AtomicWriteOptions::default()
        };
        if let Err(error) =
            lingxi_core::host::rooted_fs::atomic_write(&cwd, &relative, &bytes, options)
        {
            for path in &committed {
                let _ = lingxi_core::host::rooted_fs::remove_file(&cwd, path);
            }
            return Err(format!(
                "install `{}`: {error}",
                cwd.join(&relative).display()
            ));
        }
        committed.push(relative);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_services() -> (ProviderServices, sdk::files::ProviderFileRef) {
        use lingxi_core::host::provider_accounts::{
            AccountAuthKind, AccountMetadata, CredentialRef,
        };
        let account_id = "00000000-0000-4000-8000-000000000001";
        let mut profile = llm_runtime::builtin_presets()
            .providers
            .into_iter()
            .find(|profile| profile.profile_name == "openai")
            .unwrap();
        profile.profile_name = format!("account.{account_id}");
        profile.credential = llm_runtime::CredentialConfig::Static {
            id: "fixture".into(),
        };
        let account = AccountMetadata {
            account_id: account_id.into(),
            provider_id: "openai".into(),
            display_name: "Fixture".into(),
            auth_kind: AccountAuthKind::ApiKey,
            execution_binding_id: "openai".into(),
            credential_ref: CredentialRef {
                store_scope: "fixture".into(),
                secret_ref: format!("acct.{account_id}"),
            },
            enabled: true,
            user_priority: 0,
            config_revision: 1,
            identity_generation: 3,
        };
        struct Wire;
        #[async_trait::async_trait]
        impl sdk::Transport for Wire {
            async fn send(
                &self,
                request: sdk::HttpRequest,
            ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
                use futures::StreamExt;
                assert!(request.url.ends_with("/files/file-fixture/content"));
                assert!(request
                    .headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                        && value == "Bearer fixture-key"));
                Ok(sdk::StreamResponse {
                    status: 200,
                    headers: vec![],
                    body: futures::stream::once(async {
                        Ok(b"account-owned bytes".to_vec().into())
                    })
                    .boxed(),
                })
            }
        }
        let runtime = llm_runtime::ModelRuntime::from_config(llm_runtime::ClientConfig {
            providers: vec![profile],
        })
        .unwrap()
        .with_account_pool(Arc::new(llm_runtime::accounts::AccountPool::new(
            vec![account],
            Arc::new(llm_runtime::accounts::MemoryAccountPoolStore::default()),
        )))
        .with_credential_provider(Arc::new(llm_runtime::StaticCredentialProvider::new(
            llm_runtime::Credential::ApiKey("fixture-key".into()),
        )));
        let services = runtime
            .provider_services(llm_runtime::Region::International, Arc::new(Wire))
            .unwrap();
        let snapshot = services.snapshot();
        let profile = snapshot.profile(&format!("account.{account_id}")).unwrap();
        let resource = sdk::files::ProviderFileRef {
            provider_id: profile.provider_id.clone(),
            profile_name: profile.profile_name.clone(),
            endpoint_fingerprint: sdk::files::provider_file_endpoint_fingerprint(&profile.base_url),
            account_scope: Some(
                serde_json::to_string(&llm_runtime::accounts::AccountResourceScope {
                    store_scope: "fixture".into(),
                    account_id: account_id.into(),
                    identity_generation: 3,
                })
                .unwrap(),
            ),
            protocol: profile.protocol,
            file_id: "file-fixture".into(),
            uri: None,
            filename: None,
            media_type: None,
            size_bytes: None,
            expires_at: None,
            processing_status: None,
            downloadable: None,
            purpose: None,
        };
        (services, resource)
    }

    #[test]
    fn file_specs_require_returned_scoped_sdk_resources() {
        let (_, resource) = fixture_services();
        let spec = serde_json::json!({"resource":resource,"relativePath":"docs/a.txt"}).to_string();
        assert_eq!(
            parse_file_specs(&[spec]).unwrap()[0].relative_path,
            "docs/a.txt"
        );
        assert!(parse_file_specs(&["file-fixture:docs/a.txt".into()]).is_err());
        let mut unbound = resource;
        unbound.account_scope = None;
        assert!(parse_file_specs(&[
            serde_json::json!({"resource":unbound,"relativePath":"a.txt"}).to_string()
        ])
        .is_err());
    }

    #[tokio::test]
    async fn startup_file_download_uses_exact_account_and_stages_before_install() {
        let (services, resource) = fixture_services();
        let root = tempfile::tempdir().unwrap();
        let specs = parse_file_specs(&[
            serde_json::json!({"resource":resource,"relativePath":"docs/a.txt"}).to_string(),
        ])
        .unwrap();
        let staged = download_files(&services, specs, root.path()).await.unwrap();
        assert!(!root.path().join("docs/a.txt").exists());
        install_files(root.path(), staged).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("docs/a.txt")).unwrap(),
            b"account-owned bytes"
        );
    }

    #[test]
    fn file_install_failure_rolls_back_only_its_new_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("existing.txt"), b"preserve").unwrap();
        assert!(install_files(
            root.path(),
            vec![
                (PathBuf::from("new.txt"), b"new".to_vec()),
                (PathBuf::from("existing.txt"), b"replace".to_vec()),
            ]
        )
        .is_err());
        assert!(!root.path().join("new.txt").exists());
        assert_eq!(
            std::fs::read(root.path().join("existing.txt")).unwrap(),
            b"preserve"
        );
    }

    #[test]
    fn file_paths_are_confined() {
        assert!(validate_relative_path("docs/a.txt").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/tmp/secret").is_err());
        assert!(validate_relative_path("").is_err());
    }
}
