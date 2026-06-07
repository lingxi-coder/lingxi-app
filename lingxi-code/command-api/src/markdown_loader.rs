//! Filesystem loader for custom markdown slash commands (`.claude/commands/**.md`).
//!
//! Faithful port of the discovery half of
//! `claude-code/src/utils/markdownConfigLoader.ts` plus the namespacing helpers
//! from `claude-code/src/skills/loadSkillsDir.ts`:
//!
//! * [`project_dirs_up_to_home`] — the git-root-bounded upward walk collecting
//!   `.claude/<subdir>` directories (TS `getProjectDirsUpToHome`).
//! * [`load_command_markdown_files`] — walks managed / user / project dirs,
//!   recursively finds `*.md` via native `std::fs` (TS `findMarkdownFilesNative`
//!   — the `CLAUDE_CODE_USE_NATIVE_FILE_SEARCH` path), parses frontmatter, and
//!   deduplicates by `(dev, ino)` with `managed > user > project` priority
//!   (TS `loadMarkdownFilesForSubdir`).
//! * [`command_name_from_path`] — `buildNamespace` + `getRegularCommandName`
//!   (`:`-joined namespace from the relative directory path).
//! * [`extract_description_from_markdown`] — first non-empty line, `#`-header
//!   stripped, truncated to 100 chars (TS `extractDescriptionFromMarkdown`).
//! * [`build_markdown_command`] — assembles a [`SlashCommand`] of kind
//!   [`SlashCommandKind::Markdown`].
//!
//! ## Divergence from TS (documented intentionally)
//!
//! * **No `walkdir`/`ripgrep`.** Recursion is hand-written with `std::fs`,
//!   mirroring `findMarkdownFilesNative` (`--no-ignore --hidden --follow`):
//!   hidden files are not skipped, `.gitignore` is not consulted, symlinks are
//!   followed, and directory cycles are broken by a `(dev, ino)` visited-set.
//! * **`resolveStopBoundary` submodule refinement is simplified.** Without the
//!   `findCanonicalGitRoot`/`getProjectRoot` helpers (other crates), the upward
//!   walk stops at the nearest `.git` above `cwd`. The worktree fallback in
//!   [`load_command_markdown_files`] is ported in shape: when the nearest `.git`
//!   is a *file* (worktree marker) pointing at a `gitdir`, the resolved main
//!   repo's `.claude/<subdir>` is added iff the worktree itself lacks one.
//! * Frontmatter parsing reuses the existing `serde_yaml` dependency and the
//!   `---`/`\n---\n` splitter pattern from `skill-api::frontmatter`
//!   (`parse_skill_markdown`) without depending on `skill-api`.
//! * **`disable-model-invocation` and `when_to_use` are not yet carried.** TS
//!   `parseSkillFrontmatterFields` reads these two keys and `createSkillCommand`
//!   places them on the top-level command (`disableModelInvocation`, `whenToUse`).
//!   The matching `SlashCommand` fields exist, but the only path from the parse
//!   step to [`build_markdown_command`] is [`MarkdownCommandFile`], whose
//!   `frontmatter` is a [`CommandFrontmatter`] (defined in `crate::model`) with no
//!   slot for either value. Carrying them needs a field added in another file
//!   (`CommandFrontmatter`, or a new [`MarkdownCommandFile`] field that breaks the
//!   exhaustive struct literal in `crate::expand`'s tests), so they are
//!   intentionally left unported here.

use crate::argument_substitution::{parse_argument_names, FrontmatterArgs};
use crate::model::{
    CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind,
};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// A loaded markdown command file with its parsed frontmatter and body.
///
/// Mirrors the TS `MarkdownFile` record (minus the analytics `source` enum,
/// which is folded into [`CommandSource`] when building the command).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownCommandFile {
    /// Absolute path to the `.md` file on disk.
    pub file_path: PathBuf,
    /// The `.claude/<subdir>` directory the file was discovered under, used as
    /// the namespace base (TS `baseDir`).
    pub base_dir: PathBuf,
    /// Parsed YAML frontmatter (defaults when absent).
    pub frontmatter: CommandFrontmatter,
    /// Markdown body after the frontmatter block.
    pub content: String,
    /// Which configuration layer the file came from.
    pub source: CommandSource,
}

/// Raw YAML frontmatter shape mirroring the TS `FrontmatterData` slice that
/// slash commands consult. Kebab-case keys match the on-disk format. Unknown
/// keys are ignored (TS `[key: string]: unknown`).
#[derive(Debug, Default, Deserialize)]
struct RawFrontmatter {
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<ToolsField>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "argument-hint")]
    argument_hint: Option<String>,
    #[serde(default)]
    arguments: Option<ArgumentsField>,
    #[serde(default)]
    shell: Option<String>,
    /// SLASH.1: TS `disable-model-invocation` (boolean or the string `"true"`).
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: Option<Boolish>,
    /// SLASH.4: TS `when_to_use` (`snake_case` key, free-form string).
    #[serde(default)]
    when_to_use: Option<String>,
}

/// A frontmatter value that TS `parseBooleanFrontmatter` accepts as a boolean:
/// either a real YAML bool or the literal string `"true"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Boolish {
    Bool(bool),
    Str(String),
}

/// `allowed-tools` accepts either a single string or a list of strings
/// (TS `string | string[] | null`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolsField {
    One(String),
    Many(Vec<String>),
}

