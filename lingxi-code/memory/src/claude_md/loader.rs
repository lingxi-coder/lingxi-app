//! File reader with 10 MB cap.
//!
//! Beyond the raw [`load_file`] reader this module also ports the
//! claude-code `@import` / `@include` expansion and the per-file body
//! sanitisation (frontmatter + block HTML-comment stripping) so the
//! orchestrator can splice referenced files into the `<memory>` block
//! exactly the way the TS reference does. See [`expand_memory_file`].

use crate::MAX_MEMORY_FILE_SIZE;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use thiserror::Error;

/// One loaded CLAUDE.md (or local override) file.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    /// Absolute path the file was loaded from.
    pub path: PathBuf,
    /// File body (post-cap, secrets NOT yet redacted at this layer).
    pub body: String,
    /// File size in bytes at the time of load (pre-redaction).
    pub size_bytes: u64,
}

/// Failure modes the loader can encounter.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// I/O error reading the file.
    #[error("io: {0}")]
    Io(String),
    /// File exceeded `MAX_MEMORY_FILE_SIZE`; skipped (event emitted).
    #[error("file too large: {bytes} bytes at {path}")]
    FileTooLarge {
        /// Path of the oversized file.
        path: PathBuf,
        /// Observed size in bytes.
        bytes: u64,
    },
}

/// Telemetry event name emitted when a file exceeds `MAX_MEMORY_FILE_SIZE`.
pub const TENGU_MEMORY_FILE_TOO_LARGE: &str = "tengu_memory_file_too_large";

/// Load one CLAUDE.md (or local override) with the 10 MB cap.
///
/// Returns `Err(FileTooLarge)` when the file exceeds the cap. The caller
/// is expected to log the event via [`emit_file_too_large`] and continue
/// processing the remaining files — oversized files are skipped, never
/// fatal.
///
/// # Errors
///
/// - [`LoaderError::Io`] for filesystem errors (file unreadable, perms).
/// - [`LoaderError::FileTooLarge`] when the size exceeds the cap.
pub fn load_file(
    path: &Path,
    _bus: Option<&Arc<telemetry::AnalyticsBus>>,
) -> Result<LoadedFile, LoaderError> {
    let meta = std::fs::metadata(path).map_err(|e| LoaderError::Io(e.to_string()))?;
    let size = meta.len();
    // On 32-bit targets a >4 GB file overflows `usize`; in that case it
    // is by definition over the 10 MB cap, so treat the conversion failure
    // as "too large" rather than rejecting it as an I/O error.
    let over_cap = match usize::try_from(size) {
        Ok(n) => n > MAX_MEMORY_FILE_SIZE,
        Err(_) => true,
    };
    if over_cap {
        return Err(LoaderError::FileTooLarge {
            path: path.to_path_buf(),
            bytes: size,
        });
    }
    let body = std::fs::read_to_string(path).map_err(|e| LoaderError::Io(e.to_string()))?;
    Ok(LoadedFile {
        path: path.to_path_buf(),
        body,
        size_bytes: size,
    })
}

/// Emit `tengu_memory_file_too_large` when a file was skipped.
///
/// No-op when `bus` is `None`. Payload keys (locked):
/// `_PROTO_path: PiiTagged(<path>)`, `size_bytes: Int(N)`.
pub async fn emit_file_too_large(
    bus: Option<&Arc<telemetry::AnalyticsBus>>,
    path: &Path,
    bytes: u64,
) {
    let Some(bus) = bus else {
        return;
    };
    let mut md = telemetry::sink::LogEventMetadata::new();
    md.insert(
        "_PROTO_path".into(),
        telemetry::sink::AnalyticsValue::String(
            telemetry::pii::PiiTagged::assert_pii_tagged_column(path.display().to_string())
                .into_inner(),
        ),
    );
    // Files this big are pathological — saturate rather than wrap so the
    // event still carries a sensible value if `bytes > i64::MAX`.
    let size_int = i64::try_from(bytes).unwrap_or(i64::MAX);
    md.insert(
        "size_bytes".into(),
        telemetry::sink::AnalyticsValue::Int(size_int),
    );
    bus.log_event(TENGU_MEMORY_FILE_TOO_LARGE, md).await;
}

