//! Pre-runtime materialisation for `--plugin-url` and `--file`.

use crate::argv::Argv;
use futures::StreamExt;
use std::path::{Component, Path, PathBuf};

const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn download_client(allow_cross_origin_redirects: bool) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" && !cfg!(test) {
                attempt.error("refusing redirect to a non-HTTPS URL")
            } else if !allow_cross_origin_redirects
                && attempt
                    .previous()
                    .first()
                    .is_some_and(|initial| !same_origin(initial, attempt.url()))
            {
                // reqwest strips Authorization/Cookie on cross-origin redirects,
                // but `x-api-key` is an application header and would otherwise
                // be forwarded to the redirect target.
                attempt.error("credentialed download refuses a cross-origin redirect")
            } else {
                attempt.follow()
            }
        }))
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| format!("initialise download client: {e}"))
}

async fn bounded_get(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Vec<u8>, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL `{url}`: {e}"))?;
    if parsed.scheme() != "https" && !cfg!(test) {
        return Err(format!("refusing non-HTTPS URL `{url}`"));
    }
    let mut request = client.get(parsed);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("download `{url}`: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "download `{url}` returned HTTP {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err(format!("download `{url}` exceeds 64 MiB"));
    }
    let mut out = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download `{url}`: {e}"))?;
        if out.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            return Err(format!("download `{url}` exceeds 64 MiB"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

async fn download_plugin_urls(urls: &[String], root: &Path) -> Result<Vec<PathBuf>, String> {
    let client = download_client(true)?;
    let mut paths = Vec::with_capacity(urls.len());
    for (index, url) in urls.iter().enumerate() {
        let bytes = bounded_get(&client, url, &[]).await?;
        let extracted = root.join(format!("plugin-{index}"));
        std::fs::create_dir(&extracted)
            .map_err(|e| format!("create plugin extraction directory: {e}"))?;
        plugin::unpack_plugin_archive(&bytes, &extracted)
            .map_err(|e| format!("invalid plugin archive `{url}`: {e}"))?;
        let plugin_root = unwrap_plugin_root(&extracted)?;
        plugin::ensure_plugin_manifest(&plugin_root)
            .map_err(|e| format!("invalid plugin manifest `{url}`: {e}"))?;
        paths.push(plugin_root);
    }
    Ok(paths)
}

fn has_plugin_manifest(root: &Path) -> bool {
    root.join(branding::PLUGIN_MANIFEST_DIR).is_dir() || root.join("manifest.json").is_file()
}

fn unwrap_plugin_root(root: &Path) -> Result<PathBuf, String> {
    if has_plugin_manifest(root) {
        return Ok(root.to_path_buf());
    }
    let mut entries = std::fs::read_dir(root)
        .map_err(|e| format!("inspect extracted plugin `{}`: {e}", root.display()))?;
    let Some(entry) = entries
        .next()
        .transpose()
        .map_err(|e| format!("inspect extracted plugin `{}`: {e}", root.display()))?
    else {
        return Ok(root.to_path_buf());
    };
    if entries.next().is_none()
        && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
        && has_plugin_manifest(&entry.path())
    {
        Ok(entry.path())
    } else {
        Ok(root.to_path_buf())
    }
}

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
        let options = traits::rooted_fs::AtomicWriteOptions {
            overwrite: false,
            create_parents: true,
            ..traits::rooted_fs::AtomicWriteOptions::default()
        };
        if let Err(error) = traits::rooted_fs::atomic_write(&cwd, &relative, &bytes, options) {
            for path in &committed {
                let _ = traits::rooted_fs::remove_file(&cwd, path);
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

    #[tokio::test]
    async fn credentialed_download_rejects_cross_origin_redirect_without_leaking_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let redirect_target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = redirect_target.local_addr().unwrap();
        let redirect_source = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_addr = redirect_source.local_addr().unwrap();
        let source_task = tokio::spawn(async move {
            let (mut stream, _) = redirect_source.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let client = download_client(false).unwrap();
        let result = bounded_get(
            &client,
            &format!("http://{source_addr}/file"),
            &[("x-api-key".to_string(), "must-not-leak".to_string())],
        )
        .await;
        source_task.await.unwrap();
        let error = result.expect_err("cross-origin redirect must fail");
        assert!(
            error.contains("redirect"),
            "expected redirect failure, got: {error}"
        );
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                redirect_target.accept()
            )
            .await
            .is_err(),
            "redirect target must not receive the credentialed request"
        );
    }

    #[test]
    fn session_plugin_normalizes_root_mcpb_manifest() {
        let extracted = tempfile::tempdir().unwrap();
        let bundle = extracted.path().join("bundle");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::write(
            bundle.join("manifest.json"),
            r#"{"name":"session-plugin","version":"1.2.3"}"#,
        )
        .unwrap();

        let root = unwrap_plugin_root(extracted.path()).unwrap();
        assert_eq!(root, bundle);
        plugin::ensure_plugin_manifest(&root).unwrap();
        let normalized =
            std::fs::read_to_string(root.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json"))
                .unwrap();
        assert!(normalized.contains("session-plugin"));
    }
}