/// `arguments` accepts either a space-separated string or a list of names
/// (TS `string | string[] | undefined`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ArgumentsField {
    One(String),
    Many(Vec<String>),
}

/// Extract a description from markdown content. Faithful port of TS
/// `extractDescriptionFromMarkdown`: the first non-empty (trimmed) line, with a
/// leading `#`-header prefix stripped, truncated to 100 chars (`> 100` →
/// `substring(0, 97) + "..."`). Falls back to `default_description` when every
/// line is blank.
#[must_use]
pub fn extract_description_from_markdown(content: &str, default_description: &str) -> String {
    for line in content.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // `^#+\s+(.+)$`: one-or-more '#', then one-or-more whitespace, then the
        // rest. When it matches, use the captured tail; otherwise the whole line.
        let text = strip_header_prefix(trimmed).unwrap_or(trimmed);

        return if text.chars().count() > 100 {
            // TS `substring(0, 97) + '...'` — by UTF-16 code units in JS; here
            // we truncate by `char` (Unicode scalar) which matches for the BMP
            // text these descriptions contain.
            let head: String = text.chars().take(97).collect();
            format!("{head}...")
        } else {
            text.to_string()
        };
    }
    default_description.to_string()
}

/// Emulate `^#+\s+(.+)$` on an already-trimmed line. Returns the captured tail
/// (group 1) when the line is a header, else `None`.
fn strip_header_prefix(trimmed: &str) -> Option<&str> {
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b'#' {
        i += 1;
    }
    if i == 0 {
        return None; // no leading '#'
    }
    let after_hashes = i;
    while i < bytes.len() && is_regex_ws(bytes[i]) {
        i += 1;
    }
    // Need at least one whitespace char and a non-empty tail (`(.+)`).
    if i == after_hashes || i >= bytes.len() {
        return None;
    }
    Some(&trimmed[i..])
}

/// `\s` as the JS regex recognises it (ASCII subset that appears in practice).
fn is_regex_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// Compute the namespaced command name for a regular `.md` command file.
///
/// Faithful port of TS `getRegularCommandName` + `buildNamespace`: the file name
/// minus the `.md` suffix, prefixed by the `:`-joined path of intermediate
/// directories between `base_dir` and the file's directory. A file directly in
/// `base_dir` has no namespace.
#[must_use]
pub fn command_name_from_path(file: &Path, base_dir: &Path) -> String {
    let file_name = file
        .file_name()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let command_base_name = file_name
        .strip_suffix(".md")
        .unwrap_or(&file_name)
        .to_string();

    let file_directory = file.parent().unwrap_or(file);
    let namespace = build_namespace(file_directory, base_dir);
    if namespace.is_empty() {
        command_base_name
    } else {
        format!("{namespace}:{command_base_name}")
    }
}

/// Port of TS `buildNamespace`: the relative path from `base_dir` to
/// `target_dir`, with path separators replaced by `:`. Empty when they are the
/// same directory.
fn build_namespace(target_dir: &Path, base_dir: &Path) -> String {
    // TS strips a single trailing separator from baseDir before comparison; the
    // PathBuf comparison below already ignores trailing separators.
    match target_dir.strip_prefix(base_dir) {
        Ok(rel) => rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(":"),
        // Not under base_dir (shouldn't happen): no namespace (TS returns '').
        Err(_) => String::new(),
    }
}

/// Traverse from `cwd` up to the git root (or home if not in a git repo),
/// collecting existing `.claude/<subdir>` directories. Faithful port of TS
/// `getProjectDirsUpToHome` with the simplified `resolveStopBoundary` (nearest
/// `.git` above `cwd`; see the module divergence note).
///
/// Result ordering is most-specific (cwd) to least-specific, matching TS.
#[must_use]
pub fn project_dirs_up_to_home(subdir: &str, cwd: &Path, home: &Path) -> Vec<PathBuf> {
    let git_root = nearest_git_root(cwd);
    let home_norm = normalize_for_comparison(home);
    let mut current = cwd.to_path_buf();
    let mut dirs: Vec<PathBuf> = Vec::new();

    loop {
        // Stop at home (loaded separately as the user dir).
        if normalize_for_comparison(&current) == home_norm {
            break;
        }

        let claude_subdir = current.join(".claude").join(subdir);
        // Perf filter: only existing dirs (the worktree fallback relies on this).
        if std::fs::metadata(&claude_subdir).is_ok() {
            dirs.push(claude_subdir);
        }

        // Stop after processing the git-root directory.
        if let Some(root) = &git_root {
            if normalize_for_comparison(&current) == normalize_for_comparison(root) {
                break;
            }
        }

        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }

    dirs
}

