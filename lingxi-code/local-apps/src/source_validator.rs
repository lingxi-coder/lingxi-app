//! Native validation for agent-generated Next.js workspace source.
//!
//! The validator is deliberately dependency-free at runtime: it walks the
//! workspace with Rust filesystem APIs, rejects symlinks, hashes locked
//! scaffold files, and scans text source for capabilities the local-app model
//! does not permit.

use crate::error::AppError;
use crate::manifest::AppLayout;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

const MAX_SOURCE_FILES: usize = 5_000;
const MAX_SOURCE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const WRITABLE_ROOTS: &[&str] = &["app", "components", "lib", "styles", "public"];

/// Exact immutable scaffold files expected beside generated source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSourcePolicy {
    /// Root-relative file -> lowercase SHA-256. This must include
    /// `package.json` and one supported npm lockfile.
    pub locked_files: BTreeMap<PathBuf, String>,
}

impl WorkspaceSourcePolicy {
    /// Validate the policy itself before it is used as a trust anchor.
    pub fn validate(&self) -> Result<(), AppError> {
        if !self.locked_files.contains_key(Path::new("package.json")) {
            return Err(AppError::InvalidRequest(
                "source policy must lock package.json".into(),
            ));
        }
        if !["package-lock.json", "npm-shrinkwrap.json"]
            .iter()
            .any(|name| self.locked_files.contains_key(Path::new(name)))
        {
            return Err(AppError::InvalidRequest(
                "source policy must lock package-lock.json or npm-shrinkwrap.json".into(),
            ));
        }
        for (relative, digest) in &self.locked_files {
            validate_relative_file(relative)?;
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(AppError::InvalidRequest(format!(
                    "locked file {} has an invalid SHA-256",
                    relative.display()
                )));
            }
        }
        Ok(())
    }
}

/// Validate the complete workspace before the fixed Next build is allowed.
pub fn validate_workspace_source(
    layout: &AppLayout,
    policy: &WorkspaceSourcePolicy,
) -> Result<(), AppError> {
    policy.validate()?;
    let workspace = layout.root().join(layout.workspace_rel());
    let metadata = std::fs::symlink_metadata(&workspace).map_err(|error| {
        AppError::Io(format!(
            "inspect workspace {}: {error}",
            workspace.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::StorageCorrupt(
            "workspace is not a real directory".into(),
        ));
    }

    let mut pending = vec![workspace.clone()];
    let mut files = 0usize;
    let mut total_bytes = 0u64;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|error| {
            AppError::Io(format!(
                "inspect workspace {}: {error}",
                directory.display()
            ))
        })? {
            let entry =
                entry.map_err(|error| AppError::Io(format!("inspect workspace entry: {error}")))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&workspace)
                .map_err(|_| AppError::InvalidRequest("workspace entry escaped its root".into()))?;
            let kind = entry
                .file_type()
                .map_err(|error| AppError::Io(format!("inspect {}: {error}", path.display())))?;
            if relative == Path::new(".git") {
                if kind.is_symlink() || !kind.is_dir() {
                    return Err(AppError::InvalidRequest(
                        "workspace .git must be a real directory".into(),
                    ));
                }
                continue;
            }
            if kind.is_symlink() {
                return Err(AppError::InvalidRequest(format!(
                    "workspace symlink is forbidden: {}",
                    relative.display()
                )));
            }
            if kind.is_dir() {
                validate_directory(relative, policy)?;
                pending.push(path);
                continue;
            }
            if !kind.is_file() {
                return Err(AppError::InvalidRequest(format!(
                    "unsupported workspace entry: {}",
                    relative.display()
                )));
            }
            files += 1;
            if files > MAX_SOURCE_FILES {
                return Err(AppError::InvalidRequest(format!(
                    "workspace has more than {MAX_SOURCE_FILES} files"
                )));
            }
            let length = entry
                .metadata()
                .map_err(|error| AppError::Io(format!("inspect {}: {error}", path.display())))?
                .len();
            if length > MAX_SOURCE_FILE_BYTES {
                return Err(AppError::InvalidRequest(format!(
                    "source file {} exceeds {MAX_SOURCE_FILE_BYTES} bytes",
                    relative.display()
                )));
            }
            total_bytes = total_bytes.saturating_add(length);
            if total_bytes > MAX_TOTAL_SOURCE_BYTES {
                return Err(AppError::InvalidRequest(format!(
                    "workspace exceeds {MAX_TOTAL_SOURCE_BYTES} bytes"
                )));
            }
            validate_file(relative, &path, policy)?;
        }
    }

    for (relative, expected) in &policy.locked_files {
        let path = workspace.join(relative);
        let actual = hash_file(&path)?;
        if &actual != expected {
            return Err(AppError::InvalidRequest(format!(
                "locked scaffold file {} changed (dependency/configuration drift)",
                relative.display()
            )));
        }
    }
    Ok(())
}

