//! Ephemeral, path-bound permission leases used by local-app build workflows.
//!
//! A lease is intentionally separate from the session-wide permission mode:
//! it grants only calls whose resolved paths stay inside one app workspace and
//! disappears when the workflow drops its guard.

use crate::command_path_containment::check_command_path_containment;
use crate::filesystem::{file_tool_kind, input_path_for_tool, FileToolKind, FsRoots};
use crate::path_constraints::check_path_constraints;
use crate::shell_command::{command_from_input, is_shell_tool};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLeaseInfo {
    pub app_id: String,
    pub root: PathBuf,
}

#[derive(Debug)]
struct ActiveLease {
    info: WorkspaceLeaseInfo,
}

/// Shared registry owned by an engine composition root.
#[derive(Debug, Default)]
pub struct WorkspacePermissionLeaseRegistry {
    next: AtomicU64,
    active: RwLock<HashMap<u64, ActiveLease>>,
}

pub struct WorkspacePermissionLease {
    token: u64,
    registry: Arc<WorkspacePermissionLeaseRegistry>,
}

impl WorkspacePermissionLease {
    #[must_use]
    pub fn token(&self) -> u64 {
        self.token
    }
}

impl WorkspacePermissionLeaseRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn begin(
        self: &Arc<Self>,
        app_id: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> WorkspacePermissionLease {
        self.begin_normalized(app_id.into(), root.into())
    }

    /// Begin a lease for a production local-app workspace.
    ///
    /// Unlike [`Self::begin`], which remains intentionally permissive for
    /// in-memory/unit-test roots, this entry point requires the canonical
    /// local-app layout (`apps/<app_id>/workspace` or the matching guest
    /// `local-app-<app_id>` mount). Callers that cannot prove that binding must
    /// fail closed instead of silently granting a lease over a generic cwd.
    pub fn begin_local_app(
        self: &Arc<Self>,
        app_id: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Result<WorkspacePermissionLease, String> {
        let app_id = app_id.into();
        let root = lexical_normalize(root.into());
        let canonical_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if !is_exact_local_app_root(&canonical_root, &app_id) {
            return Err(format!(
                "workspace lease root {} is not the canonical workspace for app {}",
                canonical_root.display(),
                app_id
            ));
        }
        Ok(self.begin_normalized(app_id, canonical_root))
    }

    fn begin_normalized(
        self: &Arc<Self>,
        app_id: String,
        root: PathBuf,
    ) -> WorkspacePermissionLease {
        let token = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        // Store the canonical root, not the caller's spelling. This makes the
        // lease boundary stable across relative paths and host symlinks.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        self.active
            .write()
            .expect("workspace lease registry poisoned")
            .insert(
                token,
                ActiveLease {
                    info: WorkspaceLeaseInfo { app_id, root },
                },
            );
        WorkspacePermissionLease {
            token,
            registry: Arc::clone(self),
        }
    }

    pub fn active(&self) -> Vec<WorkspaceLeaseInfo> {
        self.active
            .read()
            .expect("workspace lease registry poisoned")
            .values()
            .map(|lease| lease.info.clone())
            .collect()
    }

    /// Returns true only for a file/shell/local-app MCP call contained by an
    /// active lease. Explicit deny/ask rules are evaluated before this hook in
    /// `PermissionPolicy::authorize_inner`, so the lease cannot override them.
    // Kept private for focused unit tests that exercise the registry in
    // isolation. Production authorization must always use the token-aware
    // entry point below; exposing an unscoped helper would make it too easy
    // for a future caller to accidentally authorize against another active
    // app's lease.
    fn allows(&self, tool_name: &str, input: &serde_json::Value, roots: &FsRoots) -> bool {
        let active = self
            .active
            .read()
            .expect("workspace lease registry poisoned");
        (active.len() == 1)
            .then(|| active.values().next())
            .flatten()
            .is_some_and(|lease| allows_for_lease(&lease.info, tool_name, input, roots))
    }

    pub fn allows_for_token(
        &self,
        token: Option<u64>,
        tool_name: &str,
        input: &serde_json::Value,
        roots: &FsRoots,
    ) -> bool {
        let Some(token) = token else { return false };
        self.active
            .read()
            .expect("workspace lease registry poisoned")
            .get(&token)
            .is_some_and(|lease| allows_for_lease(&lease.info, tool_name, input, roots))
    }

    /// Returns true when a leased workflow is attempting to modify a
    /// host-owned file. This is a hard deny, not merely a failed lease match,
    /// so a later global `auto`/allow rule cannot re-enable the mutation.
    fn denies_host_owned(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        roots: &FsRoots,
    ) -> bool {
        let active = self
            .active
            .read()
            .expect("workspace lease registry poisoned");
        (active.len() == 1)
            .then(|| active.values().next())
            .flatten()
            .is_some_and(|lease| host_owned_for_lease(&lease.info, tool_name, input, roots))
    }

    pub fn denies_host_owned_for_token(
        &self,
        token: Option<u64>,
        tool_name: &str,
        input: &serde_json::Value,
        roots: &FsRoots,
    ) -> bool {
        let Some(token) = token else { return false };
        self.active
            .read()
            .expect("workspace lease registry poisoned")
            .get(&token)
            .is_some_and(|lease| host_owned_for_lease(&lease.info, tool_name, input, roots))
    }

    /// Host-owned metadata remains protected for every conversation rooted at
    /// a local-app workspace, even after the temporary build lease is dropped.
    /// The generated local settings file is itself an allow rule, so this check
    /// must not depend on a lease token or it would be self-modifiable after a
    /// build completes.
    pub fn denies_host_owned_for_workspace(
        tool_name: &str,
        input: &serde_json::Value,
        roots: &FsRoots,
    ) -> bool {
        let root = &roots.cwd;
        is_local_app_workspace_root(root) && host_owned_for_root(root, tool_name, input, roots)
    }

    /// Return true when a file operation resolves outside a local-app's
    /// canonical workspace. This is deliberately evaluated before generic
    /// allow rules: `Edit(./**)` is lexical and cannot prove that a symlink
    /// target remains inside the workspace.
    pub fn escapes_local_app_workspace(
        tool_name: &str,
        input: &serde_json::Value,
        roots: &FsRoots,
    ) -> bool {
        if !is_local_app_workspace_root(&roots.cwd) {
            return false;
        }
        match file_tool_kind(tool_name) {
            FileToolKind::Editor | FileToolKind::Reader => {
                let lease_roots = FsRoots {
                    cwd: roots.cwd.clone(),
                    home: roots.home.clone(),
                    lingxi_home: roots.lingxi_home.clone(),
                };
                let Some(raw) = input_path_for_tool(tool_name, input, &lease_roots) else {
                    return false;
                };
                let Some(app_id) = local_app_id(&roots.cwd) else {
                    return false;
                };
                let target = resolve_target(&roots.cwd, &app_id, raw.as_ref());
                !is_workspace_path(&roots.cwd, &target)
            }
            FileToolKind::NonFile if is_shell_tool(tool_name) => {
                let Some(command) = command_from_input(input) else {
                    return false;
                };
                let Some(app_id) = local_app_id(&roots.cwd) else {
                    return false;
                };
                let lease_roots = FsRoots {
                    cwd: roots.cwd.clone(),
                    home: roots.home.clone(),
                    lingxi_home: roots.lingxi_home.clone(),
                };
                let command = command_with_guest_workspace_aliases(command, &roots.cwd, &app_id);
                // These guards are intentionally checked even before the
                // generic Bash allow walk. A global `Bash(...)` exact allow
                // must not authorize `cat /etc/passwd`, `cd /tmp`, or a
                // redirect outside this local-app workspace.
                check_path_constraints(&command, &lease_roots, &[]).is_some()
                    || check_command_path_containment(&command, &lease_roots, &[]).is_some()
            }
            _ => false,
        }
    }
}

impl Drop for WorkspacePermissionLease {
    fn drop(&mut self) {
        self.registry
            .active
            .write()
            .expect("workspace lease registry poisoned")
            .remove(&self.token);
    }
}

fn allows_for_lease(
    info: &WorkspaceLeaseInfo,
    tool_name: &str,
    input: &serde_json::Value,
    roots: &FsRoots,
) -> bool {
    // A local-app workspace has the stable on-disk shape
    // `<profile>/apps/<app-id>/workspace`. If a caller supplies a different
    // `app_id` for that root, fail closed instead of granting an app-scoped MCP
    // operation for a sibling app. Test/in-memory roots may use a generic
    // directory name, so only enforce the check when the canonical layout is
    // unambiguously the local-app shape.
    if !workspace_root_matches_app_id(&info.root, &info.app_id) {
        return false;
    }
    if tool_name.starts_with("mcp__local_apps__") {
        let allowed = matches!(
            tool_name,
            "mcp__local_apps__build"
                | "mcp__local_apps__read_logs"
                | "mcp__local_apps__manage_runtime"
                | "mcp__local_apps__update_manifest"
                | "mcp__local_apps__query_data"
        );
        return allowed
            && input.get("app_id").and_then(serde_json::Value::as_str)
                == Some(info.app_id.as_str());
    }

    match file_tool_kind(tool_name) {
        FileToolKind::Editor | FileToolKind::Reader => {
            let lease_roots = FsRoots {
                cwd: info.root.clone(),
                home: roots.home.clone(),
                lingxi_home: roots.lingxi_home.clone(),
            };
            let Some(raw) = input_path_for_tool(tool_name, input, &lease_roots) else {
                return false;
            };
            let target = resolve_target(&info.root, &info.app_id, raw.as_ref());
            is_workspace_path(&info.root, &target)
                && (file_tool_kind(tool_name) == FileToolKind::Reader
                    || !is_host_owned_path(&info.root, &target))
        }
        FileToolKind::NonFile if is_shell_tool(tool_name) => {
            let Some(command) = command_from_input(input) else {
                return false;
            };
            if !workspace_shell_is_safe(command) {
                // In particular, keep npm/node/networking commands on the
                // normal Shell approval path. The lease is a filesystem
                // boundary, not a network or arbitrary-code capability.
                return false;
            }
            let lease_roots = FsRoots {
                cwd: info.root.clone(),
                home: roots.home.clone(),
                lingxi_home: roots.lingxi_home.clone(),
            };
            let command = command_with_guest_workspace_aliases(command, &info.root, &info.app_id);
            check_path_constraints(&command, &lease_roots, &[]).is_none()
                && check_command_path_containment(&command, &lease_roots, &[]).is_none()
        }
        _ => false,
    }
}

fn host_owned_for_lease(
    info: &WorkspaceLeaseInfo,
    tool_name: &str,
    input: &serde_json::Value,
    roots: &FsRoots,
) -> bool {
    if !workspace_root_matches_app_id(&info.root, &info.app_id) {
        return false;
    }
    host_owned_for_root(&info.root, tool_name, input, roots)
}

fn host_owned_for_root(
    root: &Path,
    tool_name: &str,
    input: &serde_json::Value,
    roots: &FsRoots,
) -> bool {
    let lease_roots = FsRoots {
        cwd: root.to_path_buf(),
        home: roots.home.clone(),
        lingxi_home: roots.lingxi_home.clone(),
    };
    let app_id = local_app_id(root);
    match file_tool_kind(tool_name) {
        // Reader access is intentionally not blocked; the lease only protects
        // host-owned files from model mutation.
        FileToolKind::Editor => input_path_for_tool(tool_name, input, &lease_roots)
            .map(|raw| match app_id.as_deref() {
                Some(app_id) => resolve_target(root, app_id, raw.as_ref()),
                None => resolve_target_from_root(root, raw.as_ref()),
            })
            .is_some_and(|target| is_host_owned_path(root, &target)),
        FileToolKind::NonFile if is_shell_tool(tool_name) => {
            let Some(command) = command_from_input(input) else {
                return false;
            };
            let command = app_id
                .as_deref()
                .map(|app_id| command_with_guest_workspace_aliases(command, root, app_id))
                .unwrap_or_else(|| command.to_string());
            let redirects = crate::path_constraints::write_redirect_targets(&command, &lease_roots);
            if redirects
                .iter()
                .map(Path::new)
                .any(|target| is_host_owned_path(root, target))
            {
                return true;
            }
            // Positional write commands (cp/mv/rm/touch/mkdir/rmdir/sed) do
            // not flow through the redirect extractor. A host-owned marker in
            // one of those commands is conservatively denied; read-only
            // commands remain eligible for ordinary lease handling.
            let write_command =
                crate::shell_command::split_command(&command)
                    .iter()
                    .any(|subcommand| {
                        let command_name = subcommand.split_whitespace().next().unwrap_or("");
                        matches!(
                            Path::new(command_name)
                                .file_name()
                                .and_then(|name| name.to_str()),
                            Some("cp" | "mv" | "rm" | "rmdir" | "mkdir" | "touch" | "sed")
                        )
                    });
            if !write_command {
                return false;
            }
            crate::shell_command::split_command(&command)
                .iter()
                .flat_map(|subcommand| subcommand.split_whitespace().skip(1))
                .map(|token| token.trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | ',')))
                .filter(|token| !token.starts_with('-'))
                .map(|token| match app_id.as_deref() {
                    Some(app_id) => resolve_target(root, app_id, token),
                    None => resolve_target_from_root(root, token),
                })
                .any(|target| is_host_owned_path(root, &target))
        }
        _ => false,
    }
}

