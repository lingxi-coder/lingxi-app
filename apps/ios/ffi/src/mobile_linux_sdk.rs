//! LingXi policy and resource composition for the standalone iOS Linux SDK.
//! Native execution and rootfs lifecycle are provided by mobile-linux-ios.

use async_trait::async_trait;
use mobile_linux_api::{
    LinuxCommandRequest, LinuxCommandResult, LinuxProcessHandle, MobileLinuxCapability,
    MobileLinuxError, MobileLinuxEvent, MobileLinuxRuntime, MobileLinuxRuntimeMode,
    MobileLinuxTaskSnapshot, MountPurpose, MountSpec, ProcessStreamSink, PtyOpenRequest,
    PtySessionHandle, PtySize, RawStdioOpenRequest, RawStdioReadResult, RawStdioSessionHandle,
    RootfsStatus, SandboxBackend,
};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const LINGXI_DOT_DIR: &str = ".lingxi";

/// Immutable iSH runtime identity and path configuration supplied by the iOS
/// framework bridge.
#[derive(Debug, Clone)]
pub struct IosIshRuntimeConfig {
    /// App-private directory that holds active/staged iSH rootfs state.
    pub managed_root: PathBuf,
    /// Canonical app sandbox root used to protect `.lingxi` and config trees.
    pub app_sandbox_root: PathBuf,
    /// Host workspace path exposed as the default `/workspace/<id>` mount.
    pub workspace_host_path: PathBuf,
    /// Stable guest workspace identifier appended under `/workspace`.
    pub stable_workspace_id: String,
    /// Guest ABI label surfaced in status responses.
    pub abi: String,
    /// Expected rootfs version label.
    pub rootfs_version: String,
    /// Optional archive digest surfaced in status responses.
    pub archive_sha256: Option<String>,
    /// Optional authorization file forwarded to the native bridge.
    pub authorization_file: Option<String>,
}

impl IosIshRuntimeConfig {
    fn lingxi_root(&self) -> PathBuf {
        self.app_sandbox_root.join(LINGXI_DOT_DIR)
    }
    fn workspace_guest_path(&self) -> String {
        mobile_linux_api::guest_paths::workspace(&self.stable_workspace_id)
    }

    #[cfg(test)]
    fn default_workspace_mount(&self) -> MountSpec {
        MountSpec {
            host_path: self.workspace_host_path.clone(),
            guest_path: self.workspace_guest_path(),
            read_only: false,
            purpose: MountPurpose::Workspace,
        }
    }
}

struct ProductIosRuntime {
    inner: Arc<dyn MobileLinuxRuntime>,
    config: IosIshRuntimeConfig,
}

impl ProductIosRuntime {
    fn validate_mounts(&self, mounts: &[MountSpec]) -> Result<(), MobileLinuxError> {
        for mount in mounts {
            validate_mount(mount, &self.config)?;
        }
        Ok(())
    }
}

fn existing_host_resource(override_path: Option<PathBuf>, fallback: PathBuf) -> Option<PathBuf> {
    override_path
        .filter(|path| !path.as_os_str().is_empty() && path.exists())
        .or_else(|| fallback.exists().then_some(fallback))
        .and_then(|path| {
            if path.is_absolute() {
                Some(path)
            } else {
                std::env::current_dir()
                    .ok()
                    .map(|current| current.join(path))
            }
        })
}

pub fn linked_runtime(
    config: IosIshRuntimeConfig,
) -> Result<Arc<dyn MobileLinuxRuntime>, MobileLinuxError> {
    let inner = mobile_linux_ios::linked_runtime(mobile_linux_ios::IosIshRuntimeConfig {
        managed_root: config.managed_root.clone(),
        app_sandbox_root: config.app_sandbox_root.clone(),
        workspace_host_path: config.workspace_host_path.clone(),
        stable_workspace_id: config.stable_workspace_id.clone(),
        abi: config.abi.clone(),
        rootfs_version: config.rootfs_version.clone(),
        archive_sha256: config.archive_sha256.clone(),
        authorization_file: config.authorization_file.clone(),
        rootfs_archive_path: existing_host_resource(
            std::env::var_os("LINGXI_IOS_ISH_ROOTFS_ZIP").map(PathBuf::from),
            config.managed_root.join("resources/alpine-rootfs.zip"),
        ),
        default_mount_path: existing_host_resource(
            std::env::var_os("LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR").map(PathBuf::from),
            config.managed_root.join("resources/default_mount"),
        ),
        rootfs_patch_path: std::env::var_os("LINGXI_IOS_ISH_ROOTFS_PATCH_DIR").map(PathBuf::from),
        protected_host_roots: vec![config.lingxi_root()],
        // Product policy below validates every requested mount before forwarding.
        // Preserve externally selected, sandbox-authorized workspace directories.
        allowed_mount_roots: vec![PathBuf::from("/")],
        allowed_guest_roots: vec!["/".into()],
    })?;
    Ok(Arc::new(ProductIosRuntime { inner, config }))
}