fn validate_directory(relative: &Path, policy: &WorkspaceSourcePolicy) -> Result<(), AppError> {
    let Some(root) = relative.components().next() else {
        return Ok(());
    };
    let Component::Normal(root) = root else {
        return Err(AppError::InvalidRequest(format!(
            "invalid workspace path {}",
            relative.display()
        )));
    };
    if root == ".lingxi" && relative.components().count() == 1 {
        return Ok(());
    }
    if WRITABLE_ROOTS.iter().any(|allowed| root == *allowed) {
        return Ok(());
    }
    if policy
        .locked_files
        .keys()
        .any(|locked| locked.starts_with(relative))
    {
        return Ok(());
    }
    Err(AppError::InvalidRequest(format!(
        "generated source directory {} is outside app/components/lib/styles/public",
        relative.display()
    )))
}

fn validate_file(
    relative: &Path,
    absolute: &Path,
    policy: &WorkspaceSourcePolicy,
) -> Result<(), AppError> {
    validate_relative_file(relative)?;
    let mut components = relative.components();
    let root = components.next().and_then(|component| match component {
        Component::Normal(value) => Some(value),
        _ => None,
    });
    let file_name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_lingxi_metadata = root == Some(std::ffi::OsStr::new(".lingxi"))
        && relative.components().count() == 2
        && matches!(
            file_name.as_str(),
            "app.json" | "app.manifest.json" | "design-spec.json"
        );
    let allowed = is_lingxi_metadata
        || root.is_some_and(|root| WRITABLE_ROOTS.iter().any(|allowed| root == *allowed))
        || policy.locked_files.contains_key(relative);
    if !allowed {
        return Err(AppError::InvalidRequest(format!(
            "generated file {} is outside app/components/lib/styles/public",
            relative.display()
        )));
    }

    let slash_path = relative.to_string_lossy().replace('\\', "/");
    if slash_path.starts_with("app/api/")
        || matches!(
            file_name.as_str(),
            "route.js" | "route.jsx" | "route.ts" | "route.tsx"
        )
    {
        return Err(AppError::InvalidRequest(format!(
            "API Routes/route handlers are forbidden: {slash_path}"
        )));
    }
    if policy.locked_files.contains_key(relative) || is_lingxi_metadata || !is_text_source(relative)
    {
        return Ok(());
    }
    let bytes = std::fs::read(absolute)
        .map_err(|error| AppError::Io(format!("read {}: {error}", absolute.display())))?;
    let source = std::str::from_utf8(&bytes).map_err(|_| {
        AppError::InvalidRequest(format!("text source {slash_path} is not valid UTF-8"))
    })?;
    scan_forbidden_source(&slash_path, source)
}