fn is_local_app_workspace_root(root: &Path) -> bool {
    local_app_id(root).is_some()
}

fn local_app_id(root: &Path) -> Option<String> {
    if let Some(name) = root.file_name().and_then(|name| name.to_str()) {
        if let Some(app_id) = name.strip_prefix("local-app-") {
            return (!app_id.is_empty()).then(|| app_id.to_owned());
        }
    }
    let app_root = root.parent()?;
    let apps_root = app_root.parent()?;
    if root.file_name().and_then(|name| name.to_str()) != Some("workspace")
        || apps_root.file_name().and_then(|name| name.to_str()) != Some("apps")
    {
        return None;
    }
    app_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn resolve_target_from_root(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        lexical_normalize(path.to_path_buf())
    } else {
        lexical_normalize(root.join(path))
    }
}

fn workspace_root_matches_app_id(root: &Path, app_id: &str) -> bool {
    let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if let Some(mounted_id) = name.strip_prefix("local-app-") {
        return mounted_id == app_id;
    }
    if name != "workspace" {
        return true;
    }
    let Some(app_root) = root.parent() else {
        return true;
    };
    let Some(apps_root) = app_root.parent() else {
        return true;
    };
    if apps_root.file_name().and_then(|name| name.to_str()) != Some("apps") {
        return true;
    }
    app_root.file_name().and_then(|name| name.to_str()) == Some(app_id)
}

