//! Native validation for agent-generated Next.js workspace source.
//!
//! The validator walks the workspace with Rust filesystem APIs, rejects
//! symlinks, hashes locked scaffold files, and parses executable JavaScript
//! before a fixed Next build is allowed.

use crate::error::AppError;
use crate::manifest::AppLayout;
use crate::permissions::AppCapability;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const MAX_SOURCE_FILES: usize = 5_000;
const MAX_SOURCE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
/// Root directories a generated workspace may write under. Shared with the
/// `engine-mobile` LLM-write screen so the two never drift apart.
pub const WRITABLE_ROOTS: &[&str] = &["app", "components", "lib", "styles", "public"];

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
    validate_workspace_source_with(layout, policy, None)
}

/// The shared walk. `declared` present additionally screens every file for
/// bridge calls the plan never declared.
fn validate_workspace_source_with(
    layout: &AppLayout,
    policy: &WorkspaceSourcePolicy,
    declared: Option<&[AppCapability]>,
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
            validate_file(relative, &path, policy, declared)?;
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

/// Bridge calls that require a declared capability, and the capability each
/// one needs.
///
/// Both spellings per capability: the `lib/lingxi-bridge.js` helper the prompt
/// steers apps toward, and the direct `window.lingxi.v1` form it documents.
/// The validator matches these against parsed identifiers/member expressions,
/// so comments and ordinary strings do not request capabilities.
const CAPABILITY_CALLS: &[(&str, AppCapability)] = &[
    ("capturephoto(", AppCapability::Camera),
    (".device.capturephoto", AppCapability::Camera),
    ("pickimage(", AppCapability::PhotoLibrary),
    (".device.pickimage", AppCapability::PhotoLibrary),
    ("startrecording(", AppCapability::Microphone),
    (".device.recordaudiostart", AppCapability::Microphone),
    ("transcribespeech(", AppCapability::Microphone),
    (".device.transcribespeech", AppCapability::Microphone),
    ("getcurrentlocation(", AppCapability::Location),
    (".device.getlocation", AppCapability::Location),
    ("postnotification(", AppCapability::Notifications),
    (".device.postnotification", AppCapability::Notifications),
    ("requestllmchat(", AppCapability::Llm),
    (".llm.chat(", AppCapability::Llm),
    ("postagentevent(", AppCapability::AgentNotify),
    (".agent.post(", AppCapability::AgentNotify),
    ("mutatecollection(", AppCapability::DataMutation),
    (".data.mutate(", AppCapability::DataMutation),
];

/// Reject generated source that calls a capability the confirmed plan never
/// declared.
///
/// Without this the failure lands on the USER: generation, screening, build
/// and preview approval all pass, and the first tap returns
/// `capability_not_declared` — at which point the plan is frozen (`revise`
/// cannot add a capability, and nothing reopens the designer), so the app is
/// unrecoverable and the only recourse is building a new one. Here it is
/// just another validator rejection, fed back to the model with two repair
/// attempts left.
///
/// The model cannot fix it by DECLARING — the plan the user approved is the
/// contract — so the message tells it to drop the call instead.
///
/// Runs through [`validate_workspace_source`]'s own traversal rather than a
/// second one, so it inherits the symlink, `.git` and size guards instead of
/// re-deriving (and eventually diverging from) them.
pub fn validate_declared_capabilities(
    layout: &AppLayout,
    policy: &WorkspaceSourcePolicy,
    declared: &[AppCapability],
) -> Result<(), AppError> {
    validate_workspace_source_with(layout, policy, Some(declared))
}

fn validate_file(
    relative: &Path,
    absolute: &Path,
    policy: &WorkspaceSourcePolicy,
    declared: Option<&[AppCapability]>,
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
    let extension = relative
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !policy.locked_files.contains_key(relative)
        && matches!(extension.as_str(), "ts" | "tsx" | "cjs" | "mdx")
    {
        return Err(AppError::InvalidRequest(format!(
            "unsupported executable source extension in {slash_path}; use .js, .jsx, or .mjs"
        )));
    }
    if !policy.locked_files.contains_key(relative)
        && matches!(extension.as_str(), "html" | "htm" | "xhtml" | "svg" | "xml")
    {
        return Err(AppError::InvalidRequest(format!(
            "generated HTML/SVG/XML is forbidden in {slash_path}; use App Router JSX or a non-executable asset"
        )));
    }
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
    if root == Some(std::ffi::OsStr::new("app"))
        && matches!(
            file_name.as_str(),
            "manifest.js"
                | "manifest.jsx"
                | "robots.js"
                | "robots.jsx"
                | "sitemap.js"
                | "sitemap.jsx"
                | "opengraph-image.js"
                | "opengraph-image.jsx"
                | "twitter-image.js"
                | "twitter-image.jsx"
                | "icon.js"
                | "icon.jsx"
                | "apple-icon.js"
                | "apple-icon.jsx"
        )
    {
        return Err(AppError::InvalidRequest(format!(
            "dynamic Next metadata routes are forbidden: {slash_path}; use inspected static assets"
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
    match extension.as_str() {
        "js" | "jsx" | "mjs" => validate_javascript_source(relative, source, declared)?,
        "css" | "scss" => validate_style_source(&slash_path, source)?,
        "json" | "webmanifest" => validate_json_source(&slash_path, source)?,
        _ => {}
    }
    Ok(())
}

const ALLOWED_CLIENT_IMPORTS: &[&str] = &[
    "react",
    "react/jsx-runtime",
    "react-dom",
    "react-dom/client",
    "next/image",
    "next/link",
    "next/navigation",
];

const NODE_MODULES: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "crypto",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

fn parse_javascript(relative: &Path, source: &str) -> Result<tree_sitter::Tree, AppError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .map_err(|error| {
            AppError::InvalidRequest(format!("initialize JavaScript parser: {error}"))
        })?;
    let tree = parser.parse(source, None).ok_or_else(|| {
        AppError::InvalidRequest(format!(
            "JavaScript parser returned no syntax tree for {}",
            relative.display()
        ))
    })?;
    if tree.root_node().has_error() {
        return Err(AppError::InvalidRequest(format!(
            "invalid JavaScript syntax in {}",
            relative.display()
        )));
    }
    Ok(tree)
}

fn validate_javascript_source(
    relative: &Path,
    source: &str,
    declared: Option<&[AppCapability]>,
) -> Result<(), AppError> {
    let tree = parse_javascript(relative, source)?;
    let root = tree.root_node();
    let string_bindings = collect_const_string_bindings(root, source.as_bytes());
    let browser_object_bindings = collect_browser_object_bindings(root, source.as_bytes());
    if is_app_router_entry(relative) && !has_use_client_directive(root, source.as_bytes()) {
        return Err(AppError::InvalidRequest(format!(
            "generated App Router entrypoint {} must begin with a `use client` directive",
            relative.display()
        )));
    }
    validate_javascript_node(
        relative,
        root,
        source.as_bytes(),
        &string_bindings,
        &browser_object_bindings,
    )?;
    if let Some(declared) = declared {
        let capability_bindings =
            collect_capability_bindings(root, source.as_bytes(), &string_bindings);
        validate_capability_nodes(
            relative,
            root,
            source.as_bytes(),
            &string_bindings,
            &capability_bindings,
            declared,
        )?;
    }
    Ok(())
}

fn is_app_router_entry(relative: &Path) -> bool {
    relative.components().next() == Some(Component::Normal(std::ffi::OsStr::new("app")))
        && relative
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|stem| {
                matches!(
                    stem.to_ascii_lowercase().as_str(),
                    "page" | "layout" | "error" | "loading" | "not-found" | "template"
                )
            })
}

fn has_use_client_directive(root: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let Some(statement) = root.named_child(0) else {
        return false;
    };
    if statement.kind() != "expression_statement" {
        return false;
    }
    statement
        .named_child(0)
        .and_then(|node| javascript_string_value(node, source))
        .is_some_and(|value| value == "use client")
}

fn validate_javascript_node(
    relative: &Path,
    node: tree_sitter::Node<'_>,
    source: &[u8],
    string_bindings: &BTreeMap<String, String>,
    browser_object_bindings: &BTreeSet<String>,
) -> Result<(), AppError> {
    match node.kind() {
        "import_statement" | "export_statement" => {
            if let Some(module) = node.child_by_field_name("source") {
                let specifier = javascript_string_value(module, source).ok_or_else(|| {
                    AppError::InvalidRequest(format!(
                        "non-literal module specifier in {}",
                        relative.display()
                    ))
                })?;
                validate_import_specifier(relative, &specifier)?;
            }
        }
        "call_expression" => {
            if node
                .child_by_field_name("function")
                .is_some_and(|function| function.kind() == "import")
            {
                return forbidden_javascript(relative, "dynamic import");
            }
        }
        "expression_statement" => {
            if node
                .named_child(0)
                .and_then(|child| javascript_string_value(child, source))
                .is_some_and(|directive| directive == "use server")
            {
                return forbidden_javascript(relative, "Server Actions");
            }
        }
        "identifier" => {
            let identifier = node_text(node, source);
            if matches!(
                identifier,
                "require"
                    | "eval"
                    | "Function"
                    | "process"
                    | "global"
                    | "globalThis"
                    | "fetch"
                    | "XMLHttpRequest"
                    | "WebSocket"
                    | "EventSource"
                    | "Worker"
                    | "SharedWorker"
                    | "caches"
                    | "Cache"
                    | "CacheStorage"
                    | "Reflect"
                    | "Proxy"
            ) {
                return forbidden_javascript(relative, identifier);
            }
        }
        "member_expression" => {
            if let Some(property) = node.child_by_field_name("property") {
                let property = node_text(property, source);
                if is_forbidden_property(property) {
                    return forbidden_javascript(relative, property);
                }
            }
        }
        "subscript_expression" => {
            if let Some(index) = node.child_by_field_name("index") {
                if let Some(property) =
                    static_javascript_string_with_bindings(index, source, string_bindings)
                {
                    if is_forbidden_property(&property) {
                        return forbidden_javascript(relative, &property);
                    }
                } else if node.child_by_field_name("object").is_some_and(|object| {
                    is_browser_object_expression(object, source, browser_object_bindings)
                }) {
                    return forbidden_javascript(relative, "dynamic browser property access");
                }
            }
        }
        "jsx_attribute" => {
            let name = node
                .named_child(0)
                .map(|child| node_text(child, source))
                .unwrap_or_default();
            let tag = node
                .parent()
                .and_then(|element| {
                    element
                        .child_by_field_name("name")
                        .or_else(|| element.named_child(0))
                })
                .map(|tag| node_text(tag, source))
                .unwrap_or_default();
            let is_navigation_href = name == "href" && matches!(tag, "a" | "Link");
            let is_resource = matches!(
                name,
                "src"
                    | "srcSet"
                    | "poster"
                    | "action"
                    | "formAction"
                    | "data"
                    | "background"
                    | "ping"
                    | "xlinkHref"
            ) || (name == "href" && !is_navigation_href);
            if is_resource && contains_external_url(node_text(node, source)) {
                return forbidden_javascript(relative, "external JSX resource URL");
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        validate_javascript_node(
            relative,
            child,
            source,
            string_bindings,
            browser_object_bindings,
        )?;
    }
    Ok(())
}

fn forbidden_javascript<T>(relative: &Path, surface: &str) -> Result<T, AppError> {
    Err(AppError::InvalidRequest(format!(
        "forbidden {surface} in {}",
        relative.display()
    )))
}

fn node_text<'a>(node: tree_sitter::Node<'_>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or_default()
}

fn is_forbidden_property(property: &str) -> bool {
    matches!(
        property,
        "constructor"
            | "__proto__"
            | "prototype"
            | "require"
            | "eval"
            | "Function"
            | "process"
            | "global"
            | "globalThis"
            | "fetch"
            | "XMLHttpRequest"
            | "WebSocket"
            | "EventSource"
            | "Worker"
            | "SharedWorker"
            | "caches"
            | "sendBeacon"
            | "serviceWorker"
            | "getOwnPropertyDescriptor"
            | "getOwnPropertyDescriptors"
            | "getPrototypeOf"
            | "setPrototypeOf"
    )
}

fn validate_import_specifier(relative: &Path, specifier: &str) -> Result<(), AppError> {
    if ALLOWED_CLIENT_IMPORTS.contains(&specifier) {
        return Ok(());
    }
    if specifier.starts_with("node:")
        || specifier == "server-only"
        || specifier == "next/server"
        || NODE_MODULES
            .iter()
            .any(|module| specifier == *module || specifier.starts_with(&format!("{module}/")))
    {
        return forbidden_javascript(relative, &format!("Node/server import `{specifier}`"));
    }
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return Err(AppError::InvalidRequest(format!(
            "module import `{specifier}` in {} is outside the fixed client allowlist",
            relative.display()
        )));
    }
    if specifier.contains('\\') || specifier.contains('\0') {
        return Err(AppError::InvalidRequest(format!(
            "invalid relative module import `{specifier}` in {}",
            relative.display()
        )));
    }

    let mut resolved: Vec<String> = relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(ToOwned::to_owned),
            _ => None,
        })
        .collect();
    for segment in specifier.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if resolved.pop().is_none() {
                    return Err(AppError::InvalidRequest(format!(
                        "relative module import `{specifier}` escapes the workspace in {}",
                        relative.display()
                    )));
                }
            }
            value => resolved.push(value.to_string()),
        }
    }
    if !resolved
        .first()
        .is_some_and(|root| WRITABLE_ROOTS.contains(&root.as_str()))
        || resolved.iter().any(|segment| segment == "node_modules")
    {
        return Err(AppError::InvalidRequest(format!(
            "relative module import `{specifier}` leaves generated source roots in {}",
            relative.display()
        )));
    }
    Ok(())
}