// ---------------------------------------------------------------------------
// @import expansion + per-file body sanitisation
//
// Ports claude-code `src/utils/claudemd.ts`:
//   - processMemoryFile        (claudemd.ts:618-685) -> [`expand_memory_file`]
//   - parseMemoryFileContent   (claudemd.ts:343-400) -> [`parse_memory_content`]
//   - parseFrontmatterPaths    (claudemd.ts:254-279) -> [`strip_frontmatter`]
//   - stripHtmlCommentsFromTokens (claudemd.ts:303-334) -> [`strip_html_comments`]
//   - extractIncludePathsFromTokens (claudemd.ts:451-535) -> [`extract_include_paths`]
//   - expandPath               (path.ts:32-85)        -> [`expand_path`]
//
// FIDELITY BOUNDARY: the TS reference drives strip/extract off a full
// `marked` markdown lexer. To avoid a new dependency we use a line-oriented
// scanner that understands fenced code blocks (``` / ~~~) and inline code
// spans (`...`). Consequences vs. the lexer:
//   - 4-space *indented* code blocks are NOT recognised, so an @import or
//     `<!-- -->` inside one is treated as normal text.
//   - Emphasis/link markup wrapping a bare `@path` may leave trailing markup
//     in the captured path.
//   - Symlink resolution (safeResolvePath) is not performed; the cycle guard
//     compares lexically-normalised paths.
// These edges do not occur in any CLAUDE.md we ship and are documented here.
// ---------------------------------------------------------------------------

/// Maximum `@import` recursion depth (claude-code `MAX_INCLUDE_DEPTH`,
/// claudemd.ts:537). A node at `depth >= MAX_INCLUDE_DEPTH` is not processed,
/// so a chain expands at most `MAX_INCLUDE_DEPTH` files deep (depths 0..=4).
pub const MAX_INCLUDE_DEPTH: usize = 5;

/// One expanded memory file: a discovered CLAUDE.md or an `@import`'d file,
/// after frontmatter + HTML-comment stripping (NOT yet `.trim()`med).
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// The path this entry was loaded from (as passed in — display form).
    pub path: PathBuf,
    /// Sanitised body (frontmatter + block HTML comments removed).
    pub body: String,
}

/// Result of parsing one memory file's raw bytes.
#[derive(Debug, Clone)]
pub struct ParsedMemory {
    /// Body with a leading YAML frontmatter block and block-level HTML
    /// comments removed (code-fenced / inline-code content preserved).
    pub body: String,
    /// Resolved absolute paths of every `@import` directive found in leaf
    /// text (deduped, in first-seen order).
    pub include_paths: Vec<PathBuf>,
}

/// Recursively load `path` and every file it `@import`s, returning a flat
/// list with the parent FIRST, then each include's expansion appended in
/// directive order — matching claude-code `processMemoryFile`
/// (claudemd.ts:661-684: "Add the main file first (parent before children)").
///
/// - `processed`: shared cycle guard. A file is recorded (by lexically
///   normalised path) BEFORE it is read, so a file that imports an ancestor
///   is skipped (claudemd.ts:629-645).
/// - `include_external`: when `false`, includes resolving OUTSIDE `cwd` are
///   skipped (claudemd.ts:667-670). User memory passes `true`.
/// - `cwd`: original working dir used for the external-include gate.
/// - `home`: used to expand `@~/...`; `None` makes `~` imports resolve to
///   nothing and be skipped.
/// - `depth`: 0 for a top-level CLAUDE.md.
///
/// Missing / unreadable / oversized files are silently ignored (the ENOENT
/// branch of `safelyReadMemoryFileAsync`, claudemd.ts:433-436).
#[must_use]
pub fn expand_memory_file<S: std::hash::BuildHasher>(
    path: &Path,
    processed: &mut HashSet<PathBuf, S>,
    include_external: bool,
    cwd: &Path,
    home: Option<&Path>,
    depth: usize,
) -> Vec<MemoryEntry> {
    let key = lexical_normalize(path);
    // Skip if already processed or max depth exceeded (claudemd.ts:630).
    if depth >= MAX_INCLUDE_DEPTH || processed.contains(&key) {
        return Vec::new();
    }
    // Record before reading so cycles terminate even on read failure
    // (claudemd.ts:645).
    processed.insert(key);

    // Read with the 10 MB cap; any error (ENOENT, oversized, perms) => skip.
    let Ok(loaded) = load_file(path, None) else {
        return Vec::new();
    };

    let parsed = parse_memory_content(&loaded.body, path, home);
    // claudemd.ts:652 — drop whitespace-only files entirely.
    if parsed.body.trim().is_empty() {
        return Vec::new();
    }

    // Parent before children (claudemd.ts:663-664).
    let mut result = vec![MemoryEntry {
        path: path.to_path_buf(),
        body: parsed.body,
    }];

    for inc in parsed.include_paths {
        let is_external = !path_in_working_path(&inc, cwd);
        if is_external && !include_external {
            continue;
        }
        result.extend(expand_memory_file(
            &inc,
            processed,
            include_external,
            cwd,
            home,
            depth + 1,
        ));
    }

    result
}