#[async_trait]
impl MobileLinuxRuntime for ProductIosRuntime {
    fn backend(&self) -> SandboxBackend {
        self.inner.backend()
    }

    fn mode(&self) -> MobileLinuxRuntimeMode {
        self.inner.mode()
    }

    async fn probe_capability(&self) -> MobileLinuxCapability {
        self.inner.probe_capability().await
    }

    async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.inner.boot().await
    }

    async fn shutdown(&self) -> Result<(), MobileLinuxError> {
        self.inner.shutdown().await
    }

    async fn run(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.run(request).await
    }

    async fn run_isolated(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.run_isolated(request).await
    }

    async fn run_streaming(
        &self,
        request: LinuxCommandRequest,
        sink: Arc<dyn ProcessStreamSink>,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.run_streaming(request, sink).await
    }

    async fn spawn_background(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxProcessHandle, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.spawn_background(request).await
    }

    async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
        self.inner.kill(handle).await
    }

    async fn open_pty(
        &self,
        request: PtyOpenRequest,
    ) -> Result<PtySessionHandle, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.open_pty(request).await
    }

    async fn write_pty(
        &self,
        handle: &PtySessionHandle,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxError> {
        self.inner.write_pty(handle, input).await
    }

    async fn resize_pty(
        &self,
        handle: &PtySessionHandle,
        size: PtySize,
    ) -> Result<(), MobileLinuxError> {
        self.inner.resize_pty(handle, size).await
    }

    async fn close_pty(&self, handle: &PtySessionHandle) -> Result<(), MobileLinuxError> {
        self.inner.close_pty(handle).await
    }

    async fn open_raw_stdio(
        &self,
        request: RawStdioOpenRequest,
    ) -> Result<RawStdioSessionHandle, MobileLinuxError> {
        self.validate_mounts(&request.mounts)?;
        self.inner.open_raw_stdio(request).await
    }

    async fn write_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxError> {
        self.inner.write_raw_stdio(handle, input).await
    }

    async fn read_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
        max_bytes: usize,
    ) -> Result<RawStdioReadResult, MobileLinuxError> {
        self.inner.read_raw_stdio(handle, max_bytes).await
    }

    async fn close_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
    ) -> Result<(), MobileLinuxError> {
        self.inner.close_raw_stdio(handle).await
    }

    async fn rootfs_status(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.inner.rootfs_status().await
    }

    async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.inner.verify_rootfs().await
    }

    async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.inner.repair_rootfs().await
    }

    async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.inner.reset_rootfs().await
    }

    async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
        self.validate_mounts(&mounts)?;
        self.inner.configure_mounts(mounts).await
    }

    fn current_mounts(&self) -> Vec<MountSpec> {
        self.inner.current_mounts()
    }

    async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<MobileLinuxEvent>, MobileLinuxError> {
        self.inner.read_events(after_sequence, limit).await
    }

    async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
        self.inner.list_tasks().await
    }

    async fn task_status(
        &self,
        task_id: &str,
    ) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError> {
        self.inner.task_status(task_id).await
    }
}

fn validate_mount(
    mount: &MountSpec,
    config: &IosIshRuntimeConfig,
) -> Result<MountSpec, MobileLinuxError> {
    let host_path = normalize_host_path(&mount.host_path, "host_path")?;
    validate_guest_path(&mount.guest_path, "guest_path", false)?;
    let managed_root = normalize_host_path(&config.managed_root, "managed_root")?;
    let app_root = normalize_host_path(&config.app_sandbox_root, "app_sandbox_root")?;
    let lingxi_root = normalize_host_path(&config.lingxi_root(), ".lingxi root")?;
    let workspace_root = normalize_host_path(&config.workspace_host_path, "workspace_host_path")?;

    if guest_path_has_prefix(&mount.guest_path, mobile_linux_api::guest_paths::HOME) {
        return Err(MobileLinuxError::InvalidRequest(
            "request mounts may not replace the runtime-managed persistent /root".to_string(),
        ));
    }

    if host_path == app_root
        || host_path == managed_root
        || host_path == lingxi_root
        || host_path.starts_with(&managed_root)
        || managed_root.starts_with(&host_path)
        || host_path.starts_with(&lingxi_root)
        || lingxi_root.starts_with(&host_path)
    {
        return Err(MobileLinuxError::InvalidRequest(
            "mount host_path may not target app root, managed_root, or .lingxi".to_string(),
        ));
    }

    if path_contains_protected_subtree(&host_path) {
        return Err(MobileLinuxError::InvalidRequest(
            "mount host_path may not target provider/config subtrees".to_string(),
        ));
    }

    if matches!(mount.purpose, MountPurpose::Workspace) {
        if mount.guest_path != config.workspace_guest_path() {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "workspace mount guest_path must be {}",
                config.workspace_guest_path()
            )));
        }
        if host_path != workspace_root {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "workspace mount host_path must match workspace_host_path {} (got {})",
                workspace_root.display(),
                host_path.display()
            )));
        }
    } else if mount.guest_path == config.workspace_guest_path() {
        return Err(MobileLinuxError::InvalidRequest(
            "only workspace mounts may target the managed workspace guest path".to_string(),
        ));
    }

    Ok(MountSpec {
        host_path,
        guest_path: mount.guest_path.clone(),
        read_only: mount.read_only,
        purpose: mount.purpose,
    })
}