fn javascript_string_value(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    (node.kind() == "string")
        .then(|| decode_javascript_string(node_text(node, source)))
        .flatten()
}

fn decode_javascript_string(raw: &str) -> Option<String> {
    let quote = raw.chars().next()?;
    if !matches!(quote, '\'' | '"') || raw.chars().last()? != quote {
        return None;
    }
    let mut chars = raw[quote.len_utf8()..raw.len() - quote.len_utf8()].chars();
    let mut value = String::new();
    while let Some(character) = chars.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            '\\' | '\'' | '"' => value.push(escaped),
            'b' => value.push('\u{0008}'),
            'f' => value.push('\u{000c}'),
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            't' => value.push('\t'),
            'v' => value.push('\u{000b}'),
            '0' => value.push('\0'),
            'x' => value.push(decode_hex_escape(&mut chars, 2)?),
            'u' => {
                if chars.clone().next() == Some('{') {
                    chars.next();
                    let mut hex = String::new();
                    for next in chars.by_ref() {
                        if next == '}' {
                            break;
                        }
                        hex.push(next);
                    }
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                } else {
                    value.push(decode_hex_escape(&mut chars, 4)?);
                }
            }
            '\n' => {}
            _ => return None,
        }
    }
    Some(value)
}

fn decode_hex_escape(chars: &mut impl Iterator<Item = char>, digits: usize) -> Option<char> {
    let mut hex = String::with_capacity(digits);
    for _ in 0..digits {
        hex.push(chars.next()?);
    }
    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
}

