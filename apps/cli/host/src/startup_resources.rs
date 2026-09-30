//! Pre-runtime materialisation for `--plugin-url` and `--file`.

use crate::argv::Argv;
use std::path::{Component, Path, PathBuf};

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
        materialize_files(&specs).await?;
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

use configuration_admin::plugin_download::{bounded_get, download_client, download_plugin_urls};

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

async fn materialize_files(specs: &[String]) -> Result<(), String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "--file requires Anthropic API-key authentication".to_string())?;
    let cwd = std::env::current_dir().map_err(|e| format!("resolve cwd: {e}"))?;
    // File downloads carry the Anthropic API key. Keep every redirect on the
    // original origin so a 3xx response cannot exfiltrate `x-api-key`.
    let client = download_client(false)?;
    let base = std::env::var("LINGXI_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
    let mut prepared = Vec::new();
    for spec in specs {
        let (file_id, relative) = spec
            .split_once(':')
            .ok_or_else(|| format!("invalid --file spec `{spec}`; expected file_id:path"))?;
        if file_id.is_empty()
            || !file_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("invalid --file id in `{spec}`"));
        }
        let relative = validate_relative_path(relative)?;
        ensure_no_symlink_components(&cwd, &relative)?;
        let target = cwd.join(&relative);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(format!(
                    "--file target already exists: {}",
                    target.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("inspect `{}`: {error}", target.display()));
            }
        }
        let url = format!("{}/v1/files/{file_id}/content", base.trim_end_matches('/'));
        let headers = vec![
            ("x-api-key".to_string(), api_key.clone()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ];
        let bytes = bounded_get(&client, &url, &headers).await?;
        prepared.push((relative, bytes));
    }

    let mut committed: Vec<PathBuf> = Vec::new();
    for (relative, bytes) in prepared {
        let options = platform_api::rooted_fs::AtomicWriteOptions {
            overwrite: false,
            create_parents: true,
            ..platform_api::rooted_fs::AtomicWriteOptions::default()
        };
        if let Err(error) = platform_api::rooted_fs::atomic_write(&cwd, &relative, &bytes, options)
        {
            for path in &committed {
                let _ = platform_api::rooted_fs::remove_file(&cwd, path);
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
    #[test]
    fn file_paths_are_confined() {
        assert!(validate_relative_path("docs/a.txt").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/tmp/secret").is_err());
        assert!(validate_relative_path("").is_err());
    }
}