/// Parse one memory file's raw content into its sanitised body and the set
/// of `@import` paths it references. Pure (no I/O). Mirrors
/// `parseMemoryFileContent` (claudemd.ts:343-400): frontmatter is stripped
/// first, then includes are extracted and HTML comments stripped from the
/// remainder.
#[must_use]
pub fn parse_memory_content(raw: &str, file_path: &Path, home: Option<&Path>) -> ParsedMemory {
    let without_fm = strip_frontmatter(raw);
    let base_dir = file_path.parent().unwrap_or_else(|| Path::new("."));
    let include_paths = extract_include_paths(without_fm, base_dir, home);
    let body = strip_html_comments(without_fm);
    ParsedMemory {
        body,
        include_paths,
    }
}

fn frontmatter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // claude-code FRONTMATTER_REGEX = /^---\s*\n([\s\S]*?)---\s*\n?/
    RE.get_or_init(|| Regex::new(r"(?s)^---\s*\n.*?---\s*\n?").unwrap())
}

/// Strip a leading `---`…`---` YAML frontmatter block, returning the content
/// past it (claude-code `parseFrontmatter` / `FRONTMATTER_REGEX`,
/// frontmatterParser.ts:123-145). When no frontmatter is present the input is
/// returned unchanged.
#[must_use]
pub fn strip_frontmatter(raw: &str) -> &str {
    if let Some(m) = frontmatter_re().find(raw) {
        if m.start() == 0 {
            return &raw[m.end()..];
        }
    }
    raw
}

fn comment_span_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // claude-code commentSpan = /<!--[\s\S]*?-->/g
    RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap())
}

fn strip_comment_spans(s: &str) -> String {
    comment_span_re().replace_all(s, "").into_owned()
}

/// Strip block-level `<!-- ... -->` HTML comments, preserving comments inside
/// fenced code blocks and inline code spans. Mirrors
/// `stripHtmlCommentsFromTokens` (claudemd.ts:303-334): a comment that begins
/// a line (≤3 leading spaces) is removed up to and including the line that
/// contains `-->`; any residue on that closing line is kept. Unclosed
/// comments and mid-line (inline) comments are left in place.
#[must_use]
pub fn strip_html_comments(content: &str) -> String {
    if !content.contains("<!--") {
        return content.to_string();
    }
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut out = String::with_capacity(content.len());
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let probe = raw.trim_end_matches(['\r', '\n']);

        if let Some((fc, fl)) = fence {
            if is_closing_fence(probe, fc, fl) {
                fence = None;
            }
            out.push_str(raw);
            i += 1;
            continue;
        }
        if let Some((c, n)) = opening_fence(probe) {
            fence = Some((c, n));
            out.push_str(raw);
            i += 1;
            continue;
        }

        if strip_leading_spaces_max3(probe).starts_with("<!--") {
            // Find the line that closes the comment block.
            let mut j = i;
            let mut found = false;
            while j < lines.len() {
                if lines[j].trim_end_matches(['\r', '\n']).contains("-->") {
                    found = true;
                    break;
                }
                j += 1;
            }
            if !found {
                // Unclosed comment — leave the remainder verbatim
                // (claudemd.ts:317 requires a matching `-->`).
                for line in &lines[i..] {
                    out.push_str(line);
                }
                break;
            }
            let mut block = String::new();
            for line in &lines[i..=j] {
                block.push_str(line);
            }
            let residue = strip_comment_spans(&block);
            if !residue.trim().is_empty() {
                out.push_str(&residue);
            }
            i = j + 1;
            continue;
        }

        out.push_str(raw);
        i += 1;
    }
    out
}