fn static_javascript_string_with_bindings(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Option<String> {
    if let Some(value) = javascript_string_value(node, source) {
        return Some(value);
    }
    if node.kind() == "identifier" {
        return bindings.get(node_text(node, source)).cloned();
    }
    if node.kind() != "binary_expression" {
        return None;
    }
    let left_node = node.child_by_field_name("left")?;
    let right_node = node.child_by_field_name("right")?;
    if std::str::from_utf8(&source[left_node.end_byte()..right_node.start_byte()])
        .ok()?
        .trim()
        != "+"
    {
        return None;
    }
    let left = static_javascript_string_with_bindings(left_node, source, bindings)?;
    let right = static_javascript_string_with_bindings(right_node, source, bindings)?;
    Some(format!("{left}{right}"))
}

fn collect_const_string_bindings(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> BTreeMap<String, String> {
    fn visit(node: tree_sitter::Node<'_>, source: &[u8], bindings: &mut BTreeMap<String, String>) {
        if node.kind() == "lexical_declaration"
            && node_text(node, source).trim_start().starts_with("const ")
        {
            let mut cursor = node.walk();
            for declarator in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                let Some(name) = declarator.child_by_field_name("name") else {
                    continue;
                };
                let Some(value) = declarator.child_by_field_name("value") else {
                    continue;
                };
                if name.kind() == "identifier" {
                    if let Some(value) =
                        static_javascript_string_with_bindings(value, source, bindings)
                    {
                        bindings.insert(node_text(name, source).to_string(), value);
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, bindings);
        }
    }

    let mut bindings = BTreeMap::new();
    visit(root, source, &mut bindings);
    bindings
}

fn is_browser_object_expression(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    bindings: &BTreeSet<String>,
) -> bool {
    match node.kind() {
        "identifier" => {
            let identifier = node_text(node, source);
            matches!(
                identifier,
                "document" | "navigator" | "location" | "window" | "globalThis"
            ) || bindings.contains(identifier)
        }
        "member_expression" | "subscript_expression" => node
            .child_by_field_name("object")
            .is_some_and(|object| is_browser_object_expression(object, source, bindings)),
        "parenthesized_expression" => node
            .named_child(0)
            .is_some_and(|child| is_browser_object_expression(child, source, bindings)),
        _ => false,
    }
}

fn collect_browser_object_bindings(root: tree_sitter::Node<'_>, source: &[u8]) -> BTreeSet<String> {
    fn visit(node: tree_sitter::Node<'_>, source: &[u8], bindings: &mut BTreeSet<String>) {
        if node.kind() == "lexical_declaration"
            && node_text(node, source).trim_start().starts_with("const ")
        {
            let mut cursor = node.walk();
            for declarator in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                let Some(name) = declarator.child_by_field_name("name") else {
                    continue;
                };
                let Some(value) = declarator.child_by_field_name("value") else {
                    continue;
                };
                if name.kind() == "identifier"
                    && is_browser_object_expression(value, source, bindings)
                {
                    bindings.insert(node_text(name, source).to_string());
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, bindings);
        }
    }

    let mut bindings = BTreeSet::new();
    visit(root, source, &mut bindings);
    bindings
}

fn capability_for_identifier(identifier: &str) -> Option<AppCapability> {
    CAPABILITY_CALLS.iter().find_map(|(needle, capability)| {
        let helper = needle.trim_start_matches('.').trim_end_matches('(');
        (!helper.contains('.') && helper.eq_ignore_ascii_case(identifier)).then_some(*capability)
    })
}

#[derive(Default)]
struct CapabilityBindings {
    calls: BTreeMap<String, AppCapability>,
    objects: BTreeMap<String, Vec<String>>,
}

fn javascript_member_path(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    string_bindings: &BTreeMap<String, String>,
    object_bindings: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" => {
            let identifier = node_text(node, source);
            object_bindings
                .get(identifier)
                .cloned()
                .or_else(|| Some(vec![identifier.to_ascii_lowercase()]))
        }
        "member_expression" => {
            let mut path = javascript_member_path(
                node.child_by_field_name("object")?,
                source,
                string_bindings,
                object_bindings,
            )?;
            path.push(
                node_text(node.child_by_field_name("property")?, source).to_ascii_lowercase(),
            );
            Some(path)
        }
        "subscript_expression" => {
            let mut path = javascript_member_path(
                node.child_by_field_name("object")?,
                source,
                string_bindings,
                object_bindings,
            )?;
            path.push(
                static_javascript_string_with_bindings(
                    node.child_by_field_name("index")?,
                    source,
                    string_bindings,
                )?
                .to_ascii_lowercase(),
            );
            Some(path)
        }
        "parenthesized_expression" => javascript_member_path(
            node.named_child(0)?,
            source,
            string_bindings,
            object_bindings,
        ),
        _ => None,
    }
}

fn capability_for_path(path: &[String]) -> Option<AppCapability> {
    if path.len() == 2 && path[0] == "lingxi_bridge" {
        return capability_for_identifier(&path[1]);
    }
    let [window, lingxi, version, group, operation] = path else {
        return None;
    };
    if window != "window" || lingxi != "lingxi" || version != "v1" {
        return None;
    }
    match (group.as_str(), operation.as_str()) {
        ("device", "capturephoto") => Some(AppCapability::Camera),
        ("device", "pickimage") => Some(AppCapability::PhotoLibrary),
        ("device", "recordaudiostart" | "transcribespeech") => Some(AppCapability::Microphone),
        ("device", "getlocation") => Some(AppCapability::Location),
        ("device", "postnotification") => Some(AppCapability::Notifications),
        ("llm", "chat") => Some(AppCapability::Llm),
        ("agent", "post") => Some(AppCapability::AgentNotify),
        ("data", "mutate") => Some(AppCapability::DataMutation),
        _ => None,
    }
}

fn capability_for_member(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    string_bindings: &BTreeMap<String, String>,
    object_bindings: &BTreeMap<String, Vec<String>>,
) -> Option<AppCapability> {
    capability_for_path(&javascript_member_path(
        node,
        source,
        string_bindings,
        object_bindings,
    )?)
}

fn pattern_identifier(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            Some(node_text(node, source).to_string())
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            pattern_identifier(node.child_by_field_name("left")?, source)
        }
        _ => None,
    }
}