fn is_exact_local_app_root(root: &Path, app_id: &str) -> bool {
    let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == format!("local-app-{app_id}") {
        return true;
    }
    if name != "workspace" {
        return false;
    }
    let Some(app_root) = root.parent() else {
        return false;
    };
    let Some(apps_root) = app_root.parent() else {
        return false;
    };
    apps_root.file_name().and_then(|name| name.to_str()) == Some("apps")
        && app_root.file_name().and_then(|name| name.to_str()) == Some(app_id)
}

fn workspace_shell_is_safe(command: &str) -> bool {
    // These commands are limited to local inspection and ordinary workspace
    // file operations. Interpreters, package managers, VCS network commands,
    // and unknown binaries remain subject to the existing Shell policy.
    const SAFE: &[&str] = &[
        "cat", "cd", "cp", "cut", "diff", "echo", "false", "find", "grep", "head", "ls", "mkdir",
        "mv", "printf", "pwd", "rm", "rmdir", "rg", "sed", "sort", "tail", "test", "touch", "tr",
        "true", "wc",
    ];
    // A command whose expansion is evaluated by the shell is not statically
    // provable from the command's argv. In particular, `echo $(curl ...)`
    // would otherwise look like the harmless `echo` allowlisted command while
    // still executing an arbitrary network subprocess. Keep all expansion and
    // process-substitution forms on the normal Shell approval path.
    if command.contains("$(")
        || command.contains('`')
        || command.contains("<(")
        || command.contains(">(")
        || command.contains("$'")
        || command.contains("$\"")
    {
        return false;
    }
    let forbidden_tokens = [
        "-exec",
        "-execdir",
        "-ok",
        "-okdir",
        "-delete",
        "xargs",
        "curl",
        "wget",
        "sh",
        "bash",
        "zsh",
        "python",
        "python3",
        "node",
        "perl",
        "ruby",
        "php",
        "osascript",
        "powershell",
        "pwsh",
    ];
    let commands = crate::shell_command::split_command(command);
    !commands.is_empty()
        && commands.iter().all(|subcommand| {
            let tokens: Vec<&str> = subcommand.split_whitespace().collect();
            let mut index = 0;
            while index < tokens.len()
                && tokens[index].contains('=')
                && !tokens[index].starts_with('-')
            {
                index += 1;
            }
            let token = tokens.get(index).copied().unwrap_or_default();
            if token.contains('/') || token.contains('\\') {
                return false;
            }
            let basename = Path::new(token)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(token);
            SAFE.contains(&basename)
                && !tokens.iter().skip(index + 1).any(|token| {
                    let normalized = token.trim_matches(|ch| matches!(ch, '\'' | '"' | ';' | ','));
                    forbidden_tokens.contains(&normalized)
                })
        })
}

