//! Per-variant history cells for user attachments: attachment summary lines,
//! MCP resource updates, and image attachments.
//!
//! Split out of `message.rs` in the second message-cells phase (plan Phase
//! 9). The styled-line renderers here are the single source for these
//! variants: the cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.
//!
//! [`attachment_lines`] covers every [`Attachment`] kind explicitly with the
//! claude-code parity strings (ported from the iocraft
//! `messages::attachment::render_attachment_to_string`, itself byte-locked to
//! `AttachmentMessage.tsx`) — this replaces the pre-split `"[attachment]"`
//! sub-wildcard that flattened the PDF/selected-lines/MCP/plan/skills kinds.

use std::path::Path;

use tui_core::message::Attachment;
use tui_core::render::StyledLine;
use tui_core::theme::Theme;

use super::{colored_lines, ScrollbackEscape, StyledCell};
use crate::term_image::ImageProtocol;

/// `⧉` selected-lines glyph (U+29C9), claude-code parity.
pub(crate) const SELECTED_GLYPH: &str = "\u{29C9}";

/// One dim summary line per attachment kind — every kind explicit, strings
/// byte-locked to claude-code `AttachmentMessage.tsx` (via the iocraft port).
pub(crate) fn attachment_lines(attachment: &Attachment, theme: &Theme) -> Vec<StyledLine> {
    let text = match attachment {
        Attachment::Directory { display_path } => format!("Listed directory {display_path}/"),
        Attachment::File {
            display_path,
            num_lines,
            truncated,
        } => {
            let plus = if *truncated { "+" } else { "" };
            format!("Read {display_path} ({num_lines}{plus} lines)")
        }
        Attachment::CompactFileReference { display_path } => {
            format!("Referenced file {display_path}")
        }
        Attachment::PdfReference {
            display_path,
            page_count,
        } => format!("Referenced PDF {display_path} ({page_count} pages)"),
        Attachment::SelectedLines {
            count,
            display_path,
            ide_name,
        } => format!("{SELECTED_GLYPH} Selected {count} lines from {display_path} in {ide_name}"),
        Attachment::NestedMemory { display_path } => format!("Loaded {display_path}"),
        Attachment::McpResource { name, server } => {
            format!("Read MCP resource {name} from {server}")
        }
        Attachment::PlanFileReference { plan_file_path } => {
            format!("Plan file referenced ({plan_file_path})")
        }
        Attachment::InvokedSkills { skill_names } => {
            format!("Skills restored ({})", skill_names.join(", "))
        }
    };
    colored_lines(&text, theme.dim)
}

/// MCP resource/polling update lines: `resource updated: {server}/{target}`
/// per update (an empty update list renders nothing).
pub(crate) fn resource_update_lines(
    updates: &[(String, String, Option<String>)],
) -> Vec<StyledLine> {
    updates
        .iter()
        .map(|(server, target, _)| {
            StyledLine::plain(format!("resource updated: {server}/{target}"))
        })
        .collect()
}

/// The image text placeholder: `[Image #N]`/`[Image]` + optional
/// ` ({metadata})` suffix. ALWAYS rendered — real inline display is a
/// supplement on top, never a replacement.
pub(crate) fn user_image_lines(image_id: Option<u64>, metadata: Option<&str>) -> Vec<StyledLine> {
    let head = match image_id {
        Some(n) => format!("[Image #{n}]"),
        None => "[Image]".to_string(),
    };
    let text = match metadata {
        Some(m) => format!("{head} ({m})"),
        None => head,
    };
    vec![StyledLine::plain(text)]
}

/// [`RenderedMessage::Attachment`](tui_core::message::RenderedMessage::Attachment)
/// — the dim one-line attachment summary (all kinds explicit).
#[derive(Debug)]
pub struct AttachmentCell {
    attachment: Attachment,
}

