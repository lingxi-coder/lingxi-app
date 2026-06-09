//! `FileReadTool` — read a UTF-8 file with size guard + binary detection.
//!
//! Wire-locked constants:
//! - `MAX_FILE_READ_SIZE = 262_144` bytes (256 KB) per spec §7.
//! - Binary detection scans first `NUL_SCAN_WINDOW = 8 * 1024` bytes for NUL.
//! - Default encoding UTF-8 (BOM-aware) per spec §7.
//! - 1-based line indexing on tool input/output per spec §7.
//! - Errors carry byte-locked human strings (see [`format_too_large`],
//!   [`format_binary`]).

use crate::shared::{decode_utf8_strict, looks_binary, NUL_SCAN_WINDOW};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    FILE_READ_DEDUP, FILE_READ_LIMITS_OVERRIDE, READ_COMPLETED, READ_FAILED, READ_STARTED,
    SESSION_FILE_READ,
};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::ids::ulid_or_uuid;
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

/// Maximum file size FileReadTool will load. Spec §7 lock (256 KB).
pub const MAX_FILE_READ_SIZE: u64 = 262_144;

/// Default per-read output token budget — byte-locked to claude-code
/// `DEFAULT_MAX_OUTPUT_TOKENS` (`FileReadTool/limits.ts:18`). A full text read
/// whose estimated token count exceeds this errors with
/// [`format_max_tokens_exceeded`]. LingXi has no GrowthBook / env override and
/// `ToolUseContext` carries no `fileReadingLimits`, so the effective budget is
/// always this default (the `tengu_amber_wren` GB override + the
/// `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS` env tier are unported — 3P default).
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 25_000;

/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Read";

/// True for the FileRead image extensions (png/jpg/jpeg/gif/webp). Defined here
/// (not via `image_read`) so it compiles when `image-read` is off.
fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// `.pdf` (compiles with `pdf-read` off). Routes to the document path. Defined
/// here (not via `pdf_read`) so the gate / routing logic compiles in both
/// feature states; `pdf_read::is_pdf_path` is the same predicate.
fn is_pdf_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        == Some("pdf")
}

/// Bytes-per-token ratio for the rough local token estimate, keyed by file
/// extension — 1:1 with `bytesPerTokenForFileType` (`tokenEstimation.ts:215-224`).
/// Dense JSON has many single-char tokens, so its real ratio is ~2 not 4.
#[must_use]
fn bytes_per_token_for_file_type(ext: Option<&str>) -> u64 {
    match ext {
        Some("json" | "jsonl" | "jsonc") => 2,
        _ => 4,
    }
}

/// `roughTokenCountEstimationForFileType(content, ext)` — `Math.round(len /
/// bytesPerToken)` (`tokenEstimation.ts:203-242`). Round-half-up over a
/// nonnegative byte length is `(len + bpt/2) / bpt`. Uses the UTF-8 byte length
/// (TS uses the UTF-16 string `.length`); identical for ASCII, the same
/// convention the microcompact / mcp-output ports already use.
#[must_use]
fn rough_token_count_estimation_for_file_type(content: &str, ext: Option<&str>) -> u64 {
    let bpt = bytes_per_token_for_file_type(ext);
    u64::try_from(content.len())
        .unwrap_or(u64::MAX)
        .saturating_add(bpt / 2)
        / bpt
}

/// Build the byte-locked max-tokens-exceeded error — 1:1 with
/// `MaxFileReadTokenExceededError` (`FileReadTool.ts:175-185`).
#[must_use]
pub fn format_max_tokens_exceeded(token_count: u64, max_tokens: u64) -> String {
    format!(
        "File content ({token_count} tokens) exceeds maximum allowed tokens ({max_tokens}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
    )
}

/// Local-estimate token gate — `validateContentTokens(content, ext, maxTokens)`
/// (`FileReadTool.ts:755-772`). TS computes `tokenEstimate =
/// roughTokenCountEstimationForFileType(content, ext)`; early-passes when
/// `!tokenEstimate || tokenEstimate <= maxTokens/4`; otherwise refines with the
/// count_tokens API as `effectiveCount = apiCount ?? tokenEstimate` and errors
/// when `effectiveCount > maxTokens`.
///
/// The count_tokens API client is not reachable from the tool context, so this
/// ports TS's own offline fallback exactly: `apiCount` is `None`, hence
/// `effectiveCount == tokenEstimate`, and the gate errors iff
/// `tokenEstimate > maxTokens`. The `maxTokens/4` early-pass band still admits
/// reads in `(maxTokens/4, maxTokens]` (TS would API-refine them, but its
/// `?? tokenEstimate` fallback also admits them when the API is unavailable),
/// so behavior is identical to claude-code running offline / on a provider
/// without count_tokens. Returns the byte-locked error string on overflow.
fn validate_content_tokens(content: &str, ext: Option<&str>, max_tokens: u64) -> Result<(), String> {
    let token_estimate = rough_token_count_estimation_for_file_type(content, ext);
    if token_estimate == 0 || token_estimate <= max_tokens / 4 {
        return Ok(());
    }
    // No reachable count_tokens API → `effectiveCount = tokenEstimate`.
    if token_estimate > max_tokens {
        return Err(format_max_tokens_exceeded(token_estimate, max_tokens));
    }
    Ok(())
}

/// Build the byte-locked too-large error message per spec §5.
#[must_use]
pub fn format_too_large(path: &std::path::Path, size: u64) -> String {
    format!(
        "File {} ({}B) exceeds 256KB read limit",
        path.display(),
        size
    )
}

/// Build the byte-locked binary-file error message per spec §5.
#[must_use]
pub fn format_binary(path: &std::path::Path) -> String {
    format!(
        "File {} appears to be binary (first 8KB contains NUL bytes)",
        path.display()
    )
}

/// `MAX_FILE_EXTENSION_LENGTH` — byte-locked to claude-code
/// (`services/analytics/metadata.ts:311`). Extensions longer than this bucket to
/// `"other"` so analytics never leaks a long, potentially-identifying suffix.
const MAX_FILE_EXTENSION_LENGTH: usize = 10;

/// `getFileExtensionForAnalytics(filePath)` — 1:1 with
/// `services/analytics/metadata.ts:323-337`. Returns the lowercased extension
/// without the leading dot, `Some("other")` when it exceeds
/// [`MAX_FILE_EXTENSION_LENGTH`], and `None` when the path has no extension (TS
/// `undefined` for empty / `"."`). The analytics ext is intentionally distinct
/// from the slicing `ext` used elsewhere: TS sources the `tengu_*` `ext` metadata
/// from this helper (the `AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS`
/// type), not from the raw `path.extname` used for token estimation.
#[must_use]
fn get_file_extension_for_analytics(path: &std::path::Path) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    if ext.is_empty() {
        return None;
    }
    let lower = ext.to_ascii_lowercase();
    if lower.len() > MAX_FILE_EXTENSION_LENGTH {
        return Some("other".to_string());
    }
    Some(lower)
}

/// `detectSessionFileType(filePath)` — 1:1 with
/// `utils/memoryFileDetection.ts:40-59`. Returns `("session_memory")` when the
/// path is under `<configHome>/.../session-memory/*.md`, `("session_transcript")`
/// when under `<configHome>/.../projects/*.jsonl`, else `None`. The path is
/// compared in forward-slash form against the resolved config home
/// (`$CLAUDE_CONFIG_DIR ?? $HOME/.claude`, mirroring `getClaudeConfigHomeDir()`).
/// On POSIX (the port target) `toComparable` is a no-op beyond separator
/// normalization (no case-folding); Windows case-folding is not reproduced here.
// The `.md` / `.jsonl` suffix checks are a faithful 1:1 port of TS's
// case-SENSITIVE `normalized.endsWith('.md')` / `endsWith('.jsonl')` (TS only
// case-folds on Windows, via `toComparable`, which this POSIX port does not do).
// A case-insensitive comparison would change the matching semantics, so the
// `case_sensitive_file_extension_comparisons` lint is intentionally suppressed.
#[must_use]
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn detect_session_file_type(path: &std::path::Path) -> Option<&'static str> {
    let config_dir = claude_config_home_dir();
    let normalized = to_comparable(path);
    let config_cmp = to_comparable(&config_dir);
    if !normalized.starts_with(&config_cmp) {
        return None;
    }
    if normalized.contains("/session-memory/") && normalized.ends_with(".md") {
        return Some("session_memory");
    }
    if normalized.contains("/projects/") && normalized.ends_with(".jsonl") {
        return Some("session_transcript");
    }
    None
}