fn resolve_target(root: &Path, app_id: &str, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        if let Some(relative) = guest_workspace_relative(app_id, path) {
            lexical_normalize(root.join(relative))
        } else {
            lexical_normalize(path.to_path_buf())
        }
    } else {
        lexical_normalize(root.join(path))
    }
}

fn guest_workspace_relative<'a>(app_id: &str, path: &'a Path) -> Option<&'a Path> {
    [
        format!("/workspace/local-app-{app_id}"),
        format!("/workspace/{app_id}"),
    ]
    .iter()
    .find_map(|prefix| {
        let prefix = Path::new(prefix);
        if path == prefix {
            Some(Path::new(""))
        } else {
            path.strip_prefix(prefix).ok()
        }
    })
}

fn command_with_guest_workspace_aliases(command: &str, root: &Path, app_id: &str) -> String {
    // Static command containment works in host coordinates. Mobile Linux
    // prompts use `/workspace/<id>` (and older local-app builds used
    // `/workspace/local-app-<id>`), so translate only those exact app-bound
    // aliases before checking cwd, redirects, and positional paths. If the
    // host root contains whitespace, leave the command untouched rather than
    // introducing shell quoting into a security check; it will conservatively
    // remain on the normal approval path.
    let root = root.to_string_lossy();
    if root.chars().any(char::is_whitespace) {
        return command.to_string();
    }
    command
        .replace(&format!("/workspace/local-app-{app_id}"), root.as_ref())
        .replace(&format!("/workspace/{app_id}"), root.as_ref())
}

