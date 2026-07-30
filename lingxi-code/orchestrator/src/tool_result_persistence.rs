//! Tool-result persistence — 1:1 port of claude-code 2.1.220's `F0u` /
//! `x2e` / `Alt` / `xKr` trio (the `<persisted-output>` substitution).
//!
//! When a tool's model-facing `tool_result` content exceeds the tool's
//! persistence threshold, the FULL content is written to
//! `<config_home>/projects/<project_dir_name(cwd)>/<session-uuid>/tool-results/<id>.txt`
//! and the model instead receives a short `<persisted-output>` envelope
//! carrying a preview plus the on-disk path.
//!
//! Oracle anchors (all `LC_ALL=C grep -abo -F` offsets into
//! `~/.local/share/claude/versions/2.1.220`):
//!
//! - `230268660` — the constant run: `var AKr=50000, gor=500000, RKr=4,
//!   D0u=400000, O0u=200000, i3=50, P0u=1e4`.
//! - `230268813` — `function qzg(){return path.join(F7(gn()),kt())}`
//!   (`<projects>/<sanitized-cwd>/<sessionId>`).
//! - `230268859` — `function xke(){return path.join(qzg(),was)}` with
//!   `var was="tool-results"`.
//! - `230268971` — `async function k2e(){try{await Gi().mkdir(xke())}catch{}}`
//!   (directory creation, errors swallowed).
//! - `230269313` — `x2e`: the exclusive write + preview.
//! - `230269820` — `Alt`: the envelope (byte-verified with `od -c`).
//! - `230270568` — `F0u`: the empty-result / media / size guards.
//! - `230270990` — `xKr`: the preview slicer.
//! - `226176853` — `pl`: the byte formatter.
//!
//! # `original_size` is a BYTE length
//!
//! Verified against the real binary's own on-disk output: of the 111 files
//! under `~/.claude/projects/**/tool-results/` whose UTF-8 byte length and
//! decoded char length format to DIFFERENT `pl()` strings, all 13 that could
//! be paired back to their transcript line report the **byte** rendering
//! (e.g. `beid1bot5` — 31 546 bytes / 31 483 chars — is reported as `30.8KB`,
//! the byte form, not `30.7KB`). Rust's `String::len()` is therefore the
//! exact analogue and no char conversion is performed anywhere in this
//! module.

use std::path::{Path, PathBuf};

/// `Clt` (2.1.220 @ 230276246) — opening tag of the substituted envelope.
pub const PERSISTED_OUTPUT_OPEN: &str = "<persisted-output>";

/// `Cas` — closing tag of the substituted envelope.
pub const PERSISTED_OUTPUT_CLOSE: &str = "</persisted-output>";

/// `_or = 2000` — how much of the persisted body is echoed back to the model.
pub const PREVIEW_CHARS: usize = 2_000;

/// `AKr = 50000` — the default `persistenceThresholdCeiling` applied by `M0u`
/// when a tool does not declare its own.
pub const DEFAULT_PERSISTENCE_CEILING: usize = 50_000;

/// `RKr = 4` — chars per token, used only for the analytics estimate.
pub const CHARS_PER_TOKEN: usize = 4;

/// `was = "tool-results"` — the per-session subdirectory name.
pub const TOOL_RESULTS_DIR: &str = "tool-results";

/// `M0u`'s ceiling fold (2.1.220 @ 230269046):
/// `Math.min(tool.maxResultSizeChars, tool.persistenceThresholdCeiling ?? AKr)`.
///
/// `Number.isFinite` fails for the `1/0` tools (`Read`), which `None` models —
/// they never persist. Callers hand the already-folded value to
/// [`crate::turn_loop`] via `Tool::persistence_threshold`, so this is the
/// arithmetic each tool's override performs.
///
/// The per-tool `tengu_velvet_ibis` numeric override that `M0u` consults ahead
/// of the fold is NOT modelled: the gate map is `{}` in the binary default and
/// `{}` in the live `~/.claude.json` `.cachedGrowthBookFeatures`, so it always
/// falls through to this `min`.
#[must_use]
pub fn resolve_threshold(max_result_size_chars: usize, ceiling: Option<usize>) -> usize {
    max_result_size_chars.min(ceiling.unwrap_or(DEFAULT_PERSISTENCE_CEILING))
}