fn pattern_property(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "property_identifier" | "shorthand_property_identifier_pattern" => {
            Some(node_text(node, source).to_ascii_lowercase())
        }
        "string" => javascript_string_value(node, source).map(|value| value.to_ascii_lowercase()),
        _ => None,
    }
}

fn bind_object_pattern(
    pattern: tree_sitter::Node<'_>,
    base_path: &[String],
    source: &[u8],
    bindings: &mut CapabilityBindings,
) {
    let mut cursor = pattern.walk();
    for entry in pattern.named_children(&mut cursor) {
        match entry.kind() {
            "shorthand_property_identifier_pattern" => {
                let property = node_text(entry, source).to_ascii_lowercase();
                let mut path = base_path.to_vec();
                path.push(property);
                if let Some(capability) = capability_for_path(&path) {
                    bindings
                        .calls
                        .insert(node_text(entry, source).to_string(), capability);
                }
            }
            "pair_pattern" => {
                let Some(property) = entry
                    .child_by_field_name("key")
                    .and_then(|key| pattern_property(key, source))
                else {
                    continue;
                };
                let Some(value) = entry.child_by_field_name("value") else {
                    continue;
                };
                let mut path = base_path.to_vec();
                path.push(property);
                if value.kind() == "object_pattern" {
                    bind_object_pattern(value, &path, source, bindings);
                } else if let Some(local) = pattern_identifier(value, source) {
                    if let Some(capability) = capability_for_path(&path) {
                        bindings.calls.insert(local, capability);
                    }
                }
            }
            "object_assignment_pattern" => {
                let Some(left) = entry.child_by_field_name("left") else {
                    continue;
                };
                let Some(local) = pattern_identifier(left, source) else {
                    continue;
                };
                let mut path = base_path.to_vec();
                path.push(local.to_ascii_lowercase());
                if let Some(capability) = capability_for_path(&path) {
                    bindings.calls.insert(local, capability);
                }
            }
            "rest_pattern" => {
                if let Some(local) = entry.named_child(0).and_then(|node| {
                    (node.kind() == "identifier").then(|| node_text(node, source).to_string())
                }) {
                    bindings.objects.insert(local, base_path.to_vec());
                }
            }
            _ => {}
        }
    }
}