fn is_workspace_path(root: &Path, target: &Path) -> bool {
    // Canonicalize the deepest existing ancestor and append the non-existing
    // tail. This catches both existing symlink escapes and writes through a
    // symlink whose destination file has not been created yet. A dangling
    // symlink itself is rejected conservatively because its destination cannot
    // be proven to stay inside the lease. Do this before a lexical
    // `starts_with` check: macOS/iOS may spell the same root as `/var` or
    // `/private/var`, and both spellings should resolve to the canonical lease.
    let Some(resolved) = resolve_canonical_target(target) else {
        return false;
    };
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !resolved.starts_with(canonical_root) {
        return false;
    }
    true
}

fn is_host_owned_path(root: &Path, target: &Path) -> bool {
    let Some(resolved) = resolve_canonical_target(target) else {
        return false;
    };
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    resolved.starts_with(&canonical_root)
        && host_owned_relative(resolved.strip_prefix(&canonical_root).unwrap_or(&resolved))
}

fn host_owned_relative(relative: &Path) -> bool {
    if relative
        .components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".lingxi"))
    {
        return true;
    }
    relative.file_name().is_some_and(|name| name == "LINGXI.md")
        || relative == Path::new("lib/lingxi-bridge.js")
}

fn resolve_canonical_target(target: &Path) -> Option<PathBuf> {
    let mut cursor = target.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&cursor) {
            return Some(
                tail.iter()
                    .rev()
                    .fold(canonical, |path, part| path.join(part)),
            );
        }
        if std::fs::symlink_metadata(&cursor).is_ok() {
            return None;
        }
        let name = cursor.file_name()?.to_os_string();
        tail.push(name);
        if !cursor.pop() {
            return None;
        }
    }
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn roots(root: &Path) -> FsRoots {
        FsRoots {
            cwd: root.to_path_buf(),
            home: None,
            lingxi_home: root.to_path_buf(),
        }
    }

    #[test]
    fn lease_allows_workspace_file_and_rejects_outside() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        assert!(registry.allows(
            "Write",
            &serde_json::json!({"file_path":"src/App.jsx"}),
            &fs
        ));
        assert!(!registry.allows("Write", &serde_json::json!({"file_path":"../secret"}), &fs));
        assert!(!registry.allows(
            "Write",
            &serde_json::json!({"file_path":".lingxi/source-policy.json"}),
            &fs
        ));
        assert!(!registry.allows(
            "Write",
            &serde_json::json!({"file_path":"lib/lingxi-bridge.js"}),
            &fs
        ));
        assert!(!registry.allows("Write", &serde_json::json!({"file_path":"LINGXI.md"}), &fs));
    }

    #[test]
    fn host_owned_metadata_is_readable_but_never_writable() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(root.join(".lingxi")).unwrap();
        std::fs::write(root.join(".lingxi/settings.local.json"), b"{}\n").unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        let settings = serde_json::json!({"file_path":".lingxi/settings.local.json"});

        assert!(registry.allows("Read", &settings, &fs));
        assert!(!registry.allows("Write", &settings, &fs));
        assert!(registry.denies_host_owned("Write", &settings, &fs));
        assert!(!registry.denies_host_owned("Read", &settings, &fs));
        assert!(registry.denies_host_owned(
            "Bash",
            &serde_json::json!({"command":"echo x > .lingxi/settings.local.json"}),
            &fs
        ));
        assert!(registry.denies_host_owned(
            "Bash",
            &serde_json::json!({"command":"rm -rf .lingxi"}),
            &fs
        ));
    }

    #[test]
    fn drop_revokes_lease() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let fs = roots(&root);
        let lease = registry.begin("app", root);
        assert!(!registry.active().is_empty());
        drop(lease);
        assert!(registry.active().is_empty());
        assert!(!registry.allows(
            "Write",
            &serde_json::json!({"file_path":"src/App.jsx"}),
            &fs
        ));
    }

    #[test]
    fn shell_lease_preserves_network_and_interpreter_approval() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        assert!(registry.allows(
            "Bash",
            &serde_json::json!({"command":"echo ok > out.txt"}),
            &fs
        ));
        assert!(!registry.allows("Bash", &serde_json::json!({"command":"npm install"}), &fs));
        assert!(!registry.allows(
            "Bash",
            &serde_json::json!({"command":"curl https://example.com"}),
            &fs
        ));
        assert!(!registry.allows(
            "Bash",
            &serde_json::json!({"command":"node -e 'process.exit(0)'"}),
            &fs
        ));
        assert!(!registry.allows(
            "Bash",
            &serde_json::json!({"command":"echo $(curl https://example.com)"}),
            &fs
        ));
        assert!(!registry.allows(
            "Bash",
            &serde_json::json!({"command":"echo `curl https://example.com`"}),
            &fs
        ));
    }

    #[test]
    fn production_lease_translates_only_the_bound_guest_workspace_alias() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("apps/app-a/workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let lease = registry.begin_local_app("app-a", &root).unwrap();
        let fs = roots(&root);

        assert!(registry.allows_for_token(
            Some(lease.token()),
            "Write",
            &serde_json::json!({"file_path":"/workspace/local-app-app-a/src/App.jsx"}),
            &fs
        ));
        assert!(registry.allows_for_token(
            Some(lease.token()),
            "Bash",
            &serde_json::json!({
                "command":"echo ok > /workspace/local-app-app-a/src/out.txt"
            }),
            &fs
        ));
        assert!(!registry.allows_for_token(
            Some(lease.token()),
            "Write",
            &serde_json::json!({"file_path":"/workspace/local-app-app-b/src/App.jsx"}),
            &fs
        ));
    }

    #[test]
    fn local_app_shell_path_escape_is_hard_denied_before_global_allow_rules() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("apps/app-a/workspace");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let fs = roots(&root);

        assert!(
            !WorkspacePermissionLeaseRegistry::escapes_local_app_workspace(
                "Bash",
                &serde_json::json!({"command":"cat src/App.jsx"}),
                &fs
            )
        );
        assert!(
            WorkspacePermissionLeaseRegistry::escapes_local_app_workspace(
                "Bash",
                &serde_json::json!({"command":"cat /etc/passwd"}),
                &fs
            )
        );
        assert!(
            WorkspacePermissionLeaseRegistry::escapes_local_app_workspace(
                "Bash",
                &serde_json::json!({"command":"cd /tmp && cat src/App.jsx"}),
                &fs
            )
        );
    }

    #[test]
    fn local_app_mcp_allowlist_is_app_scoped() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app-a", root.clone());
        let fs = roots(&root);
        assert!(registry.allows(
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-a"}),
            &fs
        ));
        assert!(!registry.allows(
            "mcp__local_apps__delete_app",
            &serde_json::json!({"app_id":"app-a"}),
            &fs
        ));
        assert!(!registry.allows(
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-b"}),
            &fs
        ));
    }

    #[test]
    fn local_app_layout_binds_mcp_lease_to_workspace_app_id() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("apps/app-a/workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app-b", root.clone());
        let fs = roots(&root);
        assert!(!registry.allows(
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-b"}),
            &fs
        ));
        assert!(!registry.allows(
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-a"}),
            &fs
        ));
    }

    #[test]
    fn guest_mount_root_binds_lease_to_exact_app_id() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("local-app-app-a");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let lease = registry.begin("app-b", root.clone());
        let fs = roots(&root);
        assert!(!registry.allows_for_token(
            Some(lease.token()),
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-b"}),
            &fs
        ));
        drop(lease);
        let lease = registry.begin("app-a", root.clone());
        assert!(registry.allows_for_token(
            Some(lease.token()),
            "mcp__local_apps__build",
            &serde_json::json!({"app_id":"app-a"}),
            &fs
        ));
    }

    #[test]
    fn production_local_app_lease_rejects_generic_or_mismatched_roots() {
        let dir = tempdir().unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let generic = dir.path().join("workspace");
        std::fs::create_dir_all(&generic).unwrap();
        assert!(registry.begin_local_app("app-a", generic).is_err());

        let wrong = dir.path().join("apps/app-b/workspace");
        std::fs::create_dir_all(&wrong).unwrap();
        assert!(registry.begin_local_app("app-a", wrong).is_err());
    }

    #[test]
    fn production_local_app_lease_accepts_canonical_host_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("apps/app-a/workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let lease = registry.begin_local_app("app-a", &root).unwrap();
        assert_eq!(
            registry.active(),
            vec![WorkspaceLeaseInfo {
                app_id: "app-a".into(),
                root: std::fs::canonicalize(root).unwrap(),
            }]
        );
        drop(lease);
        assert!(registry.active().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn local_app_settings_and_symlink_escape_stay_denied_without_a_lease() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("apps/app-a/workspace");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join(".lingxi")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join(".lingxi/settings.local.json"), b"{}\n").unwrap();
        symlink(&outside, root.join("link")).unwrap();
        let fs = roots(&root);

        assert!(
            WorkspacePermissionLeaseRegistry::denies_host_owned_for_workspace(
                "Edit",
                &serde_json::json!({"file_path":"/workspace/local-app-app-a/.lingxi/settings.local.json"}),
                &fs,
            )
        );
        assert!(
            WorkspacePermissionLeaseRegistry::escapes_local_app_workspace(
                "Edit",
                &serde_json::json!({"file_path":"/workspace/local-app-app-a/link/new.txt"}),
                &fs,
            )
        );
        assert!(
            !WorkspacePermissionLeaseRegistry::escapes_local_app_workspace(
                "Edit",
                &serde_json::json!({"file_path":"/workspace/local-app-app-a/src/new.txt"}),
                &fs,
            )
        );
    }

    #[test]
    fn concurrent_leases_require_the_matching_token() {
        let dir = tempdir().unwrap();
        let root_a = dir.path().join("workspace-a");
        let root_b = dir.path().join("workspace-b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let lease_a = registry.begin("app-a", root_a.clone());
        let lease_b = registry.begin("app-b", root_b.clone());
        let fs_a = roots(&root_a);
        assert!(registry.allows_for_token(
            Some(lease_a.token()),
            "Write",
            &serde_json::json!({"file_path":"src/a.txt"}),
            &fs_a
        ));
        assert!(!registry.allows_for_token(
            Some(lease_b.token()),
            "Write",
            &serde_json::json!({"file_path":root_a.join("src/a.txt").to_string_lossy()}),
            &fs_a
        ));
    }

    #[test]
    fn shell_lease_allows_cd_but_rejects_nested_actions() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        assert!(registry.allows_for_token(
            Some(lease.token()),
            "Bash",
            &serde_json::json!({"command":"cd . && grep -rn foo src/"}),
            &fs
        ));
        assert!(!registry.allows_for_token(
            Some(lease.token()),
            "Bash",
            &serde_json::json!({"command":"find . -exec curl https://example.com {} +"}),
            &fs
        ));
        assert!(!registry.allows_for_token(
            Some(lease.token()),
            "Bash",
            &serde_json::json!({"command":"sed -n 'e curl https://example.com' README.md"}),
            &fs
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_even_when_destination_is_missing() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("link")).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        assert!(!registry.allows(
            "Write",
            &serde_json::json!({"file_path":"link/new.txt"}),
            &fs
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_var_alias_is_compared_after_canonicalization() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let registry = WorkspacePermissionLeaseRegistry::new();
        let _lease = registry.begin("app", root.clone());
        let fs = roots(&root);
        let canonical = std::fs::canonicalize(&root).unwrap();
        let Some(raw) = canonical
            .to_str()
            .and_then(|path| path.strip_prefix("/private"))
        else {
            return;
        };
        assert!(registry.allows(
            "Write",
            &serde_json::json!({"file_path": format!("{raw}/alias.txt")}),
            &fs
        ));
    }
}