/// Result of a successful [`persist`] call — the `x2e` return shape.
#[derive(Debug, Clone)]
pub struct Persisted {
    /// Absolute path the full content was written to.
    pub filepath: PathBuf,
    /// BYTE length of the persisted body (`o.length`, see the module docs).
    pub original_size: usize,
    /// Whether the body was serialized from a content ARRAY (`.json`).
    pub is_json: bool,
    /// The leading slice echoed back to the model.
    pub preview: String,
    /// Whether the body was longer than the preview.
    pub has_more: bool,
}

/// Port of claude-code `pl` (2.1.220 @ 226176853):
/// `t=n/1024; t<1 -> "${n} bytes"; t<1024 -> "${t.toFixed(1).replace(/\.0$/,"")}KB"`,
/// then MB, then GB.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(n: usize) -> String {
    /// `${x.toFixed(1).replace(/\.0$/,"")}` — one decimal, trailing `.0` dropped.
    fn fixed1(x: f64) -> String {
        let s = format!("{x:.1}");
        match s.strip_suffix(".0") {
            Some(trimmed) => trimmed.to_string(),
            None => s,
        }
    }
    let kb = n as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{n} bytes");
    }
    if kb < 1024.0 {
        return format!("{}KB", fixed1(kb));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{}MB", fixed1(mb));
    }
    format!("{}GB", fixed1(mb / 1024.0))
}

/// Port of claude-code `xKr` (2.1.220 @ 230270990):
///
/// ```text
/// if(e.length<=t)return{preview:e,hasMore:!1};
/// let n=e.slice(0,t).lastIndexOf("\n"),o=n>t*0.5?n:t;
/// return{preview:e.slice(0,o),hasMore:!0}
/// ```
///
/// Byte-indexed like the rest of this module; the initial cut is walked back
/// to the nearest UTF-8 char boundary so slicing can never panic.
#[must_use]
pub fn preview(s: &str, limit: usize) -> (&str, bool) {
    if s.len() <= limit {
        return (s, false);
    }
    // `e.slice(0,t)` — walked back to the nearest char boundary so a
    // multi-byte codepoint straddling the limit is dropped whole.
    let mut hard = limit;
    while hard > 0 && !s.is_char_boundary(hard) {
        hard -= 1;
    }
    let head = &s[..hard];
    // `n=…lastIndexOf("\n"), o = n > t*0.5 ? n : t` — the newline index is
    // taken only when it lands in the BACK half of the window. `n * 2 > limit`
    // avoids the float compare (and `lastIndexOf` returning -1 maps to `None`).
    let cut = match head.rfind('\n') {
        Some(i) if i.saturating_mul(2) > limit => i,
        _ => hard,
    };
    (&s[..cut], true)
}

/// Port of claude-code `Alt` (2.1.220 @ 230269820, byte-verified via `od -c`).
#[must_use]
pub fn wrap(original_size: usize, filepath: &str, preview: &str, has_more: bool) -> String {
    // `let t=`${Clt}\n`;
    //  t+=`Output too large (${pl(e.originalSize)}). Full output saved to: ${e.filepath}\n\n`;
    //  t+=`Preview (first ${pl(_or)}):\n`; t+=e.preview;
    //  t+=e.hasMore?`\n...\n`:`\n`; t+=Cas`
    let tail = if has_more { "\n...\n" } else { "\n" };
    format!(
        "{PERSISTED_OUTPUT_OPEN}\nOutput too large ({}). Full output saved to: {filepath}\n\nPreview (first {}):\n{preview}{tail}{PERSISTED_OUTPUT_CLOSE}",
        format_bytes(original_size),
        format_bytes(PREVIEW_CHARS),
    )
}

/// `xke()` — `<config_home>/projects/<project_dir_name(cwd)>/<session_uuid>/tool-results`.
///
/// Reuses [`session::jsonl::path::project_dir_name`] so the sanitizer can
/// never drift from the transcript writer's.
#[must_use]
pub fn tool_results_dir(config_home: &Path, cwd: &str, session_uuid: &str) -> PathBuf {
    let mut p = config_home.to_path_buf();
    p.push("projects");
    p.push(session::jsonl::path::project_dir_name(cwd));
    p.push(session_uuid);
    p.push(TOOL_RESULTS_DIR);
    p
}