impl AttachmentCell {
    /// Wrap a parsed attachment.
    #[must_use]
    pub fn new(attachment: Attachment) -> Self {
        Self { attachment }
    }
}

impl StyledCell for AttachmentCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        attachment_lines(&self.attachment, theme)
    }
}

/// [`RenderedMessage::UserResourceUpdate`](tui_core::message::RenderedMessage::UserResourceUpdate)
/// — one plain line per resource update. An empty update list is the
/// documented empty-input hidden case (renders nothing).
#[derive(Debug)]
pub struct UserResourceUpdateCell {
    updates: Vec<(String, String, Option<String>)>,
}

impl UserResourceUpdateCell {
    /// Wrap the parsed update triples.
    #[must_use]
    pub fn new(updates: Vec<(String, String, Option<String>)>) -> Self {
        Self { updates }
    }
}

impl StyledCell for UserResourceUpdateCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        resource_update_lines(&self.updates)
    }
}

/// Assumed cell height in pixels when converting image pixel height to
/// terminal rows (the same 8×16 fallback font metric
/// [`crate::image_view::make_picker`] uses when the terminal can't be
/// queried).
const CELL_PIXEL_HEIGHT: u32 = 16;

/// Cap on the rows an inline image may claim in scrollback (keeps a tall
/// image from dwarfing the terminal; protocols scale it down to fit).
const MAX_IMAGE_ROWS: u16 = 12;

/// [`RenderedMessage::UserImage`](tui_core::message::RenderedMessage::UserImage)
/// — the `[Image #N]` text placeholder, always visible. When the message
/// carries a `source_path` AND the terminal speaks an inline-image protocol,
/// the cell additionally contributes a [`ScrollbackEscape`] that displays the
/// real image below the placeholder (plan Phase 9 step 7).
#[derive(Debug)]
pub struct UserImageCell {
    image_id: Option<u64>,
    metadata: Option<String>,
    source_path: Option<String>,
    protocol: ImageProtocol,
}

impl UserImageCell {
    /// Wrap an image attachment, detecting the terminal's inline-image
    /// protocol from the live environment.
    #[must_use]
    pub fn new(
        image_id: Option<u64>,
        metadata: Option<String>,
        source_path: Option<String>,
    ) -> Self {
        Self::with_protocol(image_id, metadata, source_path, crate::term_image::detect())
    }

    /// Wrap an image attachment with an explicit protocol (tests / callers
    /// that already probed the terminal).
    #[must_use]
    pub fn with_protocol(
        image_id: Option<u64>,
        metadata: Option<String>,
        source_path: Option<String>,
        protocol: ImageProtocol,
    ) -> Self {
        Self {
            image_id,
            metadata,
            source_path,
            protocol,
        }
    }

    /// The on-disk image path, when known.
    #[must_use]
    pub fn source_path(&self) -> Option<&str> {
        self.source_path.as_deref()
    }

    /// The optional metadata suffix (dims/file name).
    #[must_use]
    pub fn metadata(&self) -> Option<&str> {
        self.metadata.as_deref()
    }
}