/// Find the nearest ancestor (inclusive of `start`) containing a `.git` entry
/// (file or directory). Returns the directory holding `.git`, or `None` if none
/// is found before the filesystem root. Mirrors TS `findGitRoot`.
fn nearest_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if std::fs::symlink_metadata(current.join(".git")).is_ok() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// Resolve a git worktree's main repository root, if `git_root` holds a `.git`
/// *file* (the worktree marker `gitdir: <path>`). Returns the canonical main
/// repo working tree, or `None` when `.git` is a normal directory (not a
/// worktree). Approximates TS `findCanonicalGitRoot` for the worktree fallback.
fn canonical_git_root(git_root: &Path) -> Option<PathBuf> {
    let git_path = git_root.join(".git");
    let meta = std::fs::symlink_metadata(&git_path).ok()?;
    if !meta.file_type().is_file() {
        return None; // a real .git directory: not a worktree
    }
    let contents = std::fs::read_to_string(&git_path).ok()?;
    // Format: `gitdir: /path/to/main/.git/worktrees/<name>`
    let gitdir = contents
        .lines()
        .find_map(|l| l.strip_prefix("gitdir:"))
        .map(str::trim)?;
    let gitdir_path = Path::new(gitdir);
    // The main repo's working tree is the dir two levels above `worktrees/<name>`
    // relative to the main `.git`. i.e. `<main>/.git/worktrees/<name>` -> `<main>`.
    let worktrees_dir = gitdir_path.parent()?; // .git/worktrees
    let main_git_dir = worktrees_dir.parent()?; // .git
    if main_git_dir.file_name().map(|s| s == ".git") != Some(true) {
        return None;
    }
    main_git_dir.parent().map(Path::to_path_buf)
}

/// Load all custom markdown command files from the managed, user, and project
/// directories rooted at `cwd`. Faithful port of TS `loadMarkdownFilesForSubdir`
/// for `subdir == "commands"`.
///
/// * `cwd` — the session working directory (drives the project upward walk).
/// * `claude_home` — the user config dir (TS `getClaudeConfigHomeDir()`); the
///   user layer is `claude_home/commands`.
/// * `managed_dir` — the managed-policy root (TS `getManagedFilePath()`); the
///   managed layer is `managed_dir/.claude/commands`.
/// * `home` — the user's home directory, the upward-walk stop boundary.
///
/// Files are returned deduplicated by `(dev, ino)` with `managed > user >
/// project` priority and source order preserved within each layer.
#[must_use]
pub async fn load_command_markdown_files(
    cwd: &Path,
    claude_home: &Path,
    managed_dir: &Path,
    home: &Path,
) -> Vec<MarkdownCommandFile> {
    const SUBDIR: &str = "commands";

    let user_dir = claude_home.join(SUBDIR);
    let managed_commands_dir = managed_dir.join(".claude").join(SUBDIR);
    let mut project_dirs = project_dirs_up_to_home(SUBDIR, cwd, home);

    // Worktree fallback: when cwd's nearest .git is a worktree marker whose main
    // repo differs, and the worktree lacks its own `.claude/<subdir>`, add the
    // main repo's copy (TS `loadMarkdownFilesForSubdir` lines 320-335).
    if let Some(git_root) = nearest_git_root(cwd) {
        if let Some(canonical_root) = canonical_git_root(&git_root) {
            if normalize_for_comparison(&canonical_root) != normalize_for_comparison(&git_root) {
                let worktree_subdir =
                    normalize_for_comparison(&git_root.join(".claude").join(SUBDIR));
                let worktree_has_subdir = project_dirs
                    .iter()
                    .any(|d| normalize_for_comparison(d) == worktree_subdir);
                if !worktree_has_subdir {
                    let main_claude_subdir = canonical_root.join(".claude").join(SUBDIR);
                    if !project_dirs.iter().any(|d| *d == main_claude_subdir) {
                        project_dirs.push(main_claude_subdir);
                    }
                }
            }
        }
    }

    // Load each layer. Order: managed, user, then project dirs (most- to
    // least-specific). Combined priority: managed > user > project.
    let mut all_files: Vec<MarkdownCommandFile> = Vec::new();
    all_files.extend(load_markdown_dir(&managed_commands_dir, CommandSource::Managed));
    all_files.extend(load_markdown_dir(&user_dir, CommandSource::User));
    for project_dir in &project_dirs {
        all_files.extend(load_markdown_dir(project_dir, CommandSource::Project));
    }

    deduplicate_by_inode(all_files)
}

/// Deduplicate files that resolve to the same physical file (same `(dev, ino)`),
/// keeping the first occurrence (highest priority). Files whose identity cannot
/// be determined are kept (fail open). Faithful port of TS dedup loop using
/// `getFileIdentity`.
fn deduplicate_by_inode(all_files: Vec<MarkdownCommandFile>) -> Vec<MarkdownCommandFile> {
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut out = Vec::with_capacity(all_files.len());
    for file in all_files {
        match file_identity(&file.file_path) {
            Some(id) => {
                if !seen.insert(id) {
                    continue; // duplicate inode already loaded from higher priority
                }
                out.push(file);
            }
            // Cannot identify -> include (fail open).
            None => out.push(file),
        }
    }
    out
}

/// `(dev, ino)` identity of a file (following symlinks via `lstat`-equivalent).
///
/// TS uses `lstat` with bigint dev/ino and treats `dev==0 && ino==0` as
/// unidentifiable (network mounts). On non-unix targets identity is always
/// `None` (dedup disabled), matching the TS Windows fail-open note.
#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let dev = meta.dev();
    let ino = meta.ino();
    if dev == 0 && ino == 0 {
        return None;
    }
    Some((dev, ino))
}