fn scan_forbidden_source(relative: &str, source: &str) -> Result<(), AppError> {
    let compact: String = source
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let lower = compact.to_ascii_lowercase();
    // Case-sensitive by design: ordinary JavaScript declarations use the
    // lowercase `function` keyword and are allowed. The uppercase global
    // `Function(...)` constructor (including `new Function(...)`) is not.
    if compact.contains("Function(") {
        return Err(AppError::InvalidRequest(format!(
            "forbidden Function constructor in {relative}"
        )));
    }
    let forbidden = [
        ("eval(", "eval"),
        ("\"useserver\"", "Server Actions"),
        ("'useserver'", "Server Actions"),
        ("fetch(", "direct fetch"),
        ("xmlhttprequest", "XMLHttpRequest"),
        ("websocket(", "WebSocket"),
        ("eventsource(", "EventSource"),
        ("npmrun", "npm invocation"),
        ("npminstall", "npm invocation"),
        ("npx", "npx invocation"),
        ("pnpm", "pnpm invocation"),
        ("yarn", "yarn invocation"),
        ("corepack", "corepack invocation"),
        ("apkadd", "apk invocation"),
    ];
    for (needle, capability) in forbidden {
        if lower.contains(needle) {
            return Err(AppError::InvalidRequest(format!(
                "forbidden {capability} in {relative}"
            )));
        }
    }
    if lower.contains("<script")
        && (lower.contains("src=\"http://")
            || lower.contains("src='http://")
            || lower.contains("src=\"https://")
            || lower.contains("src='https://")
            || lower.contains("src=\"//")
            || lower.contains("src='//"))
    {
        return Err(AppError::InvalidRequest(format!(
            "external script source is forbidden in {relative}"
        )));
    }
    Ok(())
}

fn is_text_source(relative: &Path) -> bool {
    relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "jsx"
                    | "ts"
                    | "tsx"
                    | "mjs"
                    | "cjs"
                    | "html"
                    | "css"
                    | "scss"
                    | "json"
                    | "md"
                    | "mdx"
            )
        })
}

fn validate_relative_file(relative: &Path) -> Result<(), AppError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::InvalidRequest(format!(
            "invalid workspace-relative file {}",
            relative.display()
        )));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, AppError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        AppError::InvalidRequest(format!("locked file {}: {error}", path.display()))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::InvalidRequest(format!(
            "locked file {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_SOURCE_FILE_BYTES {
        return Err(AppError::InvalidRequest(format!(
            "locked file {} exceeds size limit",
            path.display()
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| AppError::Io(format!("read locked file {}: {error}", path.display())))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, AppLayout, WorkspaceSourcePolicy) {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "app-test").unwrap();
        layout.initialize().unwrap();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(workspace.join("package.json"), "package").unwrap();
        fs::write(workspace.join("package-lock.json"), "lock").unwrap();
        fs::create_dir(workspace.join("app")).unwrap();
        fs::write(
            workspace.join("app/page.tsx"),
            "export default function Page() { return <main /> }",
        )
        .unwrap();
        let policy = WorkspaceSourcePolicy {
            locked_files: BTreeMap::from([
                (
                    PathBuf::from("package.json"),
                    format!("{:x}", Sha256::digest(b"package")),
                ),
                (
                    PathBuf::from("package-lock.json"),
                    format!("{:x}", Sha256::digest(b"lock")),
                ),
            ]),
        };
        (root, layout, policy)
    }

    #[test]
    fn accepts_locked_scaffold_and_whitelisted_source() {
        let (_root, layout, policy) = fixture();
        validate_workspace_source(&layout, &policy).unwrap();
    }

    #[test]
    fn rejects_dependency_drift_and_forbidden_runtime_capabilities() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(workspace.join("package.json"), "changed").unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        fs::write(workspace.join("package.json"), "package").unwrap();
        fs::write(
            workspace.join("app/page.tsx"),
            "fetch('https://example.com')",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }

    #[test]
    fn allows_normal_function_declarations_but_rejects_function_constructor() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(
            workspace.join("app/page.tsx"),
            "export default function Page() { function helper() { return 1 } return <main /> }",
        )
        .unwrap();
        validate_workspace_source(&layout, &policy).unwrap();
        fs::write(
            workspace.join("app/page.tsx"),
            "export const build = Function('return 1')",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }

    #[test]
    fn rejects_api_routes_and_files_outside_writable_roots() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::create_dir_all(workspace.join("app/api/test")).unwrap();
        fs::write(
            workspace.join("app/api/test/route.ts"),
            "export const GET = () => 1",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        fs::remove_dir_all(workspace.join("app/api")).unwrap();
        fs::write(workspace.join("escape.ts"), "export {}").unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        symlink("/tmp", workspace.join("public")).unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }
}
