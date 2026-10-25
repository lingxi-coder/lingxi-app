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

/// Immutable iSH runtime identity and path configuration supplied by the iOS
/// framework bridge.
#[derive(Debug, Clone)]
pub struct IosIshRuntimeConfig {
    /// App-private directory that holds active/staged iSH rootfs state.
    pub managed_root: PathBuf,
    /// Brand-owned Linux identity provided by the native client.
    pub brand_id: String,
    pub linux_user: String,
    pub config_dir_name: String,
    /// Canonical app sandbox root used to protect brand config and provider trees.
    pub app_sandbox_root: PathBuf,
    /// Host workspace path exposed as the default brand workspace mount.
    pub workspace_host_path: PathBuf,
    /// Stable guest workspace identifier appended under the selected brand workspace root.
    pub stable_workspace_id: String,
    /// Exact additional Project grants validated by the native Project store.
    /// These are compared as full host/guest/purpose records; no prefix grants
    /// are accepted by this runtime policy layer.
    pub project_mount_grants: Vec<MountSpec>,
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
    fn brand_profile(&self) -> mobile_linux_api::BrandRuntimeProfile {
        mobile_linux_api::BrandRuntimeProfile::new(
            self.brand_id.clone(),
            self.linux_user.clone(),
            self.config_dir_name.clone(),
        )
    }