fn include_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // claude-code includeRegex = /(?:^|\s)@((?:[^\s\\]|\\ )+)/g
    RE.get_or_init(|| Regex::new(r"(?:^|\s)@((?:[^\s\\]|\\ )+)").unwrap())
}

/// Extract `@import` paths from leaf text and resolve them to absolute paths
/// relative to `base_dir`. Ports `extractIncludePathsFromTokens`
/// (claudemd.ts:451-535): code spans / fenced blocks are skipped, block HTML
/// comments contribute only their residue, `#fragment` suffixes are dropped,
/// `\ ` escapes are unescaped, and the accept rules at claudemd.ts:476-489 are
/// applied before resolution. Results are deduped in first-seen order.
#[must_use]
pub fn extract_include_paths(content: &str, base_dir: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let scannable = build_scannable_text(content);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for caps in include_re().captures_iter(&scannable) {
        let raw = caps.get(1).map_or("", |m| m.as_str());
        // Strip fragment identifiers (#heading) — claudemd.ts:466-469.
        let mut path = match raw.find('#') {
            Some(idx) => raw[..idx].to_string(),
            None => raw.to_string(),
        };
        if path.is_empty() {
            continue;
        }
        // Unescape spaces — claudemd.ts:473.
        path = path.replace("\\ ", " ");
        if path.is_empty() || !is_valid_include_path(&path) {
            continue;
        }
        if let Some(resolved) = expand_path(&path, base_dir, home) {
            if seen.insert(resolved.clone()) {
                out.push(resolved);
            }
        }
    }
    out
}

/// Accept rules for a candidate `@import` path (claudemd.ts:476-489).
fn is_valid_include_path(path: &str) -> bool {
    if path.starts_with("./") || path.starts_with("~/") {
        return true;
    }
    if path.starts_with('/') && path != "/" {
        return true;
    }
    // Bareword: must start with [a-zA-Z0-9._-] (which also excludes a leading
    // '@' and the punctuation class /^[#%^&*()]+/).
    matches!(
        path.chars().next(),
        Some(c) if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
    )
}

/// Expand a path that may use `~`, `~/`, an absolute prefix, or be relative to
/// `base_dir`. Ports `expandPath` (path.ts:32-85), minus the Windows POSIX
/// conversion. Returns `None` only when a `~`/`~/` path is given but `home`
/// is `None`.
#[must_use]
pub fn expand_path(path: &str, base_dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Some(lexical_normalize(base_dir));
    }
    if trimmed == "~" {
        return home.map(lexical_normalize);
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home.map(|h| lexical_normalize(&h.join(rest)));
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return Some(lexical_normalize(p));
    }
    Some(lexical_normalize(&base_dir.join(p)))
}

/// Build the text over which `@import` extraction runs: fenced code blocks are
/// dropped, inline code spans are removed, and block HTML comments contribute
/// only their residue (claudemd.ts:494-531). Line boundaries are preserved so
/// the leading `(?:^|\s)` of the include regex still matches at line starts.
fn build_scannable_text(content: &str) -> String {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut buf = String::with_capacity(content.len());
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let probe = lines[i].trim_end_matches(['\r', '\n']);

        if let Some((fc, fl)) = fence {
            if is_closing_fence(probe, fc, fl) {
                fence = None;
            }
            buf.push('\n');
            i += 1;
            continue;
        }
        if let Some((c, n)) = opening_fence(probe) {
            fence = Some((c, n));
            buf.push('\n');
            i += 1;
            continue;
        }

        if strip_leading_spaces_max3(probe).starts_with("<!--") {
            let mut j = i;
            let mut found = false;
            while j < lines.len() {
                if lines[j].trim_end_matches(['\r', '\n']).contains("-->") {
                    found = true;
                    break;
                }
                j += 1;
            }
            if !found {
                // Unclosed comment block — skipped entirely for extraction.
                for _ in i..lines.len() {
                    buf.push('\n');
                }
                break;
            }
            let mut block = String::new();
            for line in &lines[i..=j] {
                block.push_str(line);
            }
            // Only the residue outside the comment spans can carry @paths.
            let residue = strip_comment_spans(&block);
            buf.push_str(&residue);
            if !residue.ends_with('\n') {
                buf.push('\n');
            }
            i = j + 1;
            continue;
        }

        buf.push_str(&strip_inline_code(probe));
        buf.push('\n');
        i += 1;
    }
    buf
}