#[cfg(not(unix))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Recursively load every `*.md` file under `dir`, parsing frontmatter. A
/// missing/inaccessible directory yields an empty vector (TS `isFsInaccessible`
/// fail-open). Symlinks are followed; directory cycles are broken by a
/// `(dev, ino)` visited-set.
fn load_markdown_dir(dir: &Path, source: CommandSource) -> Vec<MarkdownCommandFile> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut visited: HashSet<(u64, u64)> = HashSet::new();
    walk_markdown_files(dir, &mut files, &mut visited);

    let mut out = Vec::with_capacity(files.len());
    for file_path in files {
        match std::fs::read_to_string(&file_path) {
            Ok(raw) => {
                let (frontmatter, content) = parse_frontmatter(&raw);
                out.push(MarkdownCommandFile {
                    file_path,
                    base_dir: dir.to_path_buf(),
                    frontmatter,
                    content,
                    source,
                });
            }
            // Read/parse failure: skip this file (TS returns null and filters).
            Err(_) => continue,
        }
    }
    out
}

/// Recursive `*.md` finder. Faithful to TS `findMarkdownFilesNative`: follows
/// symlinks, tracks visited directories by `(dev, ino)` to break cycles, does
/// not skip hidden entries, and does not consult `.gitignore`.
fn walk_markdown_files(
    current_dir: &Path,
    files: &mut Vec<PathBuf>,
    visited: &mut HashSet<(u64, u64)>,
) {
    // Cycle detection via the directory's identity (follows symlinks: `stat`).
    if let Some(id) = dir_identity(current_dir) {
        if !visited.insert(id) {
            return;
        }
    }

    let Ok(entries) = std::fs::read_dir(current_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let full_path = entry.path();
        // `stat` follows symlinks (TS `stat(fullPath)` for the symlink branch and
        // the plain isFile/isDirectory checks below collapse to the same logic).
        let Ok(meta) = std::fs::metadata(&full_path) else {
            continue;
        };
        if meta.is_dir() {
            walk_markdown_files(&full_path, files, visited);
        } else if meta.is_file() && has_md_extension(&full_path) {
            files.push(full_path);
        }
    }
}

/// `true` when the file name ends with the literal `.md` suffix. TS uses
/// case-sensitive `name.endsWith('.md')`; this preserves that (a `.MD` file is
/// not matched), so the clippy case-insensitive suggestion is intentionally not
/// taken.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn has_md_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".md"))
}

/// `(dev, ino)` of a directory (following symlinks) for cycle detection.
#[cfg(unix)]
fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_dir() {
        return None;
    }
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    // Fall back to canonical path hashing-free cycle detection: use realpath via
    // canonicalize. Encode as (hash, 0); collisions are acceptable for cycles.
    let real = std::fs::canonicalize(path).ok()?;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    real.hash(&mut h);
    Some((h.finish(), 0))
}

/// Split a raw markdown string into frontmatter + body. Mirrors the
/// `skill-api::frontmatter::parse_skill_markdown` `---`/`\n---\n` splitter (the
/// shape the spec asks to reuse without depending on `skill-api`). Malformed or
/// absent frontmatter yields default frontmatter and the raw body.
fn parse_frontmatter(raw: &str) -> (CommandFrontmatter, String) {
    if let Some(rest) = raw.strip_prefix("---") {
        if let Some(end) = rest.find("\n---\n") {
            let yaml = &rest[..end];
            let body = &rest[end + 5..];
            if let Ok(raw_fm) = serde_yaml::from_str::<RawFrontmatter>(yaml) {
                return (build_frontmatter(raw_fm), body.trim_start().to_string());
            }
            // Malformed YAML: treat as no frontmatter (TS parse failure -> skip,
            // but here we fail open with defaults + full body for robustness).
        }
    }
    (CommandFrontmatter::default(), raw.to_string())
}

/// Map the raw kebab-case YAML frontmatter onto the typed [`CommandFrontmatter`].
fn build_frontmatter(raw: RawFrontmatter) -> CommandFrontmatter {
    // SLASH.5: TS stores `allowed-tools` via
    // `parseSlashCommandToolsFromFrontmatter`, which flattens the value to a
    // `string[]` then runs it through `parseToolListFromCLI` (paren-aware
    // comma/space split, trimmed) instead of keeping it verbatim. Both a single
    // string and a YAML list go through the same splitter.
    let allowed_tools = match raw.allowed_tools {
        None => None,
        Some(ToolsField::One(s)) => Some(parse_slash_command_tools_from_frontmatter(&[s])),
        Some(ToolsField::Many(v)) => Some(parse_slash_command_tools_from_frontmatter(&v)),
    };
    let argument_hints = raw.argument_hint.map(|h| vec![h]).unwrap_or_default();
    let argument_names = match raw.arguments {
        None => Vec::new(),
        Some(ArgumentsField::One(s)) => parse_argument_names(Some(&FrontmatterArgs::Str(s))),
        Some(ArgumentsField::Many(v)) => parse_argument_names(Some(&FrontmatterArgs::List(v))),
    };
    let shell = raw.shell.and_then(|s| match s.as_str() {
        "bash" => Some(crate::model::FrontmatterShell::Bash),
        "powershell" => Some(crate::model::FrontmatterShell::PowerShell),
        _ => None,
    });
    // SLASH.1: TS `parseBooleanFrontmatter` (frontmatterParser.ts) returns true
    // ONLY for a boolean `true` or the exact string `"true"` — every other value
    // (including `"false"`, `"yes"`, `1`) is false.
    let disable_model_invocation = match raw.disable_model_invocation {
        Some(Boolish::Bool(b)) => b,
        Some(Boolish::Str(s)) => s == "true",
        None => false,
    };
    CommandFrontmatter {
        description: raw.description.unwrap_or_default(),
        allowed_tools,
        model: raw.model,
        argument_hints,
        argument_names,
        thinking: None,
        shell,
        disable_model_invocation,
        // SLASH.4: TS copies `frontmatter.when_to_use` verbatim.
        when_to_use: raw.when_to_use,
    }
}