/// Port of claude-code `x2e` (2.1.220 @ 230269313).
///
/// Creates the directory (errors swallowed, `k2e`), writes `body` with an
/// EXCLUSIVE create (`writeExclusive`), and treats an `EEXIST` collision as
/// SUCCESS — falling through to the same envelope, exactly as the oracle's
/// `catch(a){if($t(a)!=="EEXIST") …}` does. Any other error is returned to the
/// caller, which then leaves the tool result unchanged.
///
/// # Errors
/// Returns the OS error message when the exclusive create fails for a reason
/// other than an already-existing path.
pub async fn persist(
    dir: &Path,
    id: &str,
    body: &str,
    is_json: bool,
) -> Result<Persisted, String> {
    // `k2e`: `try{await Gi().mkdir(xke())}catch{}`. The oracle's wrapper
    // creates the whole chain (its on-disk sessions carry sibling `subagents`
    // / `workflows` directories under the same session dir), so `create_dir_all`
    // is the observable analogue. Errors are swallowed exactly as `catch{}`
    // does — a failure here simply surfaces as the create error below.
    let _ = tokio::fs::create_dir_all(dir).await;

    // `kKr(e,t)`: `${e}.${t?"json":"txt"}`.
    let filepath = dir.join(format!("{id}.{}", if is_json { "json" } else { "txt" }));

    // `writeExclusive` → `O_CREAT | O_EXCL`. An EEXIST collision is TOLERATED
    // (the oracle's catch re-throws only for other codes) and falls through to
    // the same envelope, leaving the pre-existing file untouched.
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&filepath)
        .await
    {
        Ok(mut file) => {
            use tokio::io::AsyncWriteExt as _;
            if let Err(e) = file.write_all(body.as_bytes()).await {
                return Err(e.to_string());
            }
            if let Err(e) = file.flush().await {
                return Err(e.to_string());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }

    let (pv, has_more) = preview(body, PREVIEW_CHARS);
    Ok(Persisted {
        filepath,
        original_size: body.len(),
        is_json,
        preview: pv.to_string(),
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // T1 — `pl`. Values byte-locked against the real binary's own on-disk
    // census (`~/.claude/projects/**/tool-results/`) plus the transcript
    // strings that reference those files.
    #[test]
    fn format_bytes_matches_oracle_pl() {
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1023), "1023 bytes");
        // `pl(_or)` — the literal that appears in every real envelope.
        assert_eq!(format_bytes(2_000), "2KB");
        // b25o2lm9g.txt: 30 075 bytes on disk.
        assert_eq!(format_bytes(30_075), "29.4KB");
        // Largest persisted file observed with a MB rendering.
        assert_eq!(format_bytes(3_485_677), "3.3MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2GB");
        // `.toFixed(1).replace(/\.0$/,"")` — the trailing `.0` is dropped.
        assert_eq!(format_bytes(1024), "1KB");
    }

    // T2 — `xKr`.
    #[test]
    fn preview_cuts_at_last_newline_past_the_midpoint() {
        // Newline at index 1500 (> 2000 * 0.5) → cut there, newline excluded.
        let mut s = "a".repeat(1_500);
        s.push('\n');
        s.push_str(&"b".repeat(2_000));
        let (p, more) = preview(&s, PREVIEW_CHARS);
        assert!(more);
        assert_eq!(p.len(), 1_500);
        assert_eq!(p, "a".repeat(1_500));
    }

    #[test]
    fn preview_falls_back_to_the_hard_limit_when_the_newline_is_too_early() {
        // Newline at index 10 (<= 2000 * 0.5) → hard cut at 2000.
        let mut s = "a".repeat(10);
        s.push('\n');
        s.push_str(&"b".repeat(4_000));
        let (p, more) = preview(&s, PREVIEW_CHARS);
        assert!(more);
        assert_eq!(p.len(), PREVIEW_CHARS);
    }

    #[test]
    fn preview_passes_short_input_through() {
        let (p, more) = preview("hello", PREVIEW_CHARS);
        assert_eq!(p, "hello");
        assert!(!more);
        // `e.length<=t` — exactly at the limit is NOT truncated.
        let s = "x".repeat(PREVIEW_CHARS);
        let (p, more) = preview(&s, PREVIEW_CHARS);
        assert_eq!(p.len(), PREVIEW_CHARS);
        assert!(!more);
    }

    #[test]
    fn preview_never_splits_a_utf8_codepoint() {
        // 700 THREE-byte chars = 2100 bytes, no newline anywhere. 2000 is not
        // a multiple of 3, so the hard cut must walk back to 1998.
        let s = "→".repeat(700);
        assert_eq!(s.len(), 2_100);
        let (p, more) = preview(&s, PREVIEW_CHARS);
        assert!(more);
        assert_eq!(p.len(), 1_998);
        assert!(s.starts_with(p));
    }

    // T3 — `Alt`, byte-exact.
    #[test]
    fn wrap_is_byte_exact() {
        let out = wrap(30_075, "/tmp/s/tool-results/x.txt", "PREVIEW", true);
        assert_eq!(
            out,
            "<persisted-output>\nOutput too large (29.4KB). Full output saved to: \
             /tmp/s/tool-results/x.txt\n\nPreview (first 2KB):\nPREVIEW\n...\n</persisted-output>"
        );
    }

    #[test]
    fn wrap_without_more_uses_a_single_newline_tail() {
        let out = wrap(1_024, "/p.txt", "P", false);
        assert_eq!(
            out,
            "<persisted-output>\nOutput too large (1KB). Full output saved to: \
             /p.txt\n\nPreview (first 2KB):\nP\n</persisted-output>"
        );
    }

    // T4 — `x2e`.
    #[tokio::test]
    async fn persist_writes_txt_and_creates_the_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("sess").join(TOOL_RESULTS_DIR);
        let body = "hello world";
        let p = persist(&dir, "toolu_abc", body, false)
            .await
            .expect("persist ok");
        assert_eq!(p.filepath, dir.join("toolu_abc.txt"));
        assert_eq!(p.original_size, body.len());
        assert!(!p.is_json);
        assert!(!p.has_more);
        assert_eq!(p.preview, body);
        assert_eq!(
            std::fs::read_to_string(&p.filepath).expect("read back"),
            body
        );
    }

    #[tokio::test]
    async fn persist_uses_a_json_extension_for_array_bodies() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(TOOL_RESULTS_DIR);
        let p = persist(&dir, "id1", "[\n  1\n]", true)
            .await
            .expect("persist ok");
        assert_eq!(p.filepath, dir.join("id1.json"));
        assert!(p.is_json);
    }

    #[tokio::test]
    async fn persist_treats_an_existing_file_as_success() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(TOOL_RESULTS_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("id1.txt"), "OLD").expect("seed");
        let p = persist(&dir, "id1", "NEW-BODY", false)
            .await
            .expect("EEXIST is success");
        assert_eq!(p.filepath, dir.join("id1.txt"));
        // The oracle does NOT overwrite: `writeExclusive` threw, the catch
        // swallowed EEXIST, and the pre-existing file stays as it was.
        assert_eq!(
            std::fs::read_to_string(dir.join("id1.txt")).expect("read back"),
            "OLD"
        );
        // …but the envelope still describes the NEW body (`o.length`).
        assert_eq!(p.original_size, "NEW-BODY".len());
    }

    /// `M0u`'s fold, and the two constants it is built from.
    #[test]
    fn resolve_threshold_is_the_m0u_fold() {
        // `AKr` / `RKr` (2.1.220 @ 230268660).
        assert_eq!(DEFAULT_PERSISTENCE_CEILING, 50_000);
        assert_eq!(CHARS_PER_TOKEN, 4);
        // Bash / PowerShell: `min(30000, AKr)`.
        assert_eq!(resolve_threshold(30_000, None), 30_000);
        // A tool declaring more than the default ceiling is clamped to it.
        assert_eq!(resolve_threshold(300_000, None), 50_000);
        // The MCP factory raises the ceiling to `gor = 500000`.
        assert_eq!(resolve_threshold(300_000, Some(500_000)), 300_000);
        assert_eq!(resolve_threshold(900_000, Some(500_000)), 500_000);
    }

    #[test]
    fn tool_results_dir_is_session_scoped() {
        let d = tool_results_dir(Path::new("/home/u/.lingxi"), "/w/p", "uuid-1");
        assert_eq!(
            d,
            PathBuf::from("/home/u/.lingxi/projects/-w-p/uuid-1/tool-results")
        );
    }
}