/// Forward-slash form of a path (`toPosix`/`toComparable` on POSIX —
/// `utils/memoryFileDetection.ts:25-34`). No case-folding (POSIX target).
fn to_comparable(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Port of `getClaudeConfigHomeDir()` (`envUtils.ts:7-13`): `$CLAUDE_CONFIG_DIR`
/// when set+non-empty, else `$HOME/.claude` (falling back to `USERPROFILE` then a
/// bare `.claude`). Mirrors the existing ports in `tools/task` / `commands/core`.
fn claude_config_home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home).join(".claude"),
        None => PathBuf::from(".claude"),
    }
}

/// Cyber-risk mitigation reminder appended to the model-facing text of a
/// successful file read — byte-locked to claude-code (`FileReadTool.ts:729-730`).
/// Two leading `\n` separate it from the file body; one trailing `\n` closes it.
/// Skipped for models in [`MITIGATION_EXEMPT_MODELS`]. The TUI never shows this
/// (`FileReadTool.ts:409-413`: UI renders summary chrome only) — hence it lives
/// in the model-only `model_content` field, not the TUI-facing `content`.
pub const CYBER_RISK_MITIGATION_REMINDER: &str = "\n\n<system-reminder>\nWhenever you read a file, you should consider whether it would be considered malware. You CAN and SHOULD provide analysis of malware, what it is doing. But you MUST refuse to improve or augment the code. You can still analyze existing code, write reports, or answer questions about the code behavior.\n</system-reminder>\n";

/// Model-facing stub for the Read dedup (`file_unchanged`) case — byte-locked to
/// claude-code (`FileReadTool/prompt.ts:7-8`). The dedup decision itself (compare
/// the prior read's mtime + range from the read-file-state registry and
/// short-circuit) is a later batch; this constant locks the string the model
/// will see when that lands.
pub const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read tool_result in this conversation is still current — refer to that instead of re-reading.";

/// Model-facing warning when a read targets an existing but empty file —
/// byte-locked to claude-code (`FileReadTool.ts:705-706`).
pub const EMPTY_FILE_WARNING: &str =
    "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>";

/// Models for which the cyber-risk mitigation reminder is skipped — byte-locked
/// to claude-code (`FileReadTool.ts:733`). NOTE: claude-code canonicalizes the
/// model name (`getCanonicalName`) before this set lookup; LingXi compares the
/// raw `main_loop_model`, so only the already-canonical form matches — a
/// documented, behavior-neutral divergence (LingXi's models are not in this set).
pub const MITIGATION_EXEMPT_MODELS: &[&str] = &["claude-opus-4-6"];

/// Whether to append [`CYBER_RISK_MITIGATION_REMINDER`] for `model` — mirrors
/// `shouldIncludeFileReadMitigation()` (`FileReadTool.ts:735-738`).
#[must_use]
pub fn should_include_file_read_mitigation(model: &str) -> bool {
    !MITIGATION_EXEMPT_MODELS.contains(&model)
}

/// Build the model-facing offset-beyond-EOF warning — byte-locked to claude-code
/// (`FileReadTool.ts:707`). `offset` is the requested 1-based start line
/// (`data.file.startLine`); `total_lines` is TS `readFileInRange.totalLines`
/// (newline count + 1; see the FILE.4 note at the call site).
#[must_use]
pub fn format_offset_beyond_eof(offset: u64, total_lines: u64) -> String {
    format!(
        "<system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {total_lines} lines.</system-reminder>"
    )
}

