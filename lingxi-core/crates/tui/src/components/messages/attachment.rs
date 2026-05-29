//! `AttachmentMessage` — solo-user attachment summary lines.
//!
//! Literal locks (byte-for-byte from claude-code `AttachmentMessage.tsx`):
//!   - "Listed directory {path}/", "Read {path} ({n}[+] lines)",
//!     "Referenced file {path}", "Referenced PDF {path} ({n} pages)",
//!     "⧉ Selected {n} lines from {path} in {ide}", "Loaded {path}",
//!     "Read MCP resource {name} from {server}",
//!     "Plan file referenced ({path})", "Skills restored ({names})"
//!   - SCOPE: solo-user kinds only; team/swarm/hook/diagnostics kinds → M8.
//!     The `Line` helper renders dim text; bold spans wrap the path/count in
//!     claude-code — they collapse to plain in the string form.
//!   source: claude-code/src/components/messages/AttachmentMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `⧉` selected-lines glyph (U+29C9).
pub const SELECTED_GLYPH: &str = "\u{29C9}";

/// A solo-user attachment kind.
#[derive(Debug, Clone)]
pub enum Attachment {
    /// Directory listing.
    Directory {
        /// Display path of the listed directory.
        display_path: String,
    },
    /// File read.
    File {
        /// Display path of the read file.
        display_path: String,
        /// Number of lines read.
        num_lines: u64,
        /// `true` → file was truncated (append `+`).
        truncated: bool,
    },
    /// Compact file reference.
    CompactFileReference {
        /// Display path of the referenced file.
        display_path: String,
    },
    /// PDF reference.
    PdfReference {
        /// Display path of the referenced PDF.
        display_path: String,
        /// Number of pages.
        page_count: u64,
    },
    /// IDE-selected lines.
    SelectedLines {
        /// Number of selected lines.
        count: u64,
        /// Display path of the file.
        display_path: String,
        /// IDE name.
        ide_name: String,
    },
    /// Nested memory file loaded.
    NestedMemory {
        /// Display path of the loaded memory file.
        display_path: String,
    },
    /// MCP resource read.
    McpResource {
        /// Resource name.
        name: String,
        /// MCP server name.
        server: String,
    },
    /// Plan file referenced.
    PlanFileReference {
        /// Plan file path.
        plan_file_path: String,
    },
    /// Skills restored.
    InvokedSkills {
        /// Comma-joined skill names.
        skill_names: Vec<String>,
    },
}

/// Pure string renderer for one attachment line.
#[must_use]
pub fn render_attachment_to_string(a: &Attachment) -> String {
    match a {
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
    }
}

/// Props for [`AttachmentMessage`].
#[derive(Debug, Clone, Props)]
pub struct AttachmentProps {
    /// The attachment to render.
    pub attachment: Attachment,
}

impl Default for AttachmentProps {
    fn default() -> Self {
        Self {
            attachment: Attachment::Directory {
                display_path: String::new(),
            },
        }
    }
}

/// iocraft component (dim `Line`).
#[component]
pub fn AttachmentMessage(props: &AttachmentProps) -> impl Into<AnyElement<'static>> {
    let line = render_attachment_to_string(&props.attachment);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: line, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_glyph_bytes() {
        // U+29C9 = 0xE2 0xA7 0x89.
        assert_eq!(SELECTED_GLYPH.as_bytes(), &[0xE2, 0xA7, 0x89]);
    }

    #[test]
    fn directory_line() {
        let a = Attachment::Directory {
            display_path: "src".into(),
        };
        assert_eq!(render_attachment_to_string(&a), "Listed directory src/");
    }

    #[test]
    fn file_line_truncated() {
        let a = Attachment::File {
            display_path: "a.rs".into(),
            num_lines: 120,
            truncated: true,
        };
        assert_eq!(render_attachment_to_string(&a), "Read a.rs (120+ lines)");
    }

    #[test]
    fn file_line_untruncated() {
        let a = Attachment::File {
            display_path: "a.rs".into(),
            num_lines: 10,
            truncated: false,
        };
        assert_eq!(render_attachment_to_string(&a), "Read a.rs (10 lines)");
    }

    #[test]
    fn selected_lines() {
        let a = Attachment::SelectedLines {
            count: 3,
            display_path: "x.rs".into(),
            ide_name: "VSCode".into(),
        };
        assert_eq!(
            render_attachment_to_string(&a),
            "\u{29C9} Selected 3 lines from x.rs in VSCode"
        );
    }

    #[test]
    fn invoked_skills_join() {
        let a = Attachment::InvokedSkills {
            skill_names: vec!["a".into(), "b".into()],
        };
        assert_eq!(render_attachment_to_string(&a), "Skills restored (a, b)");
    }
}