    fn brand_config_home(&self) -> PathBuf {
        self.brand_profile()
            .persistent_home_host_path(&self.managed_root)
            .join(&self.config_dir_name)
    }
    fn workspace_guest_path(&self) -> String {
        mobile_linux_api::guest_paths::workspace_for_profile(
            &self.brand_profile(),
            &self.stable_workspace_id,
        )
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

    fn mounts_with_project_grants(&self, mounts: &[MountSpec]) -> Vec<MountSpec> {
        let mut merged = mounts.to_vec();
        for grant in &self.config.project_mount_grants {
            if !merged.iter().any(|mount| same_mount(mount, grant)) {
                merged.push(grant.clone());
            }
        }
        merged
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
    mut config: IosIshRuntimeConfig,
) -> Result<Arc<dyn MobileLinuxRuntime>, MobileLinuxError> {
    let brand_profile = config.brand_profile();
    brand_profile
        .validate()
        .map_err(|error| MobileLinuxError::InvalidRequest(error.to_string()))?;
    config.project_mount_grants = validate_project_mount_grants(&config)?;
    let inner = mobile_linux_ios::linked_runtime(mobile_linux_ios::IosIshRuntimeConfig {
        managed_root: config.managed_root.clone(),
        brand_profile,
        app_sandbox_root: config.app_sandbox_root.clone(),
        workspace_host_path: config.workspace_host_path.clone(),
        stable_workspace_id: config.stable_workspace_id.clone(),
        authorized_project_mounts: config.project_mount_grants.clone(),
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
        protected_host_roots: vec![config.brand_config_home()],
        // Product policy below validates every requested mount before forwarding.
        // Preserve externally selected, sandbox-authorized workspace directories.
        allowed_mount_roots: vec![PathBuf::from("/")],
        allowed_guest_roots: vec!["/".into()],
    })?;
    Ok(Arc::new(ProductIosRuntime { inner, config }))
}

fn validate_project_mount_grants(
    config: &IosIshRuntimeConfig,
) -> Result<Vec<MountSpec>, MobileLinuxError> {
    if config.project_mount_grants.len() > 64 {
        return Err(MobileLinuxError::InvalidRequest(
            "too many Project mount grants".into(),
        ));
    }
    let mut validated = Vec::<MountSpec>::with_capacity(config.project_mount_grants.len());
    for grant in &config.project_mount_grants {
        if !matches!(grant.purpose, MountPurpose::External) || grant.read_only {
            return Err(MobileLinuxError::InvalidRequest(
                "Project mount grants must be writable External mounts".into(),
            ));
        }
        let normalized = validate_mount(grant, config)?;
        if !same_mount(&normalized, grant) {
            return Err(MobileLinuxError::InvalidRequest(
                "Project mount grants must use canonical host paths".into(),
            ));
        }
        if validated.iter().any(|existing| {
            paths_overlap(&existing.host_path, &normalized.host_path)
                || guest_paths_overlap(&existing.guest_path, &normalized.guest_path)
        }) {
            return Err(MobileLinuxError::InvalidRequest(
                "Project mount grants may not duplicate or overlap".into(),
            ));
        }
        validated.push(normalized);
    }
    Ok(validated)
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
        let mounts = self.mounts_with_project_grants(&mounts);
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

fn same_mount(left: &MountSpec, right: &MountSpec) -> bool {
    left.host_path == right.host_path
        && left.guest_path == right.guest_path
        && left.read_only == right.read_only
        && left.purpose == right.purpose
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn guest_paths_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_mount(
    mount: &MountSpec,
    config: &IosIshRuntimeConfig,
) -> Result<MountSpec, MobileLinuxError> {
    let host_path = normalize_host_path(&mount.host_path, "host_path")?;
    validate_guest_path(&mount.guest_path, "guest_path", false)?;
    if !is_normalized_absolute_guest_path(&mount.guest_path) {
        return Err(MobileLinuxError::InvalidRequest(
            "guest mount path must be normalized".into(),
        ));
    }
    let managed_root = normalize_host_path(&config.managed_root, "managed_root")?;
    let app_root = normalize_host_path(&config.app_sandbox_root, "app_sandbox_root")?;
    let brand_config_home = normalize_host_path(&config.brand_config_home(), "brand config home")?;
    let workspace_root = normalize_host_path(&config.workspace_host_path, "workspace_host_path")?;
    let brand_profile = config.brand_profile();
    let guest_home = mobile_linux_api::guest_paths::home_for_profile(&brand_profile);
    let apps_guest = brand_profile
        .apps_guest_path()
        .to_string_lossy()
        .into_owned();
    let apps_host = brand_config_home.join(".apps");
    let mut authorized_project_mount = false;
    for grant in &config.project_mount_grants {
        let grant_host = normalize_host_path(&grant.host_path, "authorized project host")?;
        if grant_host == host_path
            && grant.host_path == mount.host_path
            && grant.guest_path == mount.guest_path
            && grant.read_only == mount.read_only
            && grant.purpose == mount.purpose
        {
            authorized_project_mount = true;
            break;
        }
    }
    let user_home_host = normalize_host_path(
        &brand_profile.persistent_home_host_path(&managed_root),
        "persistent user home",
    )?;
    let guest_home_path = Path::new(&guest_home);
    let guest_path = Path::new(&mount.guest_path);
    let project_relative = guest_path.strip_prefix(guest_home_path).ok();
    let exact_home_mapping = if authorized_project_mount {
        if let Some(relative) = project_relative {
            let expected_host = normalize_host_path(
                &user_home_host.join(relative),
                "project guest-to-host mapping",
            )?;
            expected_host == host_path && !relative.as_os_str().is_empty()
        } else {
            false
        }
    } else {
        false
    };
    if authorized_project_mount && !exact_home_mapping {
        return Err(MobileLinuxError::InvalidRequest(
            "Project mount grants must map inside the selected persistent user home".into(),
        ));
    }
    let authorized_app_source = exact_home_mapping
        && matches!(mount.purpose, MountPurpose::External)
        && mount.guest_path != apps_guest
        && guest_path_has_prefix(&mount.guest_path, &apps_guest)
        && host_path.starts_with(&apps_host)
        && host_path != apps_host;

    if ["/", "/home"].contains(&mount.guest_path.as_str())
        || (guest_path_has_prefix(&mount.guest_path, &guest_home)
            && !matches!(mount.purpose, MountPurpose::Workspace)
            && !exact_home_mapping)
        || guest_path_has_prefix(&mount.guest_path, mobile_linux_api::guest_paths::ADMIN_HOME)
    {
        return Err(MobileLinuxError::InvalidRequest(
            "request mounts may not replace the runtime-managed persistent guest homes".to_string(),
        ));
    }

    let persistent_workspace =
        mobile_linux_core::is_persistent_user_workspace(&managed_root, &brand_profile, mount);
    let exact_persistent_project_mount =
        exact_home_mapping && host_path.starts_with(&user_home_host) && host_path != user_home_host;
    let overlaps_brand_config = host_path == brand_config_home
        || host_path.starts_with(&brand_config_home)
        || brand_config_home.starts_with(&host_path);
    if host_path == app_root
        || host_path == managed_root
        || (host_path.starts_with(&managed_root)
            && !persistent_workspace
            && !exact_persistent_project_mount)
        || managed_root.starts_with(&host_path)
        || (overlaps_brand_config && !authorized_app_source)
    {
        return Err(MobileLinuxError::InvalidRequest(
            "mount host_path may not target app root, managed_root, or brand config home"
                .to_string(),
        ));
    }

    if path_contains_protected_subtree(&host_path, &config.config_dir_name)
        && !authorized_app_source
    {
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

fn is_normalized_absolute_guest_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && !path.ends_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
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
                )));
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
                )));
            }
        }
    }
}

