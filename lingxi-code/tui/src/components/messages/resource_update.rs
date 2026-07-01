//! `UserResourceUpdateMessage` — `↻ server: target · reason` lines.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - glyph: `↻` (REFRESH_ARROW U+21BB, claude-code `success`); `{target}`
//!     uses `suggestion`. M7-05 collapsed the whole line to a single `DIM`
//!     `Text`. `Theme` now has `success` + `suggestion` fields (added in
//!     M7-15), so a per-span recolor is straightforward — but the per-run
//!     split is re-deferred to keep the single-row layout — TODO(M8).
//!   - line: `↻ {server}: {target}[ · {reason}]` (separator ` · `, U+00B7)
//!   - file:// URIs → basename; non-file URIs len>40 → 39 chars + `…`
//!   source: claude-code/src/components/messages/UserResourceUpdateMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;
use crate::render_iocraft::StyleColorIocraftExt;

/// Refresh-arrow glyph (U+21BB, claude-code `REFRESH_ARROW`).
pub const REFRESH_ARROW: &str = "\u{21BB}";

/// One parsed update.
#[derive(Debug, Clone)]
pub struct ResourceUpdate {
    /// MCP server name.
    pub server: String,
    /// Resource URI (resource kind) or tool name (polling kind).
    pub target: String,
    /// Optional human-readable reason.
    pub reason: Option<String>,
}

/// Format a URI for display: `file://` → basename; non-file len>40 → 39 chars
/// + `…` (claude-code `formatUri`).
#[must_use]
pub fn format_uri(uri: &str) -> String {
    if let Some(path) = uri.strip_prefix("file://") {
        return path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(path)
            .to_string();
    }
    if uri.chars().count() > 40 {
        let head: String = uri.chars().take(39).collect();
        return format!("{head}\u{2026}");
    }
    uri.to_string()
}

/// Pure string renderer: one line per update, joined by `\n`.
#[must_use]
pub fn render_resource_update_to_string(updates: &[ResourceUpdate]) -> String {
    updates
        .iter()
        .map(|u| {
            let mut line = format!("{REFRESH_ARROW} {}: {}", u.server, u.target);
            if let Some(r) = &u.reason {
                line.push_str(&format!(" \u{00B7} {r}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Props for [`UserResourceUpdateMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserResourceUpdateProps {
    /// `(server, target, reason)` triples.
    pub updates: Vec<(String, String, Option<String>)>,
}

/// iocraft component.
#[component]
pub fn UserResourceUpdateMessage(
    props: &UserResourceUpdateProps,
) -> impl Into<AnyElement<'static>> {
    let updates: Vec<ResourceUpdate> = props
        .updates
        .iter()
        .map(|(s, t, r)| ResourceUpdate {
            server: s.clone(),
            target: t.clone(),
            reason: r.clone(),
        })
        .collect();
    let body = render_resource_update_to_string(&updates);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_arrow_bytes() {
        // U+21BB = 0xE2 0x86 0xBB.
        assert_eq!(REFRESH_ARROW.as_bytes(), &[0xE2, 0x86, 0xBB]);
    }

    #[test]
    fn format_uri_file_basename() {
        assert_eq!(format_uri("file:///a/b/c.rs"), "c.rs");
    }

    #[test]
    fn format_uri_long_truncates() {
        let long = format!("https://{}", "x".repeat(60));
        let out = format_uri(&long);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(out.chars().count(), 40);
    }

    #[test]
    fn line_with_reason() {
        let u = ResourceUpdate {
            server: "fs".into(),
            target: "x.rs".into(),
            reason: Some("changed".into()),
        };
        assert_eq!(
            render_resource_update_to_string(&[u]),
            "\u{21BB} fs: x.rs \u{00B7} changed"
        );
    }

    #[test]
    fn no_reason_no_dot() {
        let u = ResourceUpdate {
            server: "s".into(),
            target: "t".into(),
            reason: None,
        };
        assert_eq!(render_resource_update_to_string(&[u]), "\u{21BB} s: t");
    }
}