/// Port of TS `parseSlashCommandToolsFromFrontmatter` (slice that applies once
/// the value is already a `string[]`): run the tools through
/// [`parse_tool_list_from_cli`], then collapse to `["*"]` if any parsed entry is
/// the `*` wildcard (TS `parseToolListString` line `if (parsedTools.includes('*'))
/// return ['*']`).
fn parse_slash_command_tools_from_frontmatter(tools: &[String]) -> Vec<String> {
    let parsed = parse_tool_list_from_cli(tools);
    if parsed.iter().any(|t| t == "*") {
        vec!["*".to_string()]
    } else {
        parsed
    }
}

/// Faithful port of TS `parseToolListFromCLI` (`permissionSetup.ts`): split each
/// string on top-level commas and spaces, trimming each tool, while keeping any
/// separators that appear inside parentheses (e.g. `Bash(git log, foo)` stays one
/// entry). Empty strings are skipped and empty/whitespace-only tools are dropped.
fn parse_tool_list_from_cli(tools: &[String]) -> Vec<String> {
    if tools.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<String> = Vec::new();

    for tool_string in tools {
        if tool_string.is_empty() {
            continue;
        }

        let mut current = String::new();
        let mut is_in_parens = false;

        for ch in tool_string.chars() {
            match ch {
                '(' => {
                    is_in_parens = true;
                    current.push(ch);
                }
                ')' => {
                    is_in_parens = false;
                    current.push(ch);
                }
                ',' => {
                    if is_in_parens {
                        current.push(ch);
                    } else {
                        // Comma separator — push current tool and start a new one.
                        if !current.trim().is_empty() {
                            result.push(current.trim().to_string());
                        }
                        current.clear();
                    }
                }
                ' ' => {
                    if is_in_parens {
                        current.push(ch);
                    } else if !current.trim().is_empty() {
                        // Space separator — push current tool and start a new one.
                        result.push(current.trim().to_string());
                        current.clear();
                    }
                }
                _ => current.push(ch),
            }
        }

        // Push any remaining tool.
        if !current.trim().is_empty() {
            result.push(current.trim().to_string());
        }
    }

    result
}

/// Build a [`SlashCommand`] of kind [`SlashCommandKind::Markdown`] from a loaded
/// file. The `name` is computed from the file path relative to its base dir
/// (namespaced); the description is the frontmatter `description` when set, else
/// [`extract_description_from_markdown`] over the body (TS `Custom command`
/// fallback).
#[must_use]
pub fn build_markdown_command(file: &MarkdownCommandFile, source: CommandSource) -> SlashCommand {
    let name = command_name_from_path(&file.file_path, &file.base_dir);
    // Mirrors TS `hasUserSpecifiedDescription`: true when the frontmatter
    // carried an explicit `description`, false when we auto-derive it from the
    // body (`Custom command` fallback).
    let has_user_specified_description = !file.frontmatter.description.is_empty();
    let description = if file.frontmatter.description.is_empty() {
        extract_description_from_markdown(&file.content, "Custom command")
    } else {
        file.frontmatter.description.clone()
    };
    SlashCommand {
        name,
        description,
        source,
        kind: SlashCommandKind::Markdown {
            file_path: file.file_path.clone(),
            frontmatter: file.frontmatter.clone(),
            prompt_template: file.content.clone(),
        },
        has_user_specified_description,
        // SLASH.2: TS `createSkillCommand` copies the frontmatter `argument-hint`
        // onto the top-level command (`argumentHint`). The frontmatter parser
        // stores it as a one-element `argument_hints` vec (empty when the key is
        // absent), so its first element is the hint — mirroring TS
        // `frontmatter['argument-hint'] != null ? String(...) : undefined`.
        argument_hint: file.frontmatter.argument_hints.first().cloned(),
        // SLASH.3: legacy `.claude/commands/**.md` files load through TS
        // `loadSkillsFromCommandsDir`, which tags every command it builds with
        // `loadedFrom: 'commands_DEPRECATED'`.
        loaded_from: Some("commands_DEPRECATED".to_string()),
        // SLASH.1/SLASH.4: TS `createSkillCommand` copies `disable-model-invocation`
        // and `when_to_use` from the parsed frontmatter onto the top-level command.
        disable_model_invocation: file.frontmatter.disable_model_invocation,
        when_to_use: file.frontmatter.when_to_use.clone(),
        ..SlashCommand::default()
    }
}

