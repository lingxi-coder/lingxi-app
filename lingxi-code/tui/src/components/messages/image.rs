//! `UserImageMessage` — `[Image #N]` / `[Image]` placeholder + metadata.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - label: `[Image #{imageId}]` with id, else `[Image]`
//!   - SCOPE: placeholder only — inline terminal image display + hyperlinks
//!     are M8 (terminal-protocol cluster). Metadata suffix is a LingXi
//!     addition for the headless placeholder, shown in parens.
//!   source: claude-code/src/components/messages/UserImageMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;
use crate::render_iocraft::StyleColorIocraftExt;

/// Build the placeholder label (+ optional metadata suffix).
#[must_use]
pub fn render_image_label(image_id: Option<u64>, metadata: Option<&str>) -> String {
    let base = match image_id {
        Some(id) => format!("[Image #{id}]"),
        None => "[Image]".to_string(),
    };
    match metadata {
        Some(m) if !m.is_empty() => format!("{base} ({m})"),
        _ => base,
    }
}

/// Props for [`UserImageMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserImageProps {
    /// Stored image id (drives `#N`).
    pub image_id: Option<u64>,
    /// Optional metadata suffix.
    pub metadata: Option<String>,
}

/// iocraft component.
#[component]
pub fn UserImageMessage(props: &UserImageProps) -> impl Into<AnyElement<'static>> {
    let label = render_image_label(props.image_id, props.metadata.as_deref());
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: label, color: TuiTheme::USER.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_label() {
        assert_eq!(render_image_label(None, None), "[Image]");
    }

    #[test]
    fn label_with_id() {
        assert_eq!(render_image_label(Some(3), None), "[Image #3]");
    }

    #[test]
    fn label_with_metadata() {
        assert_eq!(
            render_image_label(Some(1), Some("800x600")),
            "[Image #1] (800x600)"
        );
    }
}