fn guest_path_has_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_guest_path(value: &str, field: &str, allow_root: bool) -> Result<(), MobileLinuxError> {
    if !value.starts_with('/') {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} must be an absolute guest path"
        )));
    }
    if !allow_root && value == "/" {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} may not be the guest root"
        )));
    }
    for component in Path::new(value).components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "{field} may not contain path traversal"
            )));
        }
    }
    Ok(())
}

fn normalize_host_path(path: &Path, field: &str) -> Result<PathBuf, MobileLinuxError> {
    if !path.is_absolute() {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} must be absolute"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(MobileLinuxError::InvalidRequest(format!(
                    "{field} may not contain parent traversal"
                )))
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(MobileLinuxError::InvalidRequest(format!(
                        "{field} has no resolvable ancestor"
                    )));
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(MobileLinuxError::InvalidRequest(format!(
                        "{field} has no resolvable ancestor"
                    )));
                };
                existing = parent;
            }
            Err(error) => {
                return Err(MobileLinuxError::InvalidRequest(format!(
                    "{field} cannot be resolved safely: {error}"
                )))
            }
        }
    }
}

fn path_contains_protected_subtree(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    components
        .iter()
        .any(|component| matches!(component.as_str(), ".lingxi" | "provider" | "providers"))
        || components
            .windows(2)
            .any(|pair| pair[0] == "library" && pair[1] == "preferences")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_config(root: &Path) -> IosIshRuntimeConfig {
        IosIshRuntimeConfig {
            managed_root: root.join("mobile-linux"),
            app_sandbox_root: root.to_path_buf(),
            workspace_host_path: root.join("workspaces/default"),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
        }
    }

    #[test]
    fn host_resources_fall_back_to_the_configured_managed_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let fallback = root.join("custom-managed/resources/alpine-rootfs.zip");
        fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        fs::write(&fallback, b"archive").unwrap();
        for value in [None, Some(PathBuf::new()), Some(root.join("missing.zip"))] {
            assert_eq!(
                existing_host_resource(value, fallback.clone()),
                Some(fallback.clone())
            );
        }
        let explicit = root.join("explicit.zip");
        fs::write(&explicit, b"override").unwrap();
        assert_eq!(
            existing_host_resource(Some(explicit.clone()), fallback.clone()),
            Some(explicit)
        );
        fs::remove_file(&fallback).unwrap();
        assert_eq!(
            existing_host_resource(Some(root.join("missing.zip")), fallback),
            None
        );
    }

    #[test]
    fn workspace_mount_accepts_the_managed_workspace_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let mount = validate_mount(&config.default_workspace_mount(), &config).expect("mount");
        assert_eq!(
            mount.host_path,
            normalize_host_path(&config.workspace_host_path, "workspace_host_path")
                .expect("normalized workspace path")
        );
        assert_eq!(mount.guest_path, "/workspace/default");
    }

    #[test]
    fn workspace_mount_rejects_guest_path_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let error = validate_mount(
            &MountSpec {
                host_path: config.workspace_host_path.clone(),
                guest_path: "/workspace/../etc".to_string(),
                read_only: false,
                purpose: MountPurpose::Workspace,
            },
            &config,
        )
        .expect_err("mount should fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn request_mount_cannot_replace_persistent_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let host = root.join("external");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&host).expect("create external");

        let error = validate_mount(
            &MountSpec {
                host_path: host,
                guest_path: "/root".to_string(),
                read_only: false,
                purpose: MountPurpose::External,
            },
            &config,
        )
        .expect_err("persistent root override must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn mount_rejects_symlink_alias_into_protected_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let alias_root = root.join("aliases");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&alias_root).expect("create aliases");

        let alias = alias_root.join("managed-link");
        std::os::unix::fs::symlink(config.managed_root.clone(), &alias).expect("create symlink");

        let error = validate_mount(
            &MountSpec {
                host_path: alias.join("rootfs"),
                guest_path: "/tmp/managed".to_string(),
                read_only: true,
                purpose: MountPurpose::Temp,
            },
            &config,
        )
        .expect_err("mount should fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }
}