fn collect_capability_bindings(
    root: tree_sitter::Node<'_>,
    source: &[u8],
    string_bindings: &BTreeMap<String, String>,
) -> CapabilityBindings {
    fn visit_import(node: tree_sitter::Node<'_>, source: &[u8], bindings: &mut CapabilityBindings) {
        match node.kind() {
            "import_specifier" => {
                let Some(imported) = node.child_by_field_name("name") else {
                    return;
                };
                let local = node.child_by_field_name("alias").unwrap_or(imported);
                if let Some(capability) = capability_for_identifier(node_text(imported, source)) {
                    bindings
                        .calls
                        .insert(node_text(local, source).to_string(), capability);
                }
            }
            "namespace_import" => {
                if let Some(local) = node.named_child(0) {
                    bindings.objects.insert(
                        node_text(local, source).to_string(),
                        vec!["lingxi_bridge".to_string()],
                    );
                }
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    visit_import(child, source, bindings);
                }
            }
        }
    }

    fn visit(
        node: tree_sitter::Node<'_>,
        source: &[u8],
        string_bindings: &BTreeMap<String, String>,
        bindings: &mut CapabilityBindings,
    ) {
        if node.kind() == "import_statement" {
            let is_bridge = node
                .child_by_field_name("source")
                .and_then(|module| javascript_string_value(module, source))
                .and_then(|module| module.rsplit('/').next().map(str::to_string))
                .is_some_and(|name| matches!(name.as_str(), "lingxi-bridge" | "lingxi-bridge.js"));
            if is_bridge {
                visit_import(node, source, bindings);
            }
        } else if node.kind() == "variable_declarator" {
            let name = node.child_by_field_name("name");
            let value = node.child_by_field_name("value");
            if let (Some(name), Some(value)) = (name, value) {
                if let Some(path) =
                    javascript_member_path(value, source, string_bindings, &bindings.objects)
                {
                    if name.kind() == "identifier" {
                        bindings
                            .objects
                            .insert(node_text(name, source).to_string(), path);
                    } else if name.kind() == "object_pattern" {
                        bind_object_pattern(name, &path, source, bindings);
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, string_bindings, bindings);
        }
    }

    let mut bindings = CapabilityBindings::default();
    visit(root, source, string_bindings, &mut bindings);
    bindings
}

fn validate_capability_nodes(
    relative: &Path,
    node: tree_sitter::Node<'_>,
    source: &[u8],
    string_bindings: &BTreeMap<String, String>,
    capability_bindings: &CapabilityBindings,
    declared: &[AppCapability],
) -> Result<(), AppError> {
    let capability = match node.kind() {
        "call_expression" => node
            .child_by_field_name("function")
            .filter(|callee| callee.kind() == "identifier")
            .and_then(|callee| {
                capability_bindings
                    .calls
                    .get(node_text(callee, source))
                    .copied()
            }),
        "member_expression" | "subscript_expression" => {
            capability_for_member(node, source, string_bindings, &capability_bindings.objects)
        }
        _ => None,
    };
    if let Some(capability) = capability.filter(|capability| !declared.contains(capability)) {
        let wire = serde_json::to_string(&capability)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        return Err(AppError::InvalidRequest(format!(
            "{} uses the `{wire}` capability, which this app's confirmed plan never \
             declared. The plan is the contract and cannot be changed now — remove the \
             call and implement the feature without it.",
            relative.display()
        )));
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        validate_capability_nodes(
            relative,
            child,
            source,
            string_bindings,
            capability_bindings,
            declared,
        )?;
    }
    Ok(())
}

fn strip_block_comments(mut source: &str, open: &str, close: &str) -> String {
    let mut output = String::with_capacity(source.len());
    while let Some(start) = source.find(open) {
        output.push_str(&source[..start]);
        let after_open = &source[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            return output;
        };
        source = &after_open[end + close.len()..];
    }
    output.push_str(source);
    output
}

fn contains_external_url(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("http://")
        || lower.contains("https://")
        || lower.contains("=\"//")
        || lower.contains("='//")
}

fn validate_style_source(relative: &str, source: &str) -> Result<(), AppError> {
    let semantic = strip_block_comments(source, "/*", "*/");
    let compact: String = semantic
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let lower = compact.to_ascii_lowercase();
    if [
        "url(http://",
        "url(https://",
        "url(//",
        "url(\"http://",
        "url(\"https://",
        "url(\"//",
        "url('http://",
        "url('https://",
        "url('//",
        "@import\"http://",
        "@import\"https://",
        "@import'http://",
        "@import'https://",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Err(AppError::InvalidRequest(format!(
            "external CSS resource URL is forbidden in {relative}"
        )));
    }
    Ok(())
}

fn validate_json_source(relative: &str, source: &str) -> Result<(), AppError> {
    let value: serde_json::Value = serde_json::from_str(source).map_err(|error| {
        AppError::InvalidRequest(format!("invalid JSON source {relative}: {error}"))
    })?;
    if json_contains_external_url(&value) {
        return Err(AppError::InvalidRequest(format!(
            "external JSON resource URL is forbidden in {relative}"
        )));
    }
    Ok(())
}

fn json_contains_external_url(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(value) => {
            let value = value.trim().to_ascii_lowercase();
            value.starts_with("http://") || value.starts_with("https://") || value.starts_with("//")
        }
        serde_json::Value::Array(values) => values.iter().any(json_contains_external_url),
        serde_json::Value::Object(values) => values.values().any(json_contains_external_url),
        _ => false,
    }
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
                    | "webmanifest"
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
            workspace.join("app/page.jsx"),
            "\"use client\"; export default function Page() { return <main /> }",
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

    /// The gap this closes: a plan that omits `camera` while the generated
    /// page calls `capturePhoto()` passes generation, screening, build and
    /// preview approval, and fails at the user's FIRST tap — by which time
    /// the capability list is frozen (`revise` cannot add one), so the app
    /// is unrecoverable. Caught here it is a generation-time rejection the
    /// model still has two repair attempts to fix.
    #[test]
    fn rejects_a_bridge_call_the_confirmed_plan_never_declared() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(
            workspace.join("app/page.jsx"),
            "\"use client\"; import { capturePhoto } from '../lib/lingxi-bridge';\n             export default function Page() { capturePhoto(); return <main /> }",
        )
        .unwrap();

        let error = validate_declared_capabilities(&layout, &policy, &[])
            .expect_err("an undeclared capability must not reach the user");
        let message = error.to_string();
        assert!(message.contains("camera"), "{message}");
        assert!(
            message.contains("app/page.jsx"),
            "the model needs the file to fix: {message}"
        );

        validate_declared_capabilities(&layout, &policy, &[AppCapability::Camera])
            .expect("the same call is legal once the plan declares it");
    }

    /// The direct form the prompt also documents, for apps that skip the
    /// helper wrapper.
    #[test]
    fn the_direct_window_form_is_screened_too() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(
            workspace.join("app/page.jsx"),
            "\"use client\"; export default function Page() { window.lingxi.v1.llm.chat({}); return <main /> }",
        )
        .unwrap();

        let error = validate_declared_capabilities(&layout, &policy, &[])
            .expect_err("window.lingxi.v1.llm.chat needs the llm capability");
        assert!(error.to_string().contains("llm"), "{error}");
    }

    /// A capability declared but never called is not an error: over-declaring
    /// costs one permission prompt that never fires, while failing the build
    /// over it would reject a legal app.
    #[test]
    fn an_unused_declared_capability_is_not_an_error() {
        let (_root, layout, policy) = fixture();
        validate_declared_capabilities(
            &layout,
            &policy,
            &[AppCapability::Camera, AppCapability::Llm],
        )
        .expect("declaring more than you use is allowed");
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
            workspace.join("app/page.jsx"),
            "\"use client\"; fetch('https://example.com')",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }

    #[test]
    fn allows_normal_function_declarations_but_rejects_function_constructor() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(
            workspace.join("app/page.jsx"),
            "\"use client\"; export default function Page() { function helper() { return 1 } return <main /> }",
        )
        .unwrap();
        validate_workspace_source(&layout, &policy).unwrap();
        fs::write(
            workspace.join("app/page.jsx"),
            "\"use client\"; export const build = Function('return 1')",
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
            workspace.join("app/api/test/route.js"),
            "export const GET = () => 1",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        fs::remove_dir_all(workspace.join("app/api")).unwrap();
        fs::write(workspace.join("escape.js"), "export {}").unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
    }

    fn write_page(workspace: &Path, source: &str) {
        fs::write(workspace.join("app/page.jsx"), source).unwrap();
    }

    #[test]
    fn rejects_unsupported_executable_extensions_and_server_entrypoints() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::create_dir(workspace.join("components")).unwrap();
        fs::write(
            workspace.join("components/Widget.tsx"),
            "export const Widget = () => null",
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        fs::remove_file(workspace.join("components/Widget.tsx")).unwrap();

        write_page(
            &workspace,
            "export default function Page() { return <main /> }",
        );
        let error = validate_workspace_source(&layout, &policy)
            .expect_err("generated App Router entrypoints must be client components");
        assert!(error.to_string().contains("use client"), "{error}");
    }

    #[test]
    fn javascript_policy_ignores_comments_and_string_contents() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client";
               // fetch("https://example.invalid"); eval("1"); node:https
               const help = "npm run and child_process are documentation words";
               export default function Page() { return <main>{help}</main>; }"#,
        );
        validate_workspace_source(&layout, &policy)
            .expect("comments and ordinary strings are not executable policy violations");
    }

    #[test]
    fn javascript_policy_rejects_node_and_dynamic_execution_surfaces() {
        let cases = [
            r#""use client"; import fs from "node:fs"; export default function Page(){return null}"#,
            r#""use client"; import cp from "child_process"; export default function Page(){return null}"#,
            r#""use client"; export { request } from "node:https""#,
            r#""use client"; const load = () => import("./other.js"); export default load"#,
            r#""use client"; const run = eval; export default () => run("1")"#,
            r#""use client"; const Make = Function; export default () => new Make("return 1")"#,
            r#""use client"; const env = process.env; export default () => env"#,
            r#""use client"; const spawn = globalThis["pro" + "cess"]; export default () => spawn"#,
            r#""use client"; const Ctor = ({})["constr" + "uctor"]["constructor"]; export default () => Ctor("return 1")()"#,
            r#""use client"; const Ctor = ({})["constr" + "uctor"]; export default () => Ctor"#,
        ];
        for source in cases {
            let (root, layout, policy) = fixture();
            let workspace = root.path().join(layout.workspace_rel());
            write_page(&workspace, source);
            assert!(
                validate_workspace_source(&layout, &policy).is_err(),
                "must reject executable surface: {source}"
            );
        }
    }

    #[test]
    fn javascript_policy_rejects_direct_network_and_browser_escape_surfaces() {
        let cases = [
            r#""use client"; const request = fetch; export default () => request("/")"#,
            r#""use client"; export default () => new XMLHttpRequest()"#,
            r#""use client"; export default () => new WebSocket("wss://example.test")"#,
            r#""use client"; export default () => new EventSource("/events")"#,
            r#""use client"; export default () => new Worker("worker.js")"#,
            r#""use client"; export default () => navigator.sendBeacon("/x")"#,
            r#""use client"; export default () => navigator.serviceWorker.register("/sw.js")"#,
            r#""use client"; export default () => caches.open("v1")"#,
            r#""use client"; export default () => window["fet" + "ch"]("/")"#,
            r#""use client"; const key = "fet" + "ch"; const w = document.defaultView; export default () => w[key]("/")"#,
            r#""use client"; const w = document.defaultView; export default key => w[key]("/")"#,
            r#""use client"; export default () => document.defaultView.fetch("/")"#,
        ];
        for source in cases {
            let (root, layout, policy) = fixture();
            let workspace = root.path().join(layout.workspace_rel());
            write_page(&workspace, source);
            assert!(
                validate_workspace_source(&layout, &policy).is_err(),
                "must reject browser escape surface: {source}"
            );
        }
    }

    #[test]
    fn javascript_policy_allows_fixed_client_imports_and_lingxi_bridge() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client";
               import { useMemo } from "react";
               import Link from "next/link";
               import { queryCollection } from "../lib/lingxi-bridge.js";
               export default function Page() {
                 const ready = useMemo(() => Boolean(window.lingxi.v1), []);
                 return <Link href="/">{ready ? "ready" : "wait"}</Link>;
               }"#,
        );
        validate_workspace_source(&layout, &policy).expect("fixed client surface is allowed");
    }

    #[test]
    fn external_navigation_href_is_not_misclassified_as_a_resource_fetch() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client";
               export default function Page() {
                 return <a href="https://example.test/docs">Open documentation</a>;
               }"#,
        );
        validate_workspace_source(&layout, &policy)
            .expect("external anchors remain subject to the host navigation confirmation");

        write_page(
            &workspace,
            r#""use client"; export default () => <img src="https://example.test/x.png" />"#,
        );
        assert!(validate_workspace_source(&layout, &policy).is_err());

        for source in [
            r#""use client"; export default () => <link rel="preload" href="https://example.test/x.css" />"#,
            r#""use client"; export default () => <svg><image href="https://example.test/x.png" /></svg>"#,
        ] {
            write_page(&workspace, source);
            assert!(
                validate_workspace_source(&layout, &policy).is_err(),
                "external resource href must be rejected: {source}"
            );
        }
    }

    #[test]
    fn capability_detection_ignores_comments_and_strings() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client";
               // capturePhoto(); window.lingxi.v1.llm.chat({});
               const help = "postNotification() and mutateCollection()";
               export default function Page() { return <main>{help}</main>; }"#,
        );
        validate_declared_capabilities(&layout, &policy, &[])
            .expect("non-executable mentions do not require capabilities");

        write_page(
            &workspace,
            r#""use client";
               const capturePhoto = "button label";
               export default function Page() { return <main>{capturePhoto}</main>; }"#,
        );
        validate_declared_capabilities(&layout, &policy, &[])
            .expect("a non-called local name matching a bridge helper is not a capability use");

        write_page(
            &workspace,
            r#""use client";
               function capturePhoto() { return "local"; }
               const helpers = { mutate() { return "local"; } };
               const { mutate } = helpers;
               export default function Page() { return <main>{capturePhoto()}{mutate()}</main>; }"#,
        );
        validate_declared_capabilities(&layout, &policy, &[])
            .expect("ordinary local calls and destructuring are not bridge capabilities");
    }

    #[test]
    fn capability_detection_rejects_member_aliases_and_computed_members() {
        for source in [
            r#""use client"; const cam = window.lingxi.v1.device.capturePhoto; export default () => <button onClick={() => cam()}>Photo</button>"#,
            r#""use client"; const key = "capture" + "Photo"; const cam = window.lingxi.v1.device[key]; export default () => cam()"#,
        ] {
            let (root, layout, policy) = fixture();
            let workspace = root.path().join(layout.workspace_rel());
            write_page(&workspace, source);
            let error = validate_declared_capabilities(&layout, &policy, &[])
                .expect_err("aliasing a bridge member must not bypass capability screening");
            assert!(error.to_string().contains("camera"), "{error}");
        }
    }

    #[test]
    fn capability_detection_rejects_bridge_object_destructuring_aliases() {
        let cases = [
            (
                r#""use client"; const { capturePhoto } = window.lingxi.v1.device; export default () => capturePhoto()"#,
                "camera",
            ),
            (
                r#""use client"; const { pickImage } = window.lingxi.v1.device; export default () => pickImage()"#,
                "photo_library",
            ),
            (
                r#""use client"; const { recordAudioStart } = window.lingxi.v1.device; export default () => recordAudioStart()"#,
                "microphone",
            ),
            (
                r#""use client"; const { getLocation } = window.lingxi.v1.device; export default () => getLocation()"#,
                "location",
            ),
            (
                r#""use client"; const { postNotification } = window.lingxi.v1.device; export default () => postNotification()"#,
                "notifications",
            ),
            (
                r#""use client"; const { chat } = window.lingxi.v1.llm; export default () => chat({})"#,
                "llm",
            ),
            (
                r#""use client"; const { post } = window.lingxi.v1.agent; export default () => post({})"#,
                "agent_notify",
            ),
            (
                r#""use client"; const { mutate } = window.lingxi.v1.data; export default () => mutate({})"#,
                "data_mutation",
            ),
            (
                r#""use client"; const { mutate: write } = window.lingxi.v1.data; export default () => write({})"#,
                "data_mutation",
            ),
            (
                r#""use client"; const { data: { mutate: write } } = window.lingxi.v1; export default () => write({})"#,
                "data_mutation",
            ),
            (
                r#""use client"; const { mutate: write } = window["lingxi"].v1["data"]; export default () => write({})"#,
                "data_mutation",
            ),
        ];
        for (source, wire) in cases {
            let (root, layout, policy) = fixture();
            let workspace = root.path().join(layout.workspace_rel());
            write_page(&workspace, source);
            let error = validate_declared_capabilities(&layout, &policy, &[])
                .expect_err("bridge destructuring must not bypass capability screening");
            assert!(error.to_string().contains(wire), "{source}: {error}");
        }

        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client"; const { mutate: write } = window.lingxi.v1.data; export default () => write({})"#,
        );
        validate_declared_capabilities(&layout, &policy, &[AppCapability::DataMutation])
            .expect("a confirmed capability remains allowed through a destructured alias");
    }

    #[test]
    fn benign_window_apis_remain_available() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        write_page(
            &workspace,
            r#""use client";
               export default function Page() {
                 const compact = window.matchMedia("(max-width: 600px)").matches;
                 window.addEventListener("resize", () => {});
                 return <main>{compact ? window.innerWidth : "wide"}</main>;
               }"#,
        );
        validate_workspace_source(&layout, &policy)
            .expect("benign window APIs are not direct network or dynamic-code surfaces");
    }

    #[test]
    fn generated_active_markup_is_fail_closed() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::create_dir(workspace.join("public")).unwrap();
        for (name, source) in [
            ("hostile.html", r#"<img src=x onerror="alert(1)">"#),
            ("hostile.svg", r#"<svg><script>alert(1)</script></svg>"#),
            (
                "remote.svg",
                r#"<svg><image href="https://example.test/x.png" /></svg>"#,
            ),
            ("document.xml", r#"<root href="https://example.test" />"#),
        ] {
            fs::write(workspace.join("public").join(name), source).unwrap();
            let error = validate_workspace_source(&layout, &policy)
                .expect_err("generated HTML/SVG/XML is executable or can fetch remote resources");
            assert!(error.to_string().contains("HTML/SVG/XML"), "{error}");
            fs::remove_file(workspace.join("public").join(name)).unwrap();
        }
        fs::write(
            workspace.join("app/manifest.js"),
            r#"export default function manifest(){return {icons:[{src:"https://example.test/icon.png"}]}}"#,
        )
        .unwrap();
        let error = validate_workspace_source(&layout, &policy)
            .expect_err("dynamic metadata routes must not bypass static manifest inspection");
        assert!(error.to_string().contains("metadata routes"), "{error}");
    }

    #[test]
    fn static_resource_policy_ignores_display_text_but_rejects_loads() {
        let (root, layout, policy) = fixture();
        let workspace = root.path().join(layout.workspace_rel());
        fs::create_dir(workspace.join("styles")).unwrap();
        fs::create_dir(workspace.join("public")).unwrap();
        fs::write(
            workspace.join("styles/help.css"),
            r#".help::after { content: "read https://example.test/docs"; }"#,
        )
        .unwrap();
        fs::write(
            workspace.join("public/help.txt"),
            "Read https://example.test/docs",
        )
        .unwrap();
        validate_workspace_source(&layout, &policy)
            .expect("display text containing a URL is not a resource load");

        fs::write(
            workspace.join("styles/help.css"),
            r#".hero { background: url(https://example.test/hero.png); }"#,
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        for quoted in [
            r#".hero { background: url("https://example.test/hero.png"); }"#,
            r#".hero { background: url('//example.test/hero.png'); }"#,
        ] {
            fs::write(workspace.join("styles/help.css"), quoted).unwrap();
            assert!(
                validate_workspace_source(&layout, &policy).is_err(),
                "quoted external CSS resource must be rejected: {quoted}"
            );
        }
        fs::write(workspace.join("styles/help.css"), ".hero { color: red; }").unwrap();
        fs::write(
            workspace.join("public/app.webmanifest"),
            r#"{"name":"App","icons":[{"src":"https://example.test/icon.png"}]}"#,
        )
        .unwrap();
        assert!(validate_workspace_source(&layout, &policy).is_err());
        fs::remove_file(workspace.join("public/app.webmanifest")).unwrap();
        fs::write(
            workspace.join("public/help.html"),
            r#"<script>document.body.textContent = "unsafe"</script>"#,
        )
        .unwrap();
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