/// `cat -n` line numbering for the model-facing read output — 1:1 with
/// claude-code's compact-format `addLineNumbers` (`utils/file.ts:290-319`,
/// killswitch off = the current default). Each line becomes `{n}\t{line}` where
/// `n` counts up from `start_line` (1-based); lines are joined by `\n`. Empty
/// content yields `""`. Splitting mirrors the TS `/\r?\n/` regex (a trailing
/// `\r` is stripped per line). The legacy padded-arrow format
/// (`String(n).padStart(6, ' ') + "→"`) only applied with the killswitch on and
/// is intentionally not ported.
#[must_use]
pub fn add_line_numbers(content: &str, start_line: u64) -> String {
    if content.is_empty() {
        return String::new();
    }
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .zip(start_line..)
        .map(|(line, n)| format!("{n}\t{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `FileReadTool` — reads a UTF-8 file inside the trusted-dirs whitelist.
pub struct FileReadTool {
    ctx: BuiltinToolContext,
}

impl FileReadTool {
    /// Construct a new tool. Cheap — only clones the shared `Arc`s.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str, path: &std::path::Path) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "_PROTO_file_path".to_string(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(path.display().to_string()).into_inner(),
            ),
        );
        self.ctx.bus.log_event(READ_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, bytes_read: u64, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "bytes_read".to_string(),
            AnalyticsValue::Int(bytes_read as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(READ_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, failure_kind: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "failure_kind".to_string(),
            AnalyticsValue::String(Verified::assert_safe(failure_kind.to_string()).into_inner()),
        );
        self.ctx.bus.log_event(READ_FAILED, md).await;
    }

    /// `tengu_file_read_dedup` (`FileReadTool.ts:559-561`) — fired in the dedup
    /// short-circuit (the `file_unchanged` path). Metadata: `ext` ONLY when
    /// present (TS spreads `...(analyticsExt !== undefined && { ext })`); the
    /// analytics ext comes from [`get_file_extension_for_analytics`].
    async fn emit_file_read_dedup(&self, path: &std::path::Path) {
        let mut md: LogEventMetadata = HashMap::new();
        if let Some(ext) = get_file_extension_for_analytics(path) {
            md.insert(
                "ext".to_string(),
                AnalyticsValue::String(Verified::assert_safe(ext).into_inner()),
            );
        }
        self.ctx.bus.log_event(FILE_READ_DEDUP, md).await;
    }

    /// `tengu_session_file_read` (`FileReadTool.ts:1069-1083`) — fired after a
    /// successful TEXT read. Metadata mirrors TS exactly:
    /// `totalLines`/`readLines`/`totalBytes`/`readBytes`/`offset` (int, always),
    /// `limit`/`ext`/`messageID` (only-when-present), and the two session-file
    /// booleans. `messageID` is sourced from the assistant message in TS
    /// (`parentMessage`), which has no analog reachable from the Rust tool
    /// context — so it is faithfully OMITTED (matching TS's `undefined` spread).
    #[allow(clippy::too_many_arguments)]
    async fn emit_session_file_read(
        &self,
        path: &std::path::Path,
        total_lines: u64,
        read_lines: u64,
        total_bytes: u64,
        read_bytes: u64,
        offset: u64,
        limit: Option<u64>,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("totalLines".to_string(), AnalyticsValue::Int(total_lines as i64));
        md.insert("readLines".to_string(), AnalyticsValue::Int(read_lines as i64));
        md.insert("totalBytes".to_string(), AnalyticsValue::Int(total_bytes as i64));
        md.insert("readBytes".to_string(), AnalyticsValue::Int(read_bytes as i64));
        md.insert("offset".to_string(), AnalyticsValue::Int(offset as i64));
        if let Some(l) = limit {
            md.insert("limit".to_string(), AnalyticsValue::Int(l as i64));
        }
        if let Some(ext) = get_file_extension_for_analytics(path) {
            md.insert(
                "ext".to_string(),
                AnalyticsValue::String(Verified::assert_safe(ext).into_inner()),
            );
        }
        // messageID: TS spreads `...(messageId !== undefined && { messageID })`.
        // No assistant-message id is reachable from the Rust tool context, so the
        // key is faithfully omitted (the TS-undefined branch).
        let session_type = detect_session_file_type(path);
        md.insert(
            "is_session_memory".to_string(),
            AnalyticsValue::Bool(session_type == Some("session_memory")),
        );
        md.insert(
            "is_session_transcript".to_string(),
            AnalyticsValue::Bool(session_type == Some("session_transcript")),
        );
        self.ctx.bus.log_event(SESSION_FILE_READ, md).await;
    }

    /// `tengu_file_read_limits_override` (`FileReadTool.ts:512-515`) — fires only
    /// when `fileReadingLimits !== undefined`. The Rust `ToolUseContext` carries
    /// NO `fileReadingLimits` field, so this condition is never reachable in the
    /// port: the event name is registered, but the emit is a documented no-op
    /// (a faithful port of a never-true branch — no invented source). When TS
    /// fires it, the metadata is `hasMaxTokens`/`hasMaxSizeBytes` (bool); this
    /// helper accepts those for shape-parity even though no call site supplies a
    /// `Some(limits)`.
    async fn emit_file_read_limits_override(&self, limits: Option<(bool, bool)>) {
        // TS: `if (fileReadingLimits !== undefined) { logEvent(...) }`.
        let Some((has_max_tokens, has_max_size_bytes)) = limits else {
            // Unreachable in the port — `fileReadingLimits` is always `None`.
            return;
        };
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "hasMaxTokens".to_string(),
            AnalyticsValue::Bool(has_max_tokens),
        );
        md.insert(
            "hasMaxSizeBytes".to_string(),
            AnalyticsValue::Bool(has_max_size_bytes),
        );
        self.ctx.bus.log_event(FILE_READ_LIMITS_OVERRIDE, md).await;
    }

    /// Process an image file and return it as multimodal content. The pixels ride
    /// on `new_messages` as a `ContentBlock::Image` (the frozen tool-result content
    /// is text-only); the tool-result text is a short placeholder. Mirrors
    /// claude-code FileRead's image path (the model-facing image block + an
    /// optional `[Image: original …]` metadata message when the image was resized).
    #[cfg(feature = "image-read")]
    async fn read_image_result(
        &self,
        invocation_id: &str,
        canon: &std::path::Path,
        bytes: Vec<u8>,
        original_size: u64,
        started: Instant,
    ) -> Result<ToolCallResult, ToolError> {
        let processed = match crate::image_read::process_image(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.emit_failed(invocation_id, "image_process").await;
                return Err(ToolError::Io(e));
            }
        };

        let source = protocol::ImageSource::Base64 {
            media_type: processed.media_type.clone(),
            data: processed.base64,
        };
        let text = match processed.resized {
            Some((ow, oh, dw, dh)) => {
                let scale = f64::from(ow) / f64::from(dw.max(1));
                format!(
                    "[Image: original {ow}x{oh}, displayed at {dw}x{dh}. Multiply coordinates by {scale:.2} to map to original image.]"
                )
            }
            None => String::new(),
        };
        let msg = protocol::ConversationMessage::user_with_images(
            protocol::MessageId::new(),
            text,
            vec![source],
        );

        // MIRROR the text-read success path's completion telemetry
        // (`emit_completed(invocation_id, bytes_read, duration_ms)` at the
        // `tengu_tool_read_completed` site). The image branch has no line/byte
        // slice counts (`emit_session_file_read` is text-only — TS fires
        // `tengu_session_file_read` only after a text read, not on images), so
        // we emit the simpler completion event with the original file size as the
        // `bytes_read` figure — telemetry is recorded, not silently skipped.
        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(invocation_id, original_size, duration_ms)
            .await;

        Ok(ToolCallResult {
            data: serde_json::json!({
                "type": "image",
                "file_path": canon.display().to_string(),
                "media_type": processed.media_type,
                "original_size": original_size,
                "model_content": "[Image content provided in the following message.]",
            }),
            new_messages: vec![msg],
            context_modifier: None,
            mcp_meta: None,
        })
    }

    /// Read a PDF as an inline document block (claude-code FileRead PDF path).
    /// Routing: `pages` ⇒ validate then defer to P4b extraction (error); else
    /// page-count > 10 ⇒ error; unsupported model OR size > 3MB ⇒ extraction-
    /// required error; else inline document block on `new_messages`.
    ///
    /// The PDF bytes ride on `new_messages` as a `ContentBlock::Document`
    /// (the frozen tool-result content is text-only); the tool-result text is a
    /// short placeholder, mirroring [`Self::read_image_result`].
    #[cfg(feature = "pdf-read")]
    #[allow(clippy::too_many_arguments)]
    async fn read_pdf_result(
        &self,
        invocation_id: &str,
        canon: &std::path::Path,
        bytes: Vec<u8>,
        original_size: u64,
        pages: Option<String>,
        model: &str,
        started: Instant,
    ) -> Result<ToolCallResult, ToolError> {
        use crate::pdf_read::{
            is_pdf_supported, parse_pdf_page_range, pdf_page_count, PDF_AT_MENTION_INLINE_THRESHOLD,
            PDF_EXTRACT_SIZE_THRESHOLD, PDF_MAX_PAGES_PER_READ,
        };
        use base64::Engine;
        // Model comes from the per-call `ToolUseContext` (`ctx.options.
        // main_loop_model`), the same source the text path uses for the
        // cyber-risk-mitigation gate. claude-code canonicalizes the model name
        // before `isPDFSupported`; LingXi compares the raw model id (the same
        // documented divergence as `MITIGATION_EXEMPT_MODELS`). LingXi's default
        // models are PDF-capable, so a stray non-canonical id only ever yields
        // `true` here unless it literally contains `claude-3-haiku`.
        let supported = is_pdf_supported(model);

        // `pages` parameter ⇒ ranged read. Validate the range and the per-read
        // page cap, then defer to P4b page-image extraction (not yet available).
        if let Some(ref p) = pages {
            let Some((first, last)) = parse_pdf_page_range(p) else {
                self.emit_failed(invocation_id, "pdf_pages_invalid").await;
                return Err(ToolError::Io(format!(
                    "Invalid pages parameter: \"{p}\". Use formats like \"1-5\", \"3\", or \"10-20\". Pages are 1-indexed."
                )));
            };
            // Open-ended `"N-"` ranges are treated as exceeding the cap (TS
            // resolves the open end against the doc's page count, which can be
            // any size; conservatively over-cap here so they take the error
            // path rather than silently uncapped extraction).
            let range_size = if last == u32::MAX {
                PDF_MAX_PAGES_PER_READ + 1
            } else {
                last - first + 1
            };
            if range_size > PDF_MAX_PAGES_PER_READ {
                self.emit_failed(invocation_id, "pdf_pages_too_many").await;
                return Err(ToolError::Io(format!(
                    "Page range \"{p}\" exceeds maximum of {PDF_MAX_PAGES_PER_READ} pages per request. Please use a smaller range."
                )));
            }
            self.emit_failed(invocation_id, "pdf_extraction_unavailable")
                .await;
            return Err(ToolError::Io(
                "Reading specific PDF pages requires page extraction, which is not yet available. Read the whole PDF (omit pages) if it is small.".to_string(),
            ));
        }

        // No `pages` ⇒ inline read. A parseable page count > 10 is too many to
        // read inline (TS: pdfinfo page-count gate; `None` skips the gate,
        // mirroring pdfinfo returning null).
        if let Some(count) = pdf_page_count(&bytes) {
            if count > PDF_AT_MENTION_INLINE_THRESHOLD {
                self.emit_failed(invocation_id, "pdf_too_many_pages").await;
                return Err(ToolError::Io(format!(
                    "This PDF has {count} pages, which is too many to read at once. Use the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Maximum {PDF_MAX_PAGES_PER_READ} pages per request."
                )));
            }
        }

        // Unsupported model OR oversize ⇒ inline read is refused; the model
        // must extract specific pages (P4b) or switch models.
        if !supported || original_size > PDF_EXTRACT_SIZE_THRESHOLD {
            self.emit_failed(invocation_id, "pdf_extraction_required")
                .await;
            return Err(ToolError::Io(
                "Reading full PDFs is not supported with this model or this file is too large. Use a newer model, or use the pages parameter to read specific page ranges (e.g., pages: \"1-5\", maximum 20 pages per request).".to_string(),
            ));
        }

        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let source = protocol::DocumentSource::Base64 {
            media_type: "application/pdf".to_string(),
            data,
        };
        let msg = protocol::ConversationMessage::user_with_documents(
            protocol::MessageId::new(),
            String::new(),
            vec![source],
        );
        // MIRROR the text/image success path's completion telemetry — the PDF
        // branch has no line/byte slice counts (`emit_session_file_read` is
        // text-only), so we emit the simpler completion event with the original
        // file size as the `bytes_read` figure.
        self.emit_completed(invocation_id, original_size, started.elapsed().as_millis() as u64)
            .await;
        Ok(ToolCallResult {
            data: serde_json::json!({
                "type": "pdf",
                "file_path": canon.display().to_string(),
                "original_size": original_size,
                "model_content": format!("PDF file read: {} ({} bytes)", canon.display(), original_size),
            }),
            new_messages: vec![msg],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file_path"],
        "properties": {
            "file_path": { "type": "string" },
            "offset": { "type": "integer", "minimum": 0 },
            "limit":  { "type": "integer", "minimum": 1 },
            "pages":  { "type": "string" }
        }
    })
});

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-01 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        "Read a file from the workspace.".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Read a UTF-8 text file. Returns content, line range, total lines.".to_string()
    }

    fn get_path(&self, input: &Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = ulid_or_uuid();
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;
        // Raw input values, preserved verbatim for the read-state registry
        // (TS `readFileState.set` stores the `offset`/`limit` as provided —
        // `undefined` when absent). `offset` below defaults to `1` only for
        // slicing; the registry records the un-defaulted `Option`.
        let input_offset = input.get("offset").and_then(Value::as_u64);
        let input_limit = input.get("limit").and_then(Value::as_u64);
        let offset = input_offset.unwrap_or(1);
        let limit = input_limit;

        let started = Instant::now();
        let path = PathBuf::from(file_path);
        self.emit_started(&invocation_id, &path).await;

        // `tengu_file_read_limits_override` (`FileReadTool.ts:511-516`): TS fires
        // this at the top of the read iff `fileReadingLimits !== undefined`. The
        // LingXi `ToolUseContext` has NO `fileReadingLimits` field — there is no
        // source for caller-overridden read limits — so the condition is never
        // true and the emit is a documented no-op (faithful port of a never-true
        // branch; the event NAME is still registered). `None` => never fires.
        self.emit_file_read_limits_override(None).await;

        let canon = match canonicalize_and_validate(&path, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path });
            }
        };

        let metadata = match tokio::fs::metadata(&canon).await {
            Ok(m) => m,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_metadata").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };
        let size = metadata.len();
        // Floor-truncated mtime in ms, matching TS `Math.floor(mtimeMs)` for
        // the read-state registry (`readFileState.set`). A missing mtime
        // (rare; e.g. platforms without mtime) falls back to the epoch (`0`).
        let mtime_ms = metadata
            .modified()
            .map(tool_api::read_file_state::mtime_ms_floor)
            .unwrap_or(0);

        // Read dedup (`FileReadTool.ts:536-573`): if this EXACT range was
        // already read (a prior `Read`, full view, same offset+limit) and the
        // file's mtime is unchanged on disk, return the byte-locked
        // `file_unchanged` stub instead of re-sending the content. The earlier
        // Read tool_result is still in context; two full copies waste
        // cache_creation tokens on every later turn. Gated on:
        //   * a recorded entry that came from a `Read` (`from_read`; TS
        //     `existingState.offset !== undefined`) — never an Edit/Write
        //     post-write entry, whose content/mtime reflect the post-edit
        //     state and would misdirect the model;
        //   * a FULL view (`offset`/`limit` both `None`; TS
        //     `!existingState.isPartialView`, approximated as in the staleness
        //     guard);
        //   * the SAME range (`entry.offset == input_offset &&
        //     entry.limit == input_limit`; TS `rangeMatch`);
        //   * unchanged mtime (`mtime_ms == entry.mtime_ms`; TS
        //     `mtimeMs === existingState.timestamp`).
        // The GB killswitch (`tengu_read_dedup_killswitch`) is unported — LingXi
        // has no GrowthBook; dedup is always enabled (3P default = killswitch
        // off). NOTE: the partial-view (`isPartialView`) flag set by TS's
        // CLAUDE.md / memory auto-injection has no LingXi analog; the
        // offset/limit approximation matches for normal `Read`-sourced entries.
        if let Some(entry) = tool_api::read_file_state::get(&self.ctx.read_file_state, &canon) {
            let is_full_view = entry.offset.is_none() && entry.limit.is_none();
            let range_match = entry.offset == input_offset && entry.limit == input_limit;
            if entry.from_read && is_full_view && range_match && mtime_ms == entry.mtime_ms {
                // `tengu_file_read_dedup` (`FileReadTool.ts:559-561`) — fired in
                // the dedup short-circuit, before returning the `file_unchanged`
                // stub. Metadata: `ext` only when present (the analytics ext of
                // the resolved path). TS uses `fullFilePath` (`expandPath`); the
                // Rust analog is the canonicalized path.
                self.emit_file_read_dedup(&canon).await;
                // Behaves like the TS early return: the model sees the stub via
                // `model_content`; the TUI payload `content` mirrors it (there
                // is no fresh file body to render). `total_lines`/`line_range`
                // are omitted — this is the `file_unchanged` result variant.
                self.emit_completed(&invocation_id, 0, started.elapsed().as_millis() as u64)
                    .await;
                return Ok(ToolCallResult {
                    data: json!({
                        "type": "file_unchanged",
                        "content": FILE_UNCHANGED_STUB,
                        "model_content": FILE_UNCHANGED_STUB,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                });
            }
        }

        // Image files route to the multimodal image path (claude-code routes by
        // extension to readImageWithTokenBudget, bypassing the text size cap).
        let is_image = cfg!(feature = "image-read") && is_image_path(&canon);

        // PDF files route to the document path (claude-code routes by extension
        // to the PDF reader, which applies its own 3MB extraction gate, not the
        // 256KB text cap). Gated on the feature so the cap still applies — and
        // PDFs still hit the binary guard — when `pdf-read` is off.
        let is_pdf = cfg!(feature = "pdf-read") && is_pdf_path(&canon);

        // TS applies the byte cap ONLY when no `limit` is supplied
        // (`readFileInRange(..., limit === undefined ? maxSizeBytes : undefined)`
        // — FileReadTool.ts:1026). A ranged read (offset+limit) of a >256KB file
        // must succeed and return just the requested lines, so the cap is gated
        // on `input_limit.is_none()`. A no-limit oversize read still errors with
        // the byte-locked template (fixture-pinned `error_template`).
        if !is_image && !is_pdf && input_limit.is_none() && size > MAX_FILE_READ_SIZE {
            self.emit_failed(&invocation_id, "file_too_large").await;
            return Err(ToolError::Io(format_too_large(&canon, size)));
        }

        let bytes = match tokio::fs::read(&canon).await {
            Ok(b) => b,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_read").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };

        #[cfg(feature = "image-read")]
        if is_image {
            return self
                .read_image_result(&invocation_id, &canon, bytes, size, started)
                .await;
        }

        #[cfg(feature = "pdf-read")]
        if is_pdf {
            let pages = input
                .get("pages")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            return self
                .read_pdf_result(
                    &invocation_id,
                    &canon,
                    bytes,
                    size,
                    pages,
                    &ctx.options.main_loop_model,
                    started,
                )
                .await;
        }

        let head = &bytes[..bytes.len().min(NUL_SCAN_WINDOW)];
        if looks_binary(head) {
            self.emit_failed(&invocation_id, "binary_file").await;
            return Err(ToolError::Io(format_binary(&canon)));
        }

        let content = match decode_utf8_strict(&bytes) {
            Ok(s) => s,
            Err(_) => {
                self.emit_failed(&invocation_id, "non_utf8").await;
                return Err(ToolError::Io(format!(
                    "File {} is not valid UTF-8",
                    canon.display()
                )));
            }
        };

        // Notebook (`.ipynb`) structured-cell read (`FileReadTool.ts:822-863`).
        // A notebook is parsed into an ARRAY of structured cells — NOT the
        // line-numbered plain text below — so the model sees per-cell
        // `{ cellType, source, execution_count?, cell_id, language?, outputs? }`
        // exactly as TS's `readNotebook`. The TUI `data` carries the structured
        // `cells`; the model-facing string (`model_content`) is the text-block
        // projection of TS's `mapNotebookCellsToToolResult`. The `ext` is taken
        // from the ORIGINAL input path (TS `path.extname(file_path)`), matching
        // the same case-insensitive `.ipynb` test FileEditTool uses to route to
        // NotebookEdit. A parse failure surfaces the JSON error (TS's
        // `jsonParse` throws → propagates out of `call`).
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if ext.as_deref() == Some("ipynb") {
            let cells = match crate::notebook_read::read_notebook(&content) {
                Ok(c) => c,
                Err(e) => {
                    self.emit_failed(&invocation_id, "notebook_parse").await;
                    return Err(ToolError::Io(e));
                }
            };
            let model_content = crate::notebook_read::render_cells_model_text(&cells);

            // Serialized cells JSON — TS's `cellsJson = jsonStringify(cells)`
            // (`FileReadTool.ts:824`). One serialization, reused for both the
            // token gate and the registry entry (matches TS's single `cellsJson`).
            let cells_json = serde_json::to_string(&cells).unwrap_or_default();

            // Token-budget gate on the cells JSON — TS runs
            // `validateContentTokens(cellsJson, ext, maxTokens)`
            // (`FileReadTool.ts:838`) on the notebook path too, after the
            // byte-size check and before recording state. `ext` is `"ipynb"`
            // (bytesPerToken 4). The Notebook byte-size cap (`cellsJsonBytes >
            // maxSizeBytes`, FileReadTool.ts:827) is a separate pre-existing
            // gap not in scope here.
            if let Err(msg) =
                validate_content_tokens(&cells_json, Some("ipynb"), DEFAULT_MAX_OUTPUT_TOKENS)
            {
                self.emit_failed(&invocation_id, "max_tokens_exceeded").await;
                return Err(ToolError::Io(msg));
            }

            let duration_ms = started.elapsed().as_millis() as u64;
            self.emit_completed(&invocation_id, bytes.len() as u64, duration_ms)
                .await;

            // Record the read — TS stores the serialized cells JSON as the
            // entry `content` (`FileReadTool.ts:842-847`) with the DEFAULTED
            // offset/limit. To keep the registry's full-view discriminator
            // (used by the staleness guard + dedup) consistent with the rest of
            // the Rust port, a no-range notebook read records `offset`/`limit`
            // as the verbatim (un-defaulted) input — `None` for a full read —
            // matching `read.rs`'s text path. The stored `content` is the
            // cells JSON (so a same-range re-read dedups byte-for-byte).
            tool_api::read_file_state::set(
                &self.ctx.read_file_state,
                canon.clone(),
                tool_api::read_file_state::ReadFileEntry {
                    content: cells_json,
                    mtime_ms,
                    offset: input_offset,
                    limit: input_limit,
                    from_read: true,
                },
            );

            return Ok(ToolCallResult {
                data: json!({
                    "type": "notebook",
                    "file_path": file_path,
                    "cells": cells,
                    "model_content": model_content,
                }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            });
        }

        let all_lines: Vec<&str> = content.split_inclusive('\n').collect();
        // FILE.4: `total_lines` must match TS `readFileInRange.totalLines`
        // (`utils/readFileInRange.ts:188`) = (number of '\n') + 1. TS does an
        // UNCONDITIONAL post-loop `lineIndex++` that counts the (possibly empty)
        // final fragment after the last newline, so a TRAILING newline adds a
        // phantom final line (e.g. "a\nb\nc\n" => 4, not the natural 3). The prior
        // `split_inclusive().len()` undercounted trailing-newline files by one in
        // the model-facing `total_lines` field + the offset-beyond-EOF warning.
        // An empty file stays 0 so the byte-locked EMPTY_FILE_WARNING branch still
        // fires (TS's empty path is upstream of readFileInRange and not reproduced
        // here). NOTE: `all_lines` (split_inclusive) is what the range SLICING below
        // uses — only this model-facing count is TS-aligned.
        let total_lines = if content.is_empty() {
            0
        } else {
            content.bytes().filter(|&b| b == b'\n').count() as u64 + 1
        };
        let start_idx = (offset.saturating_sub(1) as usize).min(all_lines.len());
        let end_idx = match limit {
            Some(l) => (start_idx + l as usize).min(all_lines.len()),
            None => all_lines.len(),
        };
        let slice: String = all_lines[start_idx..end_idx].concat();
        let line_range_start = offset;
        let line_range_end = end_idx as u64;

        // Token-budget gate — `validateContentTokens(content, ext, maxTokens)`
        // (`FileReadTool.ts:1030`). Runs on the range-limited slice, BEFORE the
        // success/state side-effects, so an over-budget read throws (TS) /
        // errors (Rust) with no completed event and nothing recorded. `ext` is
        // the lowercased extension without the dot (TS `path.extname(...).
        // slice(1)`). LingXi has no `fileReadingLimits`, so the budget is the
        // default. See [`validate_content_tokens`] for the offline-fallback
        // mapping of TS's count_tokens API refinement.
        if let Err(msg) = validate_content_tokens(&slice, ext.as_deref(), DEFAULT_MAX_OUTPUT_TOKENS) {
            self.emit_failed(&invocation_id, "max_tokens_exceeded").await;
            return Err(ToolError::Io(msg));
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, bytes.len() as u64, duration_ms)
            .await;

        // Record the read into the shared read-state registry — 1:1 with TS
        // `readFileState.set(fullFilePath, {content, timestamp, offset,
        // limit})` (`FileReadTool.ts:1032`). `content` is the range-limited
        // slice TS stores (from `readFileInRange`), `mtime_ms` is floored, and
        // `offset`/`limit` are the verbatim (un-defaulted) input values. The
        // key is the canonicalized absolute path. Behavior-neutral side-effect:
        // nothing reads this map yet (staleness guards + Read dedup are later
        // batches), so the tool's result shape is unchanged.
        tool_api::read_file_state::set(
            &self.ctx.read_file_state,
            canon.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: slice.clone(),
                mtime_ms,
                offset: input_offset,
                limit: input_limit,
                // This entry is the product of a `Read` — the dedup gate
                // (`FileReadTool.ts:550` `offset !== undefined`) only short-
                // circuits against Read-sourced entries.
                from_read: true,
            },
        );

        // `tengu_session_file_read` (`FileReadTool.ts:1069-1083`) — fired after a
        // successful TEXT read, mirroring TS's site (after `readFileState.set`,
        // before returning the data). NOT fired on the notebook path (TS returns
        // before this site) nor on images/PDFs. The byte/line counts mirror
        // `readFileInRange`'s return: `totalBytes = Buffer.byteLength(text)` →
        // full content byte length; `readBytes = Buffer.byteLength(content)` →
        // the selected slice's byte length; `readLines = lineCount` → number of
        // selected lines (`end_idx - start_idx`). `offset` is the defaulted
        // offset; `limit` only when supplied. See [`emit_session_file_read`] for
        // the `ext` / `messageID` (omitted) / session-flag handling.
        let read_lines = (end_idx - start_idx) as u64;
        self.emit_session_file_read(
            &canon,
            total_lines,
            read_lines,
            content.len() as u64,
            slice.len() as u64,
            offset,
            input_limit,
        )
        .await;

        // Model-facing serialization (FILE.A). `content` above stays the RAW
        // slice — that is what the TUI renders (`emit_tool_result` payload). The
        // model instead sees `model_content`: cat -n line numbers (+ the
        // cyber-risk reminder) for non-empty reads, or the byte-locked empty /
        // offset-beyond-EOF `<system-reminder>` warning otherwise. This mirrors
        // claude-code's `FileReadTool` mapper (`FileReadTool.ts:692-714`), where
        // the model string diverges from the UI chrome.
        let model_content = if slice.is_empty() {
            if total_lines == 0 {
                EMPTY_FILE_WARNING.to_string()
            } else {
                format_offset_beyond_eof(offset, total_lines)
            }
        } else {
            let mut mc = add_line_numbers(&slice, offset);
            if should_include_file_read_mitigation(&ctx.options.main_loop_model) {
                mc.push_str(CYBER_RISK_MITIGATION_REMINDER);
            }
            mc
        };

        Ok(ToolCallResult {
            data: json!({
                "content": slice,
                "model_content": model_content,
                "line_range": [line_range_start, line_range_end],
                "total_lines": total_lines
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tempfile::TempDir;
    use tool_api::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};

    fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let fs = make_dummy_fs();
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        (
            tool_api::test_support::ctx_for_file_tools(fs, bus, vec![tmp.path().to_path_buf()]),
            sink,
        )
    }

    #[test]
    fn max_size_byte_locked() {
        assert_eq!(MAX_FILE_READ_SIZE, 262_144);
    }

    #[test]
    fn tool_name_is_read() {
        assert_eq!(TOOL_NAME, "Read");
    }

    #[test]
    fn too_large_message_byte_locked() {
        let msg = format_too_large(std::path::Path::new("/tmp/x"), 300_000);
        assert_eq!(msg, "File /tmp/x (300000B) exceeds 256KB read limit");
    }

    #[test]
    fn binary_message_byte_locked() {
        let msg = format_binary(std::path::Path::new("/tmp/x"));
        assert_eq!(
            msg,
            "File /tmp/x appears to be binary (first 8KB contains NUL bytes)"
        );
    }

    #[tokio::test]
    async fn happy_path_reads_full_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello\nworld\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "hello\nworld\n");
        // FILE.4: TS counts the trailing newline's empty final fragment as a line
        // ("hello\nworld\n" => 3, matching readFileInRange `lineIndex`).
        assert_eq!(result.data["total_lines"], 3);
        // Model-facing string: cat -n (compact tab format, 1-based from offset)
        // + the cyber-risk reminder (ctx model "test" is not exempt).
        assert_eq!(
            result.data["model_content"],
            format!("1\thello\n2\tworld\n3\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_read_started"));
        assert!(names.contains(&"tengu_tool_read_completed"));
    }

    #[tokio::test]
    async fn rejects_oversize_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big.txt");
        std::fs::write(&target, vec![b'A'; (MAX_FILE_READ_SIZE + 1) as usize]).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exceeds 256KB read limit"), "got: {msg}");
    }

    #[tokio::test]
    async fn ranged_read_of_oversize_file_returns_requested_lines() {
        // FILE.1: TS gates the 256KB cap on `limit === undefined`
        // (FileReadTool.ts:1026). A >256KB file read with offset+limit must
        // return just the requested range, NOT the too-large error.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big_ranged.txt");
        // Build a >256KB file made of distinct 1-based lines so we can assert
        // the slice precisely. Each "lineNN\n" is small; pad to exceed the cap.
        let mut body = String::from("first\nsecond\nthird\n");
        // Fill past MAX_FILE_READ_SIZE with filler lines.
        while body.len() as u64 <= MAX_FILE_READ_SIZE {
            body.push_str("filler-line-of-some-length\n");
        }
        assert!(body.len() as u64 > MAX_FILE_READ_SIZE);
        std::fs::write(&target, &body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 2 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ranged read of an oversize file must succeed");
        assert_eq!(result.data["content"], "second\nthird\n");
    }

    #[tokio::test]
    async fn no_limit_read_of_oversize_file_still_errors() {
        // FILE.1 invariant: with no `limit`, the byte cap still applies and the
        // byte-locked too-large error is returned (fixture-pinned template).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big_nolimit.txt");
        let mut body = String::from("first\nsecond\nthird\n");
        while body.len() as u64 <= MAX_FILE_READ_SIZE {
            body.push_str("filler-line-of-some-length\n");
        }
        std::fs::write(&target, &body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exceeds 256KB read limit"), "got: {msg}");
    }

    #[tokio::test]
    async fn rejects_binary_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bin");
        let mut data = vec![b'A'; 100];
        data[10] = 0;
        std::fs::write(&target, &data).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("appears to be binary (first 8KB contains NUL bytes)"));
    }

    #[tokio::test]
    async fn offset_1_returns_from_first_line() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 1, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line1\n");
        // cat -n numbers from offset=1.
        assert_eq!(
            result.data["model_content"],
            format!("1\tline1\n2\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
    }

    #[tokio::test]
    async fn offset_2_skips_one_line() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line2\n");
        // Numbering starts at the requested offset (2), not 1.
        assert_eq!(
            result.data["model_content"],
            format!("2\tline2\n3\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
    }

    #[tokio::test]
    async fn limit_greater_than_remaining_returns_what_exists() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 50 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line2\n");
    }

    #[tokio::test]
    async fn no_offset_no_limit_returns_full_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "alpha\nbeta\ngamma\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "alpha\nbeta\ngamma\n");
        // FILE.4: trailing newline => phantom final line ("...\n" => 4).
        assert_eq!(result.data["total_lines"], 4);
    }

    #[tokio::test]
    async fn read_populates_read_file_state_map_with_offset_limit() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        // Hold a handle to the shared registry BEFORE the ctx is moved into the
        // tool — the tool's `readFileState.set` mutates this same `Arc`.
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        // canonicalize the target the same way the tool keys the entry.
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry =
            tool_api::read_file_state::get(&map, &canon).expect("registry entry recorded on read");
        // Content is the range-limited slice (TS stores `readFileInRange`'s
        // output), and offset/limit are the verbatim input values.
        assert_eq!(entry.content, "line2\n");
        assert_eq!(entry.offset, Some(2));
        assert_eq!(entry.limit, Some(1));
        // mtime recorded as a non-negative floor-truncated millisecond value.
        assert!(entry.mtime_ms >= 0);
    }

    #[tokio::test]
    async fn read_without_offset_limit_records_none() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("b.txt");
        std::fs::write(&target, "alpha\nbeta\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry = tool_api::read_file_state::get(&map, &canon).unwrap();
        assert_eq!(entry.content, "alpha\nbeta\n");
        assert_eq!(entry.offset, None);
        assert_eq!(entry.limit, None);
    }

    #[tokio::test]
    async fn failed_read_does_not_populate_map() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("missing.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let _ = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        // A read that errored (missing file) records nothing.
        assert!(map.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn utf8_bom_stripped_on_read() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bom.txt");
        // BOM + "hello"
        let mut content = vec![0xEF, 0xBB, 0xBF];
        content.extend_from_slice(b"hello");
        std::fs::write(&target, &content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_path_outside_trusted() {
        // Deviation from plan: std::fs::canonicalize requires every path
        // component to exist, so a bare outside path errors via Io rather
        // than Outside. Use a symlink inside the trusted tempdir pointing
        // at an outside location — canonicalize follows the symlink and
        // produces a real "Outside" rejection.
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target_real = outside.path().join("a.txt");
        std::fs::write(&target_real, "x").unwrap();
        let link = tmp.path().join("escape");
        std::os::unix::fs::symlink(&target_real, &link).unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": link.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::PathBlocked { .. } => {}
            other => panic!("expected PathBlocked, got {other:?}"),
        }
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_file_path_blocked"));
        assert!(names.contains(&"tengu_tool_read_failed"));
    }

    #[tokio::test]
    async fn empty_file_emits_empty_warning_model_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("empty.txt");
        std::fs::write(&target, "").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // TUI payload `content` stays the raw (empty) slice; the model sees the
        // empty-file warning instead of an empty string.
        assert_eq!(result.data["content"], "");
        assert_eq!(result.data["total_lines"], 0);
        assert_eq!(result.data["model_content"], EMPTY_FILE_WARNING);
    }

    #[tokio::test]
    async fn offset_beyond_eof_emits_offset_warning_model_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("short.txt");
        std::fs::write(&target, "a\nb\nc\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 10 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "");
        // FILE.4: "a\nb\nc\n" has 3 newlines => TS total_lines 4 (phantom final line).
        assert_eq!(result.data["total_lines"], 4);
        assert_eq!(
            result.data["model_content"],
            "<system-reminder>Warning: the file exists but is shorter than the provided offset (10). The file has 4 lines.</system-reminder>"
        );
    }

    #[test]
    fn add_line_numbers_compact_format() {
        assert_eq!(add_line_numbers("", 1), "");
        assert_eq!(
            add_line_numbers("hello\nworld\n", 1),
            "1\thello\n2\tworld\n3\t"
        );
        // Numbering starts at `start_line`.
        assert_eq!(add_line_numbers("x", 5), "5\tx");
        // CRLF: a trailing \r is stripped per line (mirrors the TS /\r?\n/ split).
        assert_eq!(add_line_numbers("a\r\nb", 1), "1\ta\n2\tb");
    }

    #[test]
    fn mitigation_reminder_gated_on_model() {
        assert!(should_include_file_read_mitigation("claude-opus-4-8"));
        assert!(should_include_file_read_mitigation("test"));
        // The one exempt model skips the reminder.
        assert!(!should_include_file_read_mitigation("claude-opus-4-6"));
    }

    // ───────────────────────── Notebook (.ipynb) structured read ────────────

    fn sample_ipynb() -> String {
        serde_json::to_string(&json!({
            "cells": [
                {
                    "cell_type": "code",
                    "id": "c1",
                    "source": ["print(", "'hi')"],
                    "execution_count": 2,
                    "outputs": [
                        { "output_type": "stream", "name": "stdout", "text": "hi\n" }
                    ]
                },
                { "cell_type": "markdown", "id": "c2", "source": "# Title" }
            ],
            "metadata": { "language_info": { "name": "python" } },
            "nbformat": 4,
            "nbformat_minor": 5
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn ipynb_read_returns_structured_cells() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_ipynb()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Notebook result variant: structured `cells`, NOT line-numbered text.
        assert_eq!(result.data["type"], "notebook");
        let cells = result.data["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 2);
        // Code cell: camelCase cellType, joined source, execution_count, language.
        assert_eq!(cells[0]["cellType"], "code");
        assert_eq!(cells[0]["source"], "print('hi')");
        assert_eq!(cells[0]["execution_count"], 2);
        assert_eq!(cells[0]["cell_id"], "c1");
        assert_eq!(cells[0]["language"], "python");
        assert_eq!(cells[0]["outputs"][0]["output_type"], "stream");
        assert_eq!(cells[0]["outputs"][0]["text"], "hi\n");
        // Markdown cell: cellType markdown, no language/outputs.
        assert_eq!(cells[1]["cellType"], "markdown");
        assert!(cells[1].get("language").is_none());
        // The model-facing string is the cell-block text projection.
        assert_eq!(
            result.data["model_content"],
            "<cell id=\"c1\">print('hi')</cell id=\"c1\">\n\nhi\n\n<cell id=\"c2\"><cell_type>markdown</cell_type># Title</cell id=\"c2\">"
        );
    }

    #[tokio::test]
    async fn ipynb_read_records_full_view_registry_entry() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_ipynb()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry = tool_api::read_file_state::get(&map, &canon).expect("notebook read recorded");
        // A no-range notebook read is a full view (offset/limit None) and is
        // flagged as Read-sourced so the staleness guard accepts a follow-up
        // NotebookEdit and the dedup gate sees a Read entry.
        assert_eq!(entry.offset, None);
        assert_eq!(entry.limit, None);
        assert!(entry.from_read);
        // Stored content is the serialized cells JSON (so a same-range re-read
        // dedups byte-for-byte).
        assert!(entry.content.contains("\"cellType\""));
    }

    #[tokio::test]
    async fn ipynb_invalid_json_errors() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bad.ipynb");
        std::fs::write(&target, "not a notebook").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("notebook JSON parse"), "got: {err}");
    }

    // ───────────────────────── Read dedup (file_unchanged) ──────────────────

    #[tokio::test]
    async fn unchanged_reread_returns_file_unchanged_stub() {
        // A full Read, then an immediate identical re-read with no on-disk
        // change → the dedup short-circuit returns the byte-locked stub.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dedup.txt");
        std::fs::write(&target, "alpha\nbeta\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        // First read populates the registry.
        let first = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(first.data["content"], "alpha\nbeta\n");
        // Second identical read → file_unchanged stub.
        let second = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(second.data["type"], "file_unchanged");
        assert_eq!(second.data["content"], FILE_UNCHANGED_STUB);
        assert_eq!(second.data["model_content"], FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn changed_file_reread_does_not_dedup() {
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("changed.txt");
        std::fs::write(&target, "v0\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // External modification bumps mtime → re-read must NOT dedup.
        std::fs::write(&target, "v1\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
        let again = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Fresh content returned, not the stub.
        assert_eq!(again.data["content"], "v1\n");
        assert!(again.data.get("type").is_none());
    }

    #[tokio::test]
    async fn different_range_reread_does_not_dedup() {
        // First read full, then a ranged re-read of the same (unchanged) file:
        // the range differs, so dedup must NOT fire.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("range.txt");
        std::fs::write(&target, "l1\nl2\nl3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let ranged = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Different range → real content, no stub.
        assert_eq!(ranged.data["content"], "l2\n");
        assert!(ranged.data.get("type").is_none());
    }

    #[tokio::test]
    async fn write_then_read_does_not_dedup_against_post_write_entry() {
        // A Write records a post-write registry entry with `from_read=false`.
        // A subsequent Read of the same path must NOT dedup against it (TS gates
        // dedup on `offset !== undefined`, i.e. Read-sourced entries only).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("wr.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let canon = {
            std::fs::write(&target, "seed\n").unwrap();
            std::fs::canonicalize(&target).unwrap()
        };
        let mtime_ms = std::fs::metadata(&canon)
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        // Simulate a Write's post-write entry: full view, but NOT from a Read.
        tool_api::read_file_state::set(
            &map,
            canon,
            tool_api::read_file_state::ReadFileEntry {
                content: "seed\n".into(),
                mtime_ms,
                offset: None,
                limit: None,
                from_read: false,
            },
        );
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Real content returned (the write entry is not a dedup candidate).
        assert_eq!(result.data["content"], "seed\n");
        assert!(result.data.get("type").is_none());
    }

    // ───────────────────────── Token-budget gate ────────────────────────────

    #[test]
    fn rough_token_estimate_byte_locked() {
        // Round-half-up over byte length, bpt=4 for non-json: (len+2)/4.
        assert_eq!(rough_token_count_estimation_for_file_type("", None), 0);
        assert_eq!(rough_token_count_estimation_for_file_type("ab", None), 1); // (2+2)/4
        assert_eq!(
            rough_token_count_estimation_for_file_type(&"a".repeat(100_000), None),
            25_000
        );
        // json/jsonl/jsonc use bpt=2 (denser tokens): (len+1)/2.
        assert_eq!(bytes_per_token_for_file_type(Some("json")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("jsonl")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("jsonc")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("txt")), 4);
        assert_eq!(bytes_per_token_for_file_type(None), 4);
        assert_eq!(
            rough_token_count_estimation_for_file_type(&"a".repeat(50_000), Some("json")),
            25_000
        );
    }

    #[test]
    fn max_tokens_message_byte_locked() {
        assert_eq!(
            format_max_tokens_exceeded(30_000, 25_000),
            "File content (30000 tokens) exceeds maximum allowed tokens (25000). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
        );
        assert_eq!(DEFAULT_MAX_OUTPUT_TOKENS, 25_000);
    }

    #[test]
    fn validate_content_tokens_gate_behavior() {
        // Below maxTokens/4 (6250) → early pass.
        assert!(validate_content_tokens(&"a".repeat(1_000), None, 25_000).is_ok());
        // In the (maxTokens/4, maxTokens] band → passes (offline API fallback
        // admits it: effectiveCount == estimate <= maxTokens).
        // 80_000 bytes / 4 = 20_000 tokens (> 6250, <= 25000).
        assert!(validate_content_tokens(&"a".repeat(80_000), None, 25_000).is_ok());
        // Above maxTokens → error. 100_004 bytes / 4 = 25_001 > 25_000.
        assert!(validate_content_tokens(&"a".repeat(100_004), None, 25_000).is_err());
        // json density: 50_002 bytes / 2 = 25_001 > 25_000 → error.
        assert!(validate_content_tokens(&"a".repeat(50_002), Some("json"), 25_000).is_err());
    }

    #[tokio::test]
    async fn full_read_over_token_budget_errors() {
        // A full read whose estimated tokens exceed DEFAULT_MAX_OUTPUT_TOKENS
        // (25000) but stay under the 256KB byte cap must error with the
        // byte-locked max-tokens message — and emit read_failed, not completed.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dense.txt");
        // ~120 KB of ASCII (under 256KB), estimate 120000/4 = 30000 > 25000.
        let body = "x".repeat(120_000);
        std::fs::write(&target, &body).unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds maximum allowed tokens (25000)"),
            "got: {msg}"
        );
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_read_failed"));
        assert!(!names.contains(&"tengu_tool_read_completed"));
    }

    #[tokio::test]
    async fn ranged_read_under_token_budget_of_huge_file_passes() {
        // The gate runs on the range-limited slice, not the whole file. A huge
        // file read with offset+limit returning a small slice must pass.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("huge.txt");
        // > token budget if read whole, but we slice 2 lines.
        let mut body = String::from("first\nsecond\nthird\n");
        while body.len() < 120_000 {
            body.push_str("filler-line\n");
        }
        std::fs::write(&target, &body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 1, "limit": 2 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("small slice of a huge file passes the token gate");
        assert_eq!(result.data["content"], "first\nsecond\n");
    }

    #[tokio::test]
    async fn read_under_token_budget_passes() {
        // A small file is well under the budget — no token error.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("small.txt");
        std::fs::write(&target, "hello\nworld\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "hello\nworld\n");
    }

    #[test]
    fn model_facing_constants_byte_locked() {
        assert_eq!(
            EMPTY_FILE_WARNING,
            "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
        );
        assert_eq!(
            format_offset_beyond_eof(500, 12),
            "<system-reminder>Warning: the file exists but is shorter than the provided offset (500). The file has 12 lines.</system-reminder>"
        );
        assert!(CYBER_RISK_MITIGATION_REMINDER.starts_with("\n\n<system-reminder>\n"));
        assert!(CYBER_RISK_MITIGATION_REMINDER.ends_with("</system-reminder>\n"));
        assert!(CYBER_RISK_MITIGATION_REMINDER.contains("would be considered malware"));
        assert!(FILE_UNCHANGED_STUB.starts_with("File unchanged since last read."));
    }

    // ───────────── FileReadTool tengu analytics events (3) ───────────────────

    /// Find the (single) recorded event with the given name.
    fn find_event<'a>(
        events: &'a [telemetry::sinks::in_memory::RecordedEvent],
        name: &str,
    ) -> Option<&'a telemetry::sinks::in_memory::RecordedEvent> {
        events.iter().find(|e| e.name == name)
    }

    #[test]
    fn analytics_ext_helper_matches_ts() {
        use std::path::Path;
        // Lowercased, no leading dot.
        assert_eq!(
            get_file_extension_for_analytics(Path::new("/a/b.RS")).as_deref(),
            Some("rs")
        );
        // No extension → None (TS `undefined`).
        assert_eq!(get_file_extension_for_analytics(Path::new("/a/README")), None);
        // Over MAX_FILE_EXTENSION_LENGTH (10) → "other".
        assert_eq!(
            get_file_extension_for_analytics(Path::new("/a/b.abcdefghijk")).as_deref(),
            Some("other")
        );
    }

    #[tokio::test]
    async fn session_file_read_fires_on_normal_text_read_with_counts() {
        // A normal (non-dedup, non-session) text read fires tengu_session_file_read
        // with the right line/byte counts and the only-if-present `ext`, and the
        // omitted `messageID` / false session flags.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("note.rs");
        std::fs::write(&target, "alpha\nbeta\ngamma\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "alpha\nbeta\ngamma\n");

        let events = sink.events().await;
        let ev = find_event(&events, "tengu_session_file_read")
            .expect("tengu_session_file_read must fire on a successful text read");
        let md = &ev.metadata;
        // totalLines = 4 (trailing-newline phantom final line, TS readFileInRange).
        assert!(matches!(md.get("totalLines"), Some(AnalyticsValue::Int(4))));
        // readLines = number of selected lines = total lines of the full read = 3
        // (split_inclusive over 3 newlines → 3 fragments).
        assert!(matches!(md.get("readLines"), Some(AnalyticsValue::Int(3))));
        // totalBytes = full content byte length; readBytes = selected slice bytes;
        // a full read makes them equal (17 bytes).
        assert!(matches!(md.get("totalBytes"), Some(AnalyticsValue::Int(17))));
        assert!(matches!(md.get("readBytes"), Some(AnalyticsValue::Int(17))));
        // offset defaulted to 1; limit omitted (only-if-present).
        assert!(matches!(md.get("offset"), Some(AnalyticsValue::Int(1))));
        assert!(md.get("limit").is_none());
        // ext present (only-if-present spread): lowercased "rs".
        assert!(matches!(md.get("ext"), Some(AnalyticsValue::String(s)) if s == "rs"));
        // messageID omitted faithfully (unreachable in the port).
        assert!(md.get("messageID").is_none());
        // Not a session file → both flags false.
        assert!(matches!(
            md.get("is_session_memory"),
            Some(AnalyticsValue::Bool(false))
        ));
        assert!(matches!(
            md.get("is_session_transcript"),
            Some(AnalyticsValue::Bool(false))
        ));
    }

    #[tokio::test]
    async fn session_file_read_includes_limit_when_ranged() {
        // A ranged read carries `limit` (only-if-present) and the ranged counts.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("ranged.txt");
        std::fs::write(&target, "l1\nl2\nl3\nl4\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 2 }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let events = sink.events().await;
        let md = &find_event(&events, "tengu_session_file_read")
            .expect("session_file_read fires on ranged read")
            .metadata;
        assert!(matches!(md.get("offset"), Some(AnalyticsValue::Int(2))));
        assert!(matches!(md.get("limit"), Some(AnalyticsValue::Int(2))));
        // 2 selected lines ("l2\nl3\n").
        assert!(matches!(md.get("readLines"), Some(AnalyticsValue::Int(2))));
        // readBytes = bytes of "l2\nl3\n" = 6; totalBytes = full file = 12.
        assert!(matches!(md.get("readBytes"), Some(AnalyticsValue::Int(6))));
        assert!(matches!(md.get("totalBytes"), Some(AnalyticsValue::Int(12))));
        // ext "txt" present.
        assert!(matches!(md.get("ext"), Some(AnalyticsValue::String(s)) if s == "txt"));
    }

    #[tokio::test]
    async fn dedup_path_fires_file_read_dedup_with_ext() {
        // The dedup short-circuit (file_unchanged) fires tengu_file_read_dedup
        // (and NOT a second tengu_session_file_read on the deduped read).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dedup.rs");
        std::fs::write(&target, "alpha\nbeta\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        // First read populates the registry + fires session_file_read.
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        sink.clear().await;
        // Second identical read → dedup stub.
        let second = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(second.data["type"], "file_unchanged");
        let events = sink.events().await;
        let md = &find_event(&events, "tengu_file_read_dedup")
            .expect("dedup short-circuit must fire tengu_file_read_dedup")
            .metadata;
        // ext present (only-if-present): "rs".
        assert!(matches!(md.get("ext"), Some(AnalyticsValue::String(s)) if s == "rs"));
        // The deduped read does NOT fire session_file_read (it returns the stub
        // before the text-read site).
        assert!(find_event(&events, "tengu_session_file_read").is_none());
    }

    #[tokio::test]
    async fn limits_override_is_wired_but_never_fires() {
        // The LingXi ToolUseContext has no fileReadingLimits, so the override
        // event is wired (name registered) but never emitted — a faithful port
        // of a never-true branch.
        assert!(
            telemetry::tengu::ALL_EVENT_NAMES.contains(&"tengu_file_read_limits_override"),
            "the limits-override event NAME must be registered even though it never fires"
        );
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("plain.txt");
        std::fs::write(&target, "x\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let events = sink.events().await;
        // Unreachable branch (fileReadingLimits == None) → never emitted.
        assert!(find_event(&events, "tengu_file_read_limits_override").is_none());
    }

    #[test]
    fn detect_session_file_type_classifies_under_config_home() {
        // session_memory: <configHome>/.../session-memory/*.md
        // session_transcript: <configHome>/.../projects/*.jsonl
        // Build paths under the resolved config home (no process-global env
        // mutation — that would race other parallel tests). `claude_config_home_dir`
        // resolves the same way `detect_session_file_type` reads it.
        let config_home = claude_config_home_dir();
        let mem = config_home.join("agents/session-memory/abc.md");
        let trans = config_home.join("projects/foo/bar.jsonl");
        let plain = config_home.join("settings.json");
        assert_eq!(detect_session_file_type(&mem), Some("session_memory"));
        assert_eq!(
            detect_session_file_type(&trans),
            Some("session_transcript")
        );
        // .md under the config home but NOT under session-memory/ → None.
        assert_eq!(detect_session_file_type(&plain), None);
        // Outside the config home → None.
        assert_eq!(
            detect_session_file_type(std::path::Path::new("/definitely/not/claude/x.md")),
            None
        );
    }

    #[cfg(feature = "image-read")]
    #[tokio::test]
    async fn reads_image_as_multimodal_new_message() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource};

        // Build a tiny PNG via the `image` crate (available under
        // `--features image-read`) and write it under the trusted dir, mirroring
        // the sibling text-read `call()` tests' tool/ctx/temp-file construction.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("pic.png");
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8))
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&target, &buf).unwrap();

        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let res = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("image read must succeed");

        // The tool-result data is text-only and tagged as an image.
        assert_eq!(res.data["type"], "image");

        // The pixels ride on a single `new_messages` user message carrying a
        // base64 `ContentBlock::Image` with the PNG media type.
        assert_eq!(res.new_messages.len(), 1);
        let content = match &res.new_messages[0] {
            ConversationMessage::User { content, .. } => content,
            other => panic!("expected a User message, got {other:?}"),
        };
        let media_type = content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Image {
                    source: ImageSource::Base64 { media_type, .. },
                } => Some(media_type.as_str()),
                _ => None,
            })
            .expect("a base64 image block");
        assert_eq!(media_type, "image/png");
    }

    /// A valid minimal 1-page PDF with a proper xref table + startxref. lopdf
    /// 0.34 rejects PDFs without an xref table, so this fixture carries one.
    #[cfg(feature = "pdf-read")]
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n186\n%%EOF\n";

    #[cfg(feature = "pdf-read")]
    #[tokio::test]
    async fn pdf_pages_over_cap_errors() {
        // A `pages` range spanning >20 pages is refused with the byte-locked
        // "exceeds maximum" error, before any extraction is attempted. This
        // exercises the routing regardless of whether the bytes parse.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("doc.pdf");
        std::fs::write(&target, MINIMAL_PDF).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "pages": "1-25" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeds maximum"),
            "got: {err}"
        );
    }

    #[cfg(feature = "pdf-read")]
    #[tokio::test]
    async fn reads_small_pdf_as_inline_document() {
        use protocol::{ContentBlock, ConversationMessage, DocumentSource};

        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("doc.pdf");
        std::fs::write(&target, MINIMAL_PDF).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let res = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("small PDF read must succeed");

        // The tool-result data is text-only and tagged as a PDF.
        assert_eq!(res.data["type"], "pdf");

        // The PDF bytes ride on a single `new_messages` user message carrying a
        // base64 `ContentBlock::Document` with the application/pdf media type.
        assert_eq!(res.new_messages.len(), 1);
        let content = match &res.new_messages[0] {
            ConversationMessage::User { content, .. } => content,
            other => panic!("expected a User message, got {other:?}"),
        };
        let media_type = content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Document {
                    source: DocumentSource::Base64 { media_type, .. },
                } => Some(media_type.as_str()),
                _ => None,
            })
            .expect("a base64 document block");
        assert_eq!(media_type, "application/pdf");
    }
}