/// Normalize a path for case/separator-insensitive comparison. Approximates TS
/// `normalizePathForComparison`: resolves the path lexically and lowercases it
/// on case-insensitive platforms. On unix we keep case (case-sensitive FS) but
/// strip a trailing separator for stable comparisons.
fn normalize_for_comparison(p: &Path) -> String {
    let s = p.to_string_lossy();
    let trimmed = s.strip_suffix('/').unwrap_or(&s);
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ---------- extract_description_from_markdown ----------

    #[test]
    fn extract_description_strips_header_prefix() {
        assert_eq!(
            extract_description_from_markdown("# My Command\n\nbody", "Custom command"),
            "My Command"
        );
        assert_eq!(
            extract_description_from_markdown("### Deep Header   \nmore", "Custom command"),
            "Deep Header"
        );
    }

    #[test]
    fn extract_description_uses_first_non_empty_line() {
        assert_eq!(
            extract_description_from_markdown("\n\n   \nplain text line\nnext", "Custom command"),
            "plain text line"
        );
    }

    #[test]
    fn extract_description_falls_back_to_default() {
        assert_eq!(
            extract_description_from_markdown("\n  \n\t\n", "Custom command"),
            "Custom command"
        );
    }

    #[test]
    fn extract_description_truncates_at_100() {
        // 120 'a' chars, no header -> first 97 + "..." (length 100).
        let long = "a".repeat(120);
        let out = extract_description_from_markdown(&long, "Custom command");
        assert_eq!(out.chars().count(), 100);
        assert!(out.ends_with("..."));
        assert_eq!(&out[..97], &"a".repeat(97));
    }

    #[test]
    fn extract_description_exactly_100_not_truncated() {
        let exactly = "a".repeat(100);
        let out = extract_description_from_markdown(&exactly, "Custom command");
        assert_eq!(out, exactly);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn extract_description_hash_without_space_is_not_header() {
        // `#no-space` does not match `^#+\s+(.+)$`; the whole trimmed line is used.
        assert_eq!(
            extract_description_from_markdown("#nospace text", "Custom command"),
            "#nospace text"
        );
    }

    // ---------- command_name_from_path ----------

    #[test]
    fn command_name_top_level() {
        let base = Path::new("/root/.claude/commands");
        let file = base.join("foo.md");
        assert_eq!(command_name_from_path(&file, base), "foo");
    }

    #[test]
    fn command_name_namespaced() {
        let base = Path::new("/root/.claude/commands");
        let file = base.join("sub").join("bar.md");
        assert_eq!(command_name_from_path(&file, base), "sub:bar");
    }

    #[test]
    fn command_name_deep_namespace() {
        let base = Path::new("/root/.claude/commands");
        let file = base.join("a").join("b").join("c.md");
        assert_eq!(command_name_from_path(&file, base), "a:b:c");
    }

    // ---------- build_frontmatter ----------

    #[test]
    fn frontmatter_allowed_tools_model_argument_hint_parse() {
        let raw = "---\nallowed-tools:\n  - Bash\n  - Edit\nmodel: opus\nargument-hint: <file>\narguments: first second\n---\nbody here\n";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(
            fm.allowed_tools,
            Some(vec!["Bash".to_string(), "Edit".to_string()])
        );
        assert_eq!(fm.model.as_deref(), Some("opus"));
        assert_eq!(fm.argument_hints, vec!["<file>".to_string()]);
        assert_eq!(
            fm.argument_names,
            vec!["first".to_string(), "second".to_string()]
        );
        // Body is `trim_start`-ed only (mirrors skill-api's splitter), so the
        // trailing newline from the file is preserved.
        assert_eq!(body, "body here\n");
    }

    #[test]
    fn frontmatter_allowed_tools_single_string() {
        let raw = "---\nallowed-tools: Bash\n---\nx";
        let (fm, _) = parse_frontmatter(raw);
        assert_eq!(fm.allowed_tools, Some(vec!["Bash".to_string()]));
    }

    // ---------- SLASH.5: allowed-tools is split via parseToolListFromCLI ----------

    #[test]
    fn frontmatter_allowed_tools_splits_comma_and_space_string() {
        // A single comma/space separated string must split into individual tools
        // (TS `parseToolListFromCLI`), each trimmed — not kept verbatim.
        let (fm, _) = parse_frontmatter("---\nallowed-tools: Bash, Edit  Read\n---\nx");
        assert_eq!(
            fm.allowed_tools,
            Some(vec![
                "Bash".to_string(),
                "Edit".to_string(),
                "Read".to_string()
            ])
        );
    }

    #[test]
    fn frontmatter_allowed_tools_keeps_commas_inside_parens() {
        // Commas/spaces inside `(...)` are part of the tool spec, not separators.
        let (fm, _) = parse_frontmatter("---\nallowed-tools: Bash(git log, foo), Read\n---\nx");
        assert_eq!(
            fm.allowed_tools,
            Some(vec!["Bash(git log, foo)".to_string(), "Read".to_string()])
        );
    }

    #[test]
    fn frontmatter_allowed_tools_list_entries_are_each_split() {
        // Each YAML list entry is itself run through the splitter.
        let (fm, _) =
            parse_frontmatter("---\nallowed-tools:\n  - Bash, Edit\n  - Read\n---\nx");
        assert_eq!(
            fm.allowed_tools,
            Some(vec![
                "Bash".to_string(),
                "Edit".to_string(),
                "Read".to_string()
            ])
        );
    }

    #[test]
    fn frontmatter_allowed_tools_wildcard_collapses() {
        // Any `*` among the parsed tools collapses the whole list to `["*"]`
        // (TS `parseToolListString`).
        let (fm, _) = parse_frontmatter("---\nallowed-tools: Bash, *\n---\nx");
        assert_eq!(fm.allowed_tools, Some(vec!["*".to_string()]));
    }

    #[test]
    fn parse_tool_list_from_cli_matches_ts_examples() {
        assert_eq!(
            parse_tool_list_from_cli(&["A, B C".to_string()]),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
        // Leading/trailing whitespace and empty segments are dropped.
        assert_eq!(
            parse_tool_list_from_cli(&["  Foo ,, , Bar  ".to_string()]),
            vec!["Foo".to_string(), "Bar".to_string()]
        );
        // Empty strings in the array are skipped entirely.
        assert_eq!(
            parse_tool_list_from_cli(&[String::new(), "X".to_string()]),
            vec!["X".to_string()]
        );
        assert!(parse_tool_list_from_cli(&[]).is_empty());
    }

    #[test]
    fn frontmatter_absent_yields_defaults() {
        let (fm, body) = parse_frontmatter("just a body\nwith lines");
        assert_eq!(fm.allowed_tools, None);
        assert_eq!(fm.model, None);
        assert!(fm.argument_hints.is_empty());
        assert!(fm.argument_names.is_empty());
        assert_eq!(body, "just a body\nwith lines");
    }

    #[test]
    fn frontmatter_shell_parses() {
        let (fm, _) = parse_frontmatter("---\nshell: powershell\n---\nx");
        assert_eq!(fm.shell, Some(crate::model::FrontmatterShell::PowerShell));
    }

    // ---------- build_markdown_command ----------

    #[test]
    fn build_markdown_command_uses_extracted_description() {
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/root/.claude/commands/foo.md"),
            base_dir: PathBuf::from("/root/.claude/commands"),
            frontmatter: CommandFrontmatter::default(),
            content: "# Foo Title\n\nHello $1".to_string(),
            source: CommandSource::Project,
        };
        let cmd = build_markdown_command(&file, CommandSource::Project);
        assert_eq!(cmd.name, "foo");
        assert_eq!(cmd.description, "Foo Title");
        match cmd.kind {
            SlashCommandKind::Markdown {
                prompt_template, ..
            } => assert_eq!(prompt_template, "# Foo Title\n\nHello $1"),
            _ => panic!("expected Markdown kind"),
        }
    }

    #[test]
    fn build_markdown_command_prefers_frontmatter_description() {
        let fm = CommandFrontmatter {
            description: "explicit".to_string(),
            ..CommandFrontmatter::default()
        };
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/r/.claude/commands/x.md"),
            base_dir: PathBuf::from("/r/.claude/commands"),
            frontmatter: fm,
            content: "# Ignored Title".to_string(),
            source: CommandSource::User,
        };
        let cmd = build_markdown_command(&file, CommandSource::User);
        assert_eq!(cmd.description, "explicit");
    }

    #[test]
    fn build_markdown_command_copies_argument_hint_and_sets_loaded_from() {
        // SLASH.2: the frontmatter `argument-hint` is surfaced on the top-level
        // command. SLASH.3: legacy commands are tagged `commands_DEPRECATED`.
        let fm = CommandFrontmatter {
            argument_hints: vec!["<file> [flags]".to_string()],
            ..CommandFrontmatter::default()
        };
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/r/.claude/commands/x.md"),
            base_dir: PathBuf::from("/r/.claude/commands"),
            frontmatter: fm,
            content: "# Body".to_string(),
            source: CommandSource::Project,
        };
        let cmd = build_markdown_command(&file, CommandSource::Project);
        assert_eq!(cmd.argument_hint.as_deref(), Some("<file> [flags]"));
        assert_eq!(cmd.loaded_from.as_deref(), Some("commands_DEPRECATED"));
    }

    #[test]
    fn frontmatter_disable_model_invocation_bool_and_string_true() {
        // SLASH.1: TS parseBooleanFrontmatter is true ONLY for `true` / "true".
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: true\n---\nx");
        assert!(fm.disable_model_invocation);
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: \"true\"\n---\nx");
        assert!(fm.disable_model_invocation);
    }

    #[test]
    fn frontmatter_disable_model_invocation_false_and_absent() {
        // SLASH.1: anything other than true/"true" (incl. "false", absent) is false.
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: false\n---\nx");
        assert!(!fm.disable_model_invocation);
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: \"yes\"\n---\nx");
        assert!(!fm.disable_model_invocation);
        let (fm, _) = parse_frontmatter("---\ndescription: d\n---\nx");
        assert!(!fm.disable_model_invocation);
    }

    #[test]
    fn frontmatter_when_to_use_parsed_and_copied_to_command() {
        // SLASH.4: `when_to_use` is parsed and surfaced on the top-level command.
        let (fm, _) = parse_frontmatter("---\nwhen_to_use: use for X\n---\nbody");
        assert_eq!(fm.when_to_use.as_deref(), Some("use for X"));
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/r/.claude/commands/x.md"),
            base_dir: PathBuf::from("/r/.claude/commands"),
            frontmatter: fm,
            content: "# Body".to_string(),
            source: CommandSource::Project,
        };
        let cmd = build_markdown_command(&file, CommandSource::Project);
        assert_eq!(cmd.when_to_use.as_deref(), Some("use for X"));
    }

    #[test]
    fn disable_model_invocation_excludes_command_from_model_invocable_set() {
        // SLASH.1 end-to-end: the parsed flag flows onto SlashCommand so the
        // registry model-invocable filter (`!disable_model_invocation`) drops it.
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: true\n---\nx");
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/r/.claude/commands/hidden.md"),
            base_dir: PathBuf::from("/r/.claude/commands"),
            frontmatter: fm,
            content: "# Body".to_string(),
            source: CommandSource::Project,
        };
        let cmd = build_markdown_command(&file, CommandSource::Project);
        assert!(cmd.disable_model_invocation);
    }

    #[test]
    fn build_markdown_command_argument_hint_absent_is_none() {
        // No `argument-hint` frontmatter -> top-level hint stays `None`, but the
        // `commands_DEPRECATED` marker is still set.
        let file = MarkdownCommandFile {
            file_path: PathBuf::from("/r/.claude/commands/y.md"),
            base_dir: PathBuf::from("/r/.claude/commands"),
            frontmatter: CommandFrontmatter::default(),
            content: "# Body".to_string(),
            source: CommandSource::Project,
        };
        let cmd = build_markdown_command(&file, CommandSource::Project);
        assert_eq!(cmd.argument_hint, None);
        assert_eq!(cmd.loaded_from.as_deref(), Some("commands_DEPRECATED"));
    }

    // ---------- filesystem loader (integration) ----------

    /// Unique temp dir under the OS temp root.
    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cmdapi-mdtest-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn loads_top_level_and_namespaced_commands() {
        let root = temp_dir("names");
        let cmds = root.join("proj").join(".claude").join("commands");
        write(&cmds.join("foo.md"), "Hello $1");
        write(&cmds.join("sub").join("bar.md"), "Bar body");

        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let claude_home = root.join("home").join(".claude");
        let managed = root.join("managed-none");

        let files =
            load_command_markdown_files(&root.join("proj"), &claude_home, &managed, &home).await;
        let mut names: Vec<String> = files
            .iter()
            .map(|f| command_name_from_path(&f.file_path, &f.base_dir))
            .collect();
        names.sort();
        assert_eq!(names, vec!["foo".to_string(), "sub:bar".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn managed_user_project_precedence_via_dedup() {
        // Same physical file shared (hardlink) across managed and project: managed
        // wins (loaded first), project duplicate dropped by inode dedup.
        let root = temp_dir("prec");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();

        let project = root.join("proj");
        let proj_cmds = project.join(".claude").join("commands");
        write(&proj_cmds.join("shared.md"), "shared body");

        let managed = root.join("managed");
        let managed_cmds = managed.join(".claude").join("commands");
        fs::create_dir_all(&managed_cmds).unwrap();
        // Hardlink the same inode into the managed dir.
        fs::hard_link(proj_cmds.join("shared.md"), managed_cmds.join("shared.md")).unwrap();

        let claude_home = root.join("home").join(".claude");
        let files = load_command_markdown_files(&project, &claude_home, &managed, &home).await;
        let shared: Vec<&MarkdownCommandFile> =
            files.iter().filter(|f| f.content == "shared body").collect();
        assert_eq!(shared.len(), 1, "inode dedup should keep exactly one");
        assert_eq!(
            shared[0].source,
            CommandSource::Managed,
            "managed must win over project"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn inode_dedup_drops_symlinked_duplicate() {
        // Mirrors the TS scenario: a *directory* symlink causes the same physical
        // file to be discovered through two paths. `lstat` of the file reached
        // via the symlinked parent resolves to the real file's inode (only the
        // parent dir was a symlink), so the duplicate is dropped.
        let root = temp_dir("symlink");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();

        let project = root.join("proj");
        let proj_cmds = project.join(".claude").join("commands");
        write(&proj_cmds.join("real.md"), "real body");

        // The user commands dir is a SYMLINK to the project's commands dir, so the
        // same `real.md` is reachable as both project and user.
        let claude_home = root.join("home").join(".claude");
        fs::create_dir_all(&claude_home).unwrap();
        std::os::unix::fs::symlink(&proj_cmds, claude_home.join("commands")).unwrap();

        let managed = root.join("managed-none");
        let files = load_command_markdown_files(&project, &claude_home, &managed, &home).await;
        let bodies: Vec<&MarkdownCommandFile> =
            files.iter().filter(|f| f.content == "real body").collect();
        assert_eq!(
            bodies.len(),
            1,
            "symlinked duplicate (same inode) must be dropped"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn missing_dirs_yield_empty() {
        let root = temp_dir("empty");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let claude_home = root.join("home").join(".claude");
        let managed = root.join("nope");
        let files =
            load_command_markdown_files(&root.join("noproj"), &claude_home, &managed, &home).await;
        assert!(files.is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn project_dirs_stops_at_git_root() {
        let root = temp_dir("gitstop");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();

        // root/repo/.git , root/repo/.claude/commands , root/repo/nested/.claude/commands
        let repo = root.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join(".claude").join("commands")).unwrap();
        let nested = repo.join("nested");
        fs::create_dir_all(nested.join(".claude").join("commands")).unwrap();
        // Above the repo: a .claude/commands that must NOT be collected.
        fs::create_dir_all(root.join(".claude").join("commands")).unwrap();

        let dirs = project_dirs_up_to_home("commands", &nested, &home);
        // Most-specific first: nested, then repo. Stops at repo (git root).
        assert_eq!(
            dirs,
            vec![
                nested.join(".claude").join("commands"),
                repo.join(".claude").join("commands"),
            ]
        );

        fs::remove_dir_all(&root).ok();
    }
}