fn path_contains_protected_subtree(path: &Path, config_dir_name: &str) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    components.iter().any(|component| {
        component == config_dir_name || matches!(component.as_str(), "provider" | "providers")
    }) || components
        .windows(2)
        .any(|pair| pair[0] == "library" && pair[1] == "preferences")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_config(root: &Path) -> IosIshRuntimeConfig {
        IosIshRuntimeConfig {
            managed_root: root.join("mobile-linux"),
            brand_id: "lingxi".to_string(),
            linux_user: "lingxi".to_string(),
            config_dir_name: ".lingxi".to_string(),
            app_sandbox_root: root.to_path_buf(),
            workspace_host_path: root.join("workspaces/default"),
            stable_workspace_id: "default".to_string(),
            project_mount_grants: Vec::new(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
        }
    }

    struct MountTableRuntime {
        inner: mobile_linux_api::UnavailableMobileLinuxRuntime,
        mounts: std::sync::Mutex<Vec<MountSpec>>,
        fail_next_configure: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl MobileLinuxRuntime for MountTableRuntime {
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
            self.inner.run(request).await
        }

        async fn spawn_background(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<LinuxProcessHandle, MobileLinuxError> {
            self.inner.spawn_background(request).await
        }

        async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
            self.inner.kill(handle).await
        }

        async fn open_pty(
            &self,
            request: PtyOpenRequest,
        ) -> Result<PtySessionHandle, MobileLinuxError> {
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
            if self
                .fail_next_configure
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                return Err(MobileLinuxError::Unavailable(
                    "test mount application failure".into(),
                ));
            }
            *self.mounts.lock().expect("mount table") = mounts;
            Ok(())
        }

        fn current_mounts(&self) -> Vec<MountSpec> {
            self.mounts.lock().expect("mount table").clone()
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
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");

        let mount = validate_mount(&config.default_workspace_mount(), &config).expect("mount");
        assert_eq!(
            mount.host_path,
            normalize_host_path(&config.workspace_host_path, "workspace_host_path")
                .expect("normalized workspace path")
        );
        assert_eq!(mount.guest_path, "/home/lingxi/lingxi/default");
    }

    #[test]
    fn second_brand_profile_controls_workspace_guest_and_persistent_host_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("atlas-app");
        let mut config = test_config(&root);
        config.brand_id = "atlas".to_string();
        config.linux_user = "atlas_guest".to_string();
        config.config_dir_name = ".atlas".to_string();
        config.workspace_host_path = config
            .brand_profile()
            .persistent_home_host_path(&config.managed_root)
            .join("atlas/projects");
        config.stable_workspace_id = "projects".to_string();
        fs::create_dir_all(&config.workspace_host_path).expect("create profile workspace");
        fs::create_dir_all(&config.managed_root).expect("create managed root");
        fs::create_dir_all(config.brand_config_home()).expect("create profile config root");

        let workspace = config.default_workspace_mount();
        assert_eq!(workspace.guest_path, "/home/atlas_guest/atlas/projects");
        assert_eq!(
            config
                .brand_profile()
                .persistent_home_host_path(&config.managed_root),
            config.managed_root.join("persistent/atlas_guest")
        );
        let mount = validate_mount(&workspace, &config).expect("profile-backed workspace mount");
        assert_eq!(
            mount.host_path,
            config.workspace_host_path.canonicalize().unwrap()
        );
    }

    #[test]
    fn workspace_mount_rejects_guest_path_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");

        let error = validate_mount(
            &MountSpec {
                host_path: config.workspace_host_path.clone(),
                guest_path: "/home/lingxi/lingxi/../etc".to_string(),
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
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");
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
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");
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

    /// Isolates the Project grant overlay with a fake runtime that has no
    /// baseline home mount. The sibling assertion proves this Project grant
    /// does not introduce another app source; it does not replace the iSH
    /// runtime's separate persistent-home policy.
    #[tokio::test]
    async fn exact_project_app_grant_reaches_current_mounts_after_configure_and_translates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let mut config = test_config(&root);
        let apps_guest = config.brand_profile().apps_guest_path();
        let apps_host = config.brand_config_home().join(".apps");
        let guest = apps_guest
            .join("authorized-app")
            .to_string_lossy()
            .into_owned();
        let host = apps_host.join("authorized-app");
        fs::create_dir_all(&config.workspace_host_path).expect("create workspace");
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");
        fs::create_dir_all(&host).expect("create authorized app source");

        let grant = MountSpec {
            host_path: host.canonicalize().expect("canonical app source"),
            guest_path: guest.clone(),
            read_only: false,
            purpose: MountPurpose::External,
        };
        let workspace = config.default_workspace_mount();
        config.project_mount_grants.push(grant.clone());
        let inner: Arc<dyn MobileLinuxRuntime> = Arc::new(MountTableRuntime {
            inner: mobile_linux_api::UnavailableMobileLinuxRuntime::unavailable(
                mobile_linux_api::SandboxBackend::IosIsh,
                mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
                "ios",
                "arm64",
                "test stub",
            ),
            mounts: std::sync::Mutex::new(Vec::new()),
            fail_next_configure: std::sync::atomic::AtomicBool::new(false),
        });
        let product = ProductIosRuntime { inner, config };

        product
            .configure_mounts(vec![workspace.clone()])
            .await
            .expect("Product configuration applies exact project grant");
        let mut overlapping_config = product.config.clone();
        let nested_host = host.join("nested");
        fs::create_dir_all(&nested_host).expect("create nested app source directory");
        overlapping_config.project_mount_grants.push(MountSpec {
            host_path: nested_host
                .canonicalize()
                .expect("canonical nested directory"),
            guest_path: format!("{}/nested", grant.guest_path),
            read_only: false,
            purpose: MountPurpose::External,
        });
        assert!(validate_project_mount_grants(&overlapping_config).is_err());

        let outside_host = root.join("outside-project");
        fs::create_dir_all(&outside_host).expect("create outside project path");
        let mut outside_config = product.config.clone();
        outside_config.project_mount_grants.push(MountSpec {
            host_path: outside_host.canonicalize().expect("canonical outside path"),
            guest_path: "/tmp/outside-project".into(),
            read_only: false,
            purpose: MountPurpose::External,
        });
        assert!(validate_project_mount_grants(&outside_config).is_err());

        let mut readonly_config = product.config.clone();
        readonly_config.project_mount_grants[0].read_only = true;
        assert!(validate_project_mount_grants(&readonly_config).is_err());

        let mut changed_capability = grant.clone();
        changed_capability.read_only = true;
        assert!(product.validate_mounts(&[changed_capability]).is_err());
        let mut changed_purpose = grant.clone();
        changed_purpose.purpose = MountPurpose::Shared;
        assert!(product.validate_mounts(&[changed_purpose]).is_err());
        let mounts = product.current_mounts();
        assert_eq!(mounts.len(), 2);
        assert!(mounts.contains(&workspace));
        assert!(mounts.contains(&grant));
        assert_eq!(
            mounts
                .iter()
                .filter(|mount| matches!(mount.purpose, MountPurpose::Workspace))
                .count(),
            1
        );
        let translated = mobile_linux_core::resolve_guest_path(
            &guest,
            &mounts,
            &mobile_linux_api::guest_paths::writable_roots(),
            false,
        )
        .expect("approved guest source translates")
        .expect("approved source is host-backed");
        assert_eq!(translated, grant.host_path);

        let sibling_guest = apps_guest.join("other-app").to_string_lossy().into_owned();
        // This fake mount table contains only explicit request mounts, so the
        // sibling is not introduced by the Project grant. The real iSH
        // runtime's pre-existing persistent-home mapping is tested separately.
        assert!(matches!(
            mobile_linux_core::resolve_guest_path(
                &sibling_guest,
                &mounts,
                &mobile_linux_api::guest_paths::writable_roots(),
                false,
            ),
            Err(mobile_linux_core::GuestPathError::NotHostBacked)
        ));

        let sibling_host = apps_host.join("other-app");
        fs::create_dir_all(&sibling_host).expect("create ungranted sibling");
        let ungranted = MountSpec {
            host_path: sibling_host,
            guest_path: sibling_guest,
            read_only: false,
            purpose: MountPurpose::External,
        };
        assert!(product.validate_mounts(&[ungranted]).is_err());

        #[cfg(unix)]
        {
            let alias = apps_host.join("source-alias");
            std::os::unix::fs::symlink(&host, &alias).expect("create project source alias");
            let aliased_grant = MountSpec {
                host_path: alias,
                guest_path: grant.guest_path.clone(),
                read_only: false,
                purpose: MountPurpose::External,
            };
            assert!(product.validate_mounts(&[aliased_grant]).is_err());
        }

        let private_host = product.config.brand_config_home().join("private");
        fs::create_dir_all(&private_host).expect("create private config path");
        let private = MountSpec {
            host_path: private_host,
            guest_path: product
                .config
                .brand_profile()
                .config_guest_path()
                .join("private")
                .to_string_lossy()
                .into_owned(),
            read_only: false,
            purpose: MountPurpose::External,
        };
        assert!(product.validate_mounts(&[private]).is_err());
    }

    #[tokio::test]
    async fn failed_project_grant_configuration_does_not_synthesize_current_mounts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let mut config = test_config(&root);
        let apps_guest = config.brand_profile().apps_guest_path();
        let apps_host = config.brand_config_home().join(".apps");
        let guest = apps_guest
            .join("authorized-app")
            .to_string_lossy()
            .into_owned();
        let host = apps_host.join("authorized-app");
        fs::create_dir_all(&config.workspace_host_path).expect("create workspace");
        fs::create_dir_all(config.brand_config_home()).expect("create brand config root");
        fs::create_dir_all(&host).expect("create authorized app source");
        config.project_mount_grants.push(MountSpec {
            host_path: host.canonicalize().expect("canonical app source"),
            guest_path: guest,
            read_only: false,
            purpose: MountPurpose::External,
        });

        let inner: Arc<dyn MobileLinuxRuntime> = Arc::new(MountTableRuntime {
            inner: mobile_linux_api::UnavailableMobileLinuxRuntime::unavailable(
                mobile_linux_api::SandboxBackend::IosIsh,
                mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
                "ios",
                "arm64",
                "test stub",
            ),
            mounts: std::sync::Mutex::new(Vec::new()),
            fail_next_configure: std::sync::atomic::AtomicBool::new(true),
        });
        let product = ProductIosRuntime { inner, config };
        let workspace = product.config.default_workspace_mount();

        let error = product
            .configure_mounts(vec![workspace])
            .await
            .expect_err("native mount application failure must propagate");
        assert!(matches!(error, MobileLinuxError::Unavailable(_)));
        assert!(product.current_mounts().is_empty());
    }
}