/// Remove inline code spans (`` `...` ``) from a single line so `@paths`
/// inside them are ignored (claudemd.ts:496-498 skips `codespan`).
fn strip_inline_code(line: &str) -> String {
    if !line.contains('`') {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let start = i;
            let mut open = 0;
            while i < chars.len() && chars[i] == '`' {
                open += 1;
                i += 1;
            }
            // Find a closing run of exactly `open` backticks.
            let mut k = i;
            let mut close_end = None;
            while k < chars.len() {
                if chars[k] == '`' {
                    let run_start = k;
                    let mut run = 0;
                    while k < chars.len() && chars[k] == '`' {
                        run += 1;
                        k += 1;
                    }
                    if run == open {
                        close_end = Some(k);
                        break;
                    }
                    let _ = run_start;
                } else {
                    k += 1;
                }
            }
            match close_end {
                Some(end) => i = end, // drop the whole span
                None => {
                    // No closing run — keep the backticks literally.
                    out.extend(&chars[start..i]);
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Detect an opening fence line: ≤3 leading spaces then a run of ≥3 `` ` `` or
/// `~`. Returns the fence char and run length.
fn opening_fence(probe: &str) -> Option<(char, usize)> {
    if count_leading_spaces(probe) > 3 {
        return None;
    }
    let rest = strip_leading_spaces_max3(probe);
    let c = rest.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let run = rest.chars().take_while(|&x| x == c).count();
    if run >= 3 {
        Some((c, run))
    } else {
        None
    }
}

/// A closing fence: same char, run length ≥ the opening run, nothing but
/// whitespace after the run.
fn is_closing_fence(probe: &str, fence_char: char, fence_len: usize) -> bool {
    if count_leading_spaces(probe) > 3 {
        return false;
    }
    let rest = strip_leading_spaces_max3(probe);
    let run = rest.chars().take_while(|&x| x == fence_char).count();
    if run < fence_len {
        return false;
    }
    rest.chars().skip(run).collect::<String>().trim().is_empty()
}

fn count_leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn strip_leading_spaces_max3(s: &str) -> &str {
    let n = count_leading_spaces(s).min(3);
    &s[n..]
}

/// Lexically normalise a path (resolve `.` / `..` components) WITHOUT touching
/// the filesystem — used for the cycle guard and the external-include gate.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when `path` is `working` or lives under it. Simplified
/// `pathInWorkingPath` (filesystem.ts:709): lexical containment only — the
/// `/private` symlink and case-fold normalisations are omitted (boundary).
fn path_in_working_path(path: &Path, working: &Path) -> bool {
    let p = lexical_normalize(path);
    let w = lexical_normalize(working);
    p == w || p.starts_with(&w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_MEMORY_FILE_SIZE;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn loads_small_file_into_loadedfile() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        fs::write(&p, b"# notes\nhello\n").unwrap();
        let out = load_file(&p, None).unwrap();
        assert_eq!(out.path, p);
        assert!(out.body.contains("hello"));
        assert_eq!(out.size_bytes, 14);
    }

    #[test]
    fn skips_oversized_file_returns_file_too_large() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        let bytes = vec![b'a'; MAX_MEMORY_FILE_SIZE + 1];
        fs::write(&p, &bytes).unwrap();
        match load_file(&p, None) {
            Err(LoaderError::FileTooLarge { path, bytes }) => {
                assert_eq!(path, p);
                assert_eq!(bytes, u64::try_from(MAX_MEMORY_FILE_SIZE + 1).unwrap());
            }
            other => panic!("expected FileTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn file_at_exact_cap_is_loaded() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        fs::write(&p, vec![b'a'; MAX_MEMORY_FILE_SIZE]).unwrap();
        let out = load_file(&p, None).unwrap();
        assert_eq!(out.size_bytes, u64::try_from(MAX_MEMORY_FILE_SIZE).unwrap());
    }

    #[tokio::test]
    async fn emit_file_too_large_writes_event_with_size() {
        use std::sync::{Arc, Mutex};
        use telemetry::{sink::LogEventMetadata, AnalyticsBus, AnalyticsSink, AnalyticsValue};

        struct Cap {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait::async_trait]
        impl AnalyticsSink for Cap {
            async fn log_event(&self, n: &str, m: LogEventMetadata) {
                self.events.lock().unwrap().push((n.into(), m));
            }
            async fn log_event_async(&self, n: &str, m: LogEventMetadata) {
                self.log_event(n, m).await;
            }
            fn name(&self) -> &str {
                "cap"
            }
        }

        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(Cap {
            events: Mutex::new(Vec::new()),
        });
        bus.attach_sink(sink.clone()).await;
        emit_file_too_large(
            Some(&bus),
            std::path::Path::new("/x/CLAUDE.md"),
            11 * 1024 * 1024,
        )
        .await;
        let ev = sink.events.lock().unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "tengu_memory_file_too_large");
        matches!(ev[0].1.get("size_bytes"), Some(AnalyticsValue::Int(_)));
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    fn bodies(entries: &[MemoryEntry]) -> Vec<String> {
        entries.iter().map(|e| e.body.trim().to_string()).collect()
    }

    #[test]
    fn imports_relative_tilde_and_abs_spliced_as_entries() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("repo");
        let home = tmp.path().join("home");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();

        // `@./rel.md` lives under cwd; `@~/tilde.md` under home; the abs
        // import lives at the tmp root (outside cwd).
        fs::write(cwd.join("rel.md"), "REL").unwrap();
        fs::write(home.join("tilde.md"), "TILDE").unwrap();
        let abs = tmp.path().join("abs.md");
        fs::write(&abs, "ABS").unwrap();

        let main = cwd.join("CLAUDE.md");
        fs::write(
            &main,
            format!("main notes\n@./rel.md\n@~/tilde.md\n@{}\n", abs.display()),
        )
        .unwrap();

        let mut processed = HashSet::new();
        // include_external=true so the home/abs imports are not gated out.
        let entries = expand_memory_file(&main, &mut processed, true, &cwd, Some(&home), 0);

        // Parent FIRST, then children in directive order.
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].path, main, "parent must come before children");
        assert!(entries[0].body.contains("main notes"));
        let child_bodies: Vec<String> = bodies(&entries[1..]);
        assert_eq!(child_bodies, vec!["REL", "TILDE", "ABS"]);
    }

    #[test]
    fn cycle_guard_terminates_mutual_imports() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("a.md"), "A\n@./b.md\n").unwrap();
        fs::write(dir.join("b.md"), "B\n@./a.md\n").unwrap();

        let mut processed = HashSet::new();
        let entries =
            expand_memory_file(&dir.join("a.md"), &mut processed, true, dir, Some(dir), 0);

        // a -> b -> (a already processed, skipped). Exactly two entries.
        assert_eq!(entries.len(), 2);
        assert!(entries[0].body.contains('A'));
        assert!(entries[1].body.contains('B'));
    }

    #[test]
    fn depth_cap_stops_at_five() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        // f0 -> f1 -> ... -> f6 (each imports the next).
        for n in 0..=6 {
            let body = if n < 6 {
                format!("F{n}\n@./f{}.md\n", n + 1)
            } else {
                format!("F{n}\n")
            };
            fs::write(dir.join(format!("f{n}.md")), body).unwrap();
        }

        let mut processed = HashSet::new();
        let entries =
            expand_memory_file(&dir.join("f0.md"), &mut processed, true, dir, Some(dir), 0);

        // Depths 0..=4 are processed (5 files); f5 would be depth 5 -> rejected.
        // The body keeps the `@import` directive text (parity: TS only strips
        // frontmatter + comments), so check the leading marker per entry.
        assert_eq!(entries.len(), MAX_INCLUDE_DEPTH);
        for (n, e) in entries.iter().enumerate() {
            assert!(e.body.trim_start().starts_with(&format!("F{n}")));
        }
        assert!(
            !bodies(&entries).iter().any(|b| b.contains("F5") || b.contains("F6")),
            "files past the depth cap must not be spliced"
        );
    }

    #[test]
    fn missing_import_is_silently_ignored() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let main = dir.join("CLAUDE.md");
        fs::write(&main, "hello\n@./does-not-exist.md\n").unwrap();

        let mut processed = HashSet::new();
        let entries = expand_memory_file(&main, &mut processed, true, dir, Some(dir), 0);

        // Only the parent — the missing include is dropped without error.
        assert_eq!(entries.len(), 1);
        assert!(entries[0].body.contains("hello"));
    }

    #[test]
    fn external_import_gated_unless_allowed() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        let outside = tmp.path().join("outside.md");
        fs::write(&outside, "OUTSIDE").unwrap();
        let main = cwd.join("CLAUDE.md");
        fs::write(&main, format!("root\n@{}\n", outside.display())).unwrap();

        // Gated out when external includes are disallowed.
        let mut p1 = HashSet::new();
        let gated = expand_memory_file(&main, &mut p1, false, &cwd, Some(&cwd), 0);
        assert_eq!(gated.len(), 1, "external include must be skipped");

        // Allowed when include_external=true.
        let mut p2 = HashSet::new();
        let allowed = expand_memory_file(&main, &mut p2, true, &cwd, Some(&cwd), 0);
        assert_eq!(allowed.len(), 2);
        assert!(allowed[1].body.contains("OUTSIDE"));
    }

    #[test]
    fn frontmatter_block_is_stripped_from_body() {
        let raw = "---\ntitle: secret\npaths: src/**\n---\nVISIBLE BODY\n";
        let parsed = parse_memory_content(raw, Path::new("/x/CLAUDE.md"), None);
        assert_eq!(parsed.body, "VISIBLE BODY\n");
        assert!(!parsed.body.contains("title"));
    }

    #[test]
    fn block_html_comment_stripped_but_code_fence_preserved() {
        let content = "\
before
<!-- authorial note, should vanish -->
after
```
<!-- fenced comment must survive -->
```
done
";
        let stripped = strip_html_comments(content);
        assert!(
            !stripped.contains("authorial note"),
            "block comment must be stripped: {stripped:?}"
        );
        assert!(stripped.contains("before"));
        assert!(stripped.contains("after"));
        assert!(
            stripped.contains("fenced comment must survive"),
            "code-fenced comment must be preserved: {stripped:?}"
        );
    }

    #[test]
    fn comment_residue_after_close_is_kept() {
        // claudemd.ts:320-326 — text after `-->` on the same line is kept.
        let content = "<!-- note --> @./keep.md\n";
        let stripped = strip_html_comments(content);
        assert!(stripped.contains("@./keep.md"));
        assert!(!stripped.contains("note"));
    }

    #[test]
    fn import_inside_fence_or_inline_code_is_not_extracted() {
        let content = "\
@./real.md
```
@./fenced.md
```
inline `@./inline.md` here
";
        let paths = extract_include_paths(content, Path::new("/base"), None);
        assert_eq!(paths, vec![PathBuf::from("/base/real.md")]);
    }

    #[test]
    fn expand_path_handles_tilde_abs_and_relative() {
        let home = Path::new("/home/u");
        let base = Path::new("/proj/sub");
        assert_eq!(
            expand_path("~/notes.md", base, Some(home)),
            Some(PathBuf::from("/home/u/notes.md"))
        );
        assert_eq!(
            expand_path("/abs/x.md", base, Some(home)),
            Some(PathBuf::from("/abs/x.md"))
        );
        assert_eq!(
            expand_path("./rel.md", base, Some(home)),
            Some(PathBuf::from("/proj/sub/rel.md"))
        );
        assert_eq!(
            expand_path("bare.md", base, Some(home)),
            Some(PathBuf::from("/proj/sub/bare.md"))
        );
        // `~` with no home resolves to nothing.
        assert_eq!(expand_path("~/x.md", base, None), None);
    }
}