impl StyledCell for UserImageCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_image_lines(self.image_id, self.metadata.as_deref())
    }

    /// The real inline image, when everything lines up: a known source path,
    /// a graphics-capable terminal, and a decodable image file (the header
    /// read doubles as validation). Row count follows the image's pixel
    /// height at the fallback font metric, capped at [`MAX_IMAGE_ROWS`].
    fn scrollback_escape(&self) -> Option<ScrollbackEscape> {
        if !self.protocol.supports_inline_images() {
            return None;
        }
        let path = Path::new(self.source_path.as_deref()?);
        let (_, height_px) = crate::image_view::image_pixel_size(path)?;
        let rows = u16::try_from(height_px.div_ceil(CELL_PIXEL_HEIGHT))
            .unwrap_or(MAX_IMAGE_ROWS)
            .clamp(1, MAX_IMAGE_ROWS);
        let escape = crate::term_image::render_inline_image(path, self.protocol, rows)?;
        Some(ScrollbackEscape { rows, escape })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell) -> Vec<String> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Every attachment kind renders its claude-code parity string — none
    /// falls back to a generic placeholder (the pre-split sub-wildcard).
    #[test]
    fn attachment_cell_renders_every_kind_with_parity_strings() {
        let cases: Vec<(Attachment, &str)> = vec![
            (
                Attachment::Directory {
                    display_path: "src".into(),
                },
                "Listed directory src/",
            ),
            (
                Attachment::File {
                    display_path: "a.rs".into(),
                    num_lines: 120,
                    truncated: true,
                },
                "Read a.rs (120+ lines)",
            ),
            (
                Attachment::File {
                    display_path: "a.rs".into(),
                    num_lines: 10,
                    truncated: false,
                },
                "Read a.rs (10 lines)",
            ),
            (
                Attachment::CompactFileReference {
                    display_path: "notes.md".into(),
                },
                "Referenced file notes.md",
            ),
            (
                Attachment::PdfReference {
                    display_path: "spec.pdf".into(),
                    page_count: 3,
                },
                "Referenced PDF spec.pdf (3 pages)",
            ),
            (
                Attachment::SelectedLines {
                    count: 4,
                    display_path: "main.rs".into(),
                    ide_name: "VS Code".into(),
                },
                "\u{29C9} Selected 4 lines from main.rs in VS Code",
            ),
            (
                Attachment::NestedMemory {
                    display_path: "MEMORY.md".into(),
                },
                "Loaded MEMORY.md",
            ),
            (
                Attachment::McpResource {
                    name: "logs".into(),
                    server: "grafana".into(),
                },
                "Read MCP resource logs from grafana",
            ),
            (
                Attachment::PlanFileReference {
                    plan_file_path: "plan.md".into(),
                },
                "Plan file referenced (plan.md)",
            ),
            (
                Attachment::InvokedSkills {
                    skill_names: vec!["deploy".into(), "review".into()],
                },
                "Skills restored (deploy, review)",
            ),
        ];
        for (attachment, expected) in cases {
            let cell = AttachmentCell::new(attachment);
            assert_eq!(plain(&cell), vec![expected.to_string()]);
        }
    }

    #[test]
    fn attachment_cell_lines_are_dim() {
        let cell = AttachmentCell::new(Attachment::Directory {
            display_path: "src".into(),
        });
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(crate::style_adapter::to_ratatui(Theme::dark().dim)),
            "attachments render dim (iocraft parity)"
        );
    }

    #[test]
    fn resource_update_cell_renders_one_line_per_update() {
        let cell = UserResourceUpdateCell::new(vec![
            (
                "grafana".to_string(),
                "logs".to_string(),
                Some("changed".to_string()),
            ),
            ("jira".to_string(), "tickets".to_string(), None),
        ]);
        assert_eq!(
            plain(&cell),
            vec![
                "resource updated: grafana/logs".to_string(),
                "resource updated: jira/tickets".to_string()
            ]
        );
    }

    #[test]
    fn resource_update_cell_with_no_updates_is_documented_hidden() {
        // Empty input → nothing to say (the documented empty-input case).
        let cell = UserResourceUpdateCell::new(Vec::new());
        assert!(plain(&cell).is_empty());
        assert!(!cell.is_visible(80));
    }

    #[test]
    fn user_image_cell_always_renders_the_text_placeholder() {
        let full = UserImageCell::new(Some(3), Some("640x480".to_string()), None);
        assert_eq!(plain(&full), vec!["[Image #3] (640x480)".to_string()]);
        assert_eq!(full.metadata(), Some("640x480"));
        assert_eq!(full.source_path(), None);

        let bare = UserImageCell::new(None, None, None);
        assert_eq!(plain(&bare), vec!["[Image]".to_string()]);

        // The placeholder stays even when a real source path exists — inline
        // display supplements the text fallback, never replaces it.
        let with_path = UserImageCell::with_protocol(
            Some(1),
            Some("pic.png".to_string()),
            Some("/tmp/pic.png".to_string()),
            ImageProtocol::None,
        );
        assert_eq!(plain(&with_path), vec!["[Image #1] (pic.png)".to_string()]);
        assert_eq!(with_path.source_path(), Some("/tmp/pic.png"));
    }

    /// Write a real decodable PNG (`width`×`height` px) to a temp path.
    fn temp_png(tag: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("tui-rata-cells2-{tag}-{}.png", std::process::id()));
        image::RgbaImage::new(width, height)
            .save(&path)
            .expect("write test png");
        path
    }

    #[test]
    fn image_scrollback_escape_needs_path_protocol_and_decodable_file() {
        // No source path → no escape, whatever the terminal speaks.
        let no_path = UserImageCell::with_protocol(Some(1), None, None, ImageProtocol::Kitty);
        assert_eq!(HistoryCell::scrollback_escape(&no_path), None);

        // Graphics-incapable terminal → no escape even with a real file. The
        // text placeholder is the whole render.
        let png = temp_png("noproto", 4, 4);
        let no_proto = UserImageCell::with_protocol(
            Some(1),
            None,
            Some(png.display().to_string()),
            ImageProtocol::None,
        );
        assert_eq!(HistoryCell::scrollback_escape(&no_proto), None);

        // A non-image file fails the header validation → no escape (never
        // spew base64 of arbitrary bytes at the terminal).
        let bogus =
            std::env::temp_dir().join(format!("tui-rata-cells2-bogus-{}.png", std::process::id()));
        std::fs::write(&bogus, b"definitely not a png").unwrap();
        let not_an_image = UserImageCell::with_protocol(
            Some(1),
            None,
            Some(bogus.display().to_string()),
            ImageProtocol::Kitty,
        );
        assert_eq!(HistoryCell::scrollback_escape(&not_an_image), None);

        // Sixel has no byte-stream encoder here → text fallback only.
        let sixel = UserImageCell::with_protocol(
            Some(1),
            None,
            Some(png.display().to_string()),
            ImageProtocol::Sixel,
        );
        assert_eq!(HistoryCell::scrollback_escape(&sixel), None);

        std::fs::remove_file(&png).ok();
        std::fs::remove_file(&bogus).ok();
    }

    #[test]
    fn image_scrollback_escape_scales_rows_to_pixel_height_with_cap() {
        // 40 px tall at the 16 px/row metric → 3 rows.
        let png = temp_png("rows", 4, 40);
        let cell = UserImageCell::with_protocol(
            Some(1),
            None,
            Some(png.display().to_string()),
            ImageProtocol::Kitty,
        );
        let escape = HistoryCell::scrollback_escape(&cell).expect("kitty escape");
        assert_eq!(escape.rows, 3);
        assert!(
            escape.escape.starts_with("\x1b_Ga=T,f=100,r=3,"),
            "kitty escape scaled to the reserved rows: {:.40}",
            escape.escape
        );

        // A very tall image caps at MAX_IMAGE_ROWS.
        let tall = temp_png("tall", 4, 1000);
        let cell = UserImageCell::with_protocol(
            Some(2),
            None,
            Some(tall.display().to_string()),
            ImageProtocol::ITerm2,
        );
        let escape = HistoryCell::scrollback_escape(&cell).expect("iterm2 escape");
        assert_eq!(escape.rows, MAX_IMAGE_ROWS);
        assert!(
            escape.escape.starts_with("\x1b]1337;File=inline=1;size="),
            "iTerm2 escape: {:.40}",
            escape.escape
        );
        assert!(escape
            .escape
            .contains(&format!(";height={MAX_IMAGE_ROWS};")));

        std::fs::remove_file(&png).ok();
        std::fs::remove_file(&tall).ok();
    }
}
