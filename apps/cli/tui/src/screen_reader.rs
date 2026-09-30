//! Flat-text accessibility serializer for the screen-reader render path.
//!
//! When `apps/cli/src/ax_screen_reader::is_enabled()` is true, the TUI must emit
//! a *linearized, flat-text* transcript of the screen instead of the visually
//! positioned ratatui frame, so an external screen reader speaks a coherent
//! stream. This module is the pure core of that alternate renderer: it walks an
//! in-memory accessibility node tree ([`AxNode`]) and produces the flat string.
//!
//! It is a 1:1 port of the binary's serializer over Ink's DOM. In `2.1.201`
//! (minified) the two functions are `U3t` (node → string) and `Xop` (box
//! flatten):
//!
//! ```js
//! function U3t(e,t){
//!   if(e.nodeName==="#text")return e.nodeValue;               // (0) raw text leaf
//!   let n=e.accessibility;
//!   if(n?.hidden)return"";                                    // (1) a11y-hidden → skip
//!   if(e.isHidden||e.yogaNode?.getDisplay()===1)return"";     //     display:none  → skip
//!   let r="";
//!   if(n?.label!==void 0)r=n.label;                           // (2) label substitution
//!   else if(e.nodeName==="ink-text"||e.nodeName==="ink-virtual-text"||e.nodeName==="ink-link")
//!     for(let o of e.childNodes)r+=U3t(o,n?.role??t);         //     text run: concat children
//!   else if(e.nodeName==="ink-box"||e.nodeName==="ink-root")
//!     r=Xop(e,n?.role??t);                                    //     box: flatten
//!   if(n?.state){                                             // (4) state prefix
//!     let o=Object.keys(n.state).filter((s)=>n.state[s]);
//!     if(o.length>0)r=`(${o.join(", ")}) ${r}`
//!   }
//!   if(n?.role&&n.role!==t)r=`${n.role}: ${r}`;               // (3) role prefix (role != parent)
//!   return r
//! }
//! function Xop(e,t){                                          // (5) box flatten
//!   let n=e.style.flexDirection??"row",
//!       r=n==="column"||n==="column-reverse",                 //     isColumn
//!       o=n==="row-reverse"||n==="column-reverse",            //     isReverse
//!       s=r?`\n`:" ",                                         //     column → "\n", row → " "
//!       i=[];
//!   for(let a of e.childNodes){let l=U3t(a,t);if(l!=="")i.push(l)}
//!   if(o)i.reverse();
//!   return i.join(s)
//! }
//! ```
//!
//! Note the wrapping ORDER inside `U3t`: the state prefix is applied first, then
//! the role prefix is prepended around it, so the final shape is
//! `role: (states) text` (role outermost). The role passed *down* to children is
//! `n?.role ?? t` (a node's own role shadows the inherited parent role), while
//! the role-difference test compares against the incoming parent role `t`.
//!
//! Nothing here is brand-specific; the transcript symbol map ([`SymbolKind`]) is
//! copied byte-for-byte from the binary's `Oao` aria-label table.

use std::fmt;

use crate::composer::EditDelta;

/// Environment propagation seam shared with `apps/cli::ax_screen_reader`.
pub const AX_SCREEN_READER_ENV: &str = "LINGXI_AX_SCREEN_READER";

/// Read the process-level accessibility mode inherited by the TUI.
///
/// The CLI resolves flag/env/config precedence once and sets this variable for
/// the mounted process and children. Any non-empty value is truthy, matching
/// the JavaScript gate.
#[must_use]
pub fn is_enabled() -> bool {
    std::env::var(AX_SCREEN_READER_ENV)
        .ok()
        .is_some_and(|value| !value.is_empty())
}

/// Convert one exact composer mutation into assistive-technology text.
///
/// Insertions announce only the inserted character/text. Spaces and newlines
/// use words VoiceOver reads correctly. Deletions preserve the removed text so
/// word/line deletion is unambiguous.
#[must_use]
pub fn input_announcement(delta: &EditDelta) -> Option<String> {
    if !delta.deleted.is_empty() {
        return Some(format!("Deleted {}", delta.deleted));
    }
    if delta.inserted.is_empty() {
        return None;
    }
    Some(match delta.inserted.as_str() {
        " " => "space".to_string(),
        "\n" => "new line".to_string(),
        text => text.to_string(),
    })
}

/// Flex direction of a box, mirroring Yoga's `flexDirection`. Encodes both the
/// join axis (row → `" "`, column → `"\n"`) and whether children are reversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl Orientation {
    /// `r` in `Xop`: `flexDirection === "column" || "column-reverse"`.
    fn is_column(self) -> bool {
        matches!(self, Orientation::Column | Orientation::ColumnReverse)
    }

    /// `o` in `Xop`: `flexDirection === "row-reverse" || "column-reverse"`.
    fn is_reverse(self) -> bool {
        matches!(self, Orientation::RowReverse | Orientation::ColumnReverse)
    }

    /// `s` in `Xop`: column axis joins with a newline, row axis with a space.
    fn separator(self) -> &'static str {
        if self.is_column() {
            "\n"
        } else {
            " "
        }
    }
}

/// The kind of a node, matching the Ink `nodeName`s the serializer branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// `#text` — a raw text leaf. Its value is returned verbatim, *before* any
    /// accessibility processing (hidden/label/state/role never apply to text).
    Text(String),
    /// `ink-text` / `ink-virtual-text` / `ink-link` — an inline text run whose
    /// children are concatenated with no separator.
    TextRun,
    /// `ink-box` / `ink-root` — a flex container flattened by [`flatten_box`].
    Box(Orientation),
}

/// The `accessibility` attribute bag attached to a node (Ink's `node.accessibility`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accessibility {
    /// `hidden: true` → the node (and subtree) serialize to the empty string.
    pub hidden: bool,
    /// `label` → substitutes for the node's own text content when present.
    pub label: Option<String>,
    /// `role` → prefixed as `"{role}: "` when it differs from the parent role,
    /// and shadows the inherited role for descendants.
    pub role: Option<String>,
    /// `state` object. Insertion order is preserved (JS `Object.keys` order);
    /// only entries whose flag is `true` are announced, joined by `", "`.
    pub state: Vec<(String, bool)>,
}

impl Accessibility {
    /// Convenience builder for a bare label.
    pub fn label(label: impl Into<String>) -> Self {
        Accessibility {
            label: Some(label.into()),
            ..Default::default()
        }
    }

    /// Convenience builder for a bare role.
    pub fn role(role: impl Into<String>) -> Self {
        Accessibility {
            role: Some(role.into()),
            ..Default::default()
        }
    }

    /// The active state keys in insertion order (the `Object.keys(...).filter`).
    fn active_states(&self) -> Vec<&str> {
        self.state
            .iter()
            .filter(|(_, on)| *on)
            .map(|(k, _)| k.as_str())
            .collect()
    }
}

/// A node in the accessibility tree the serializer walks. Callers build this from
/// the ratatui widget tree (see the module residuals for that wiring).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxNode {
    pub kind: NodeKind,
    /// `node.accessibility` (may be absent).
    pub accessibility: Option<Accessibility>,
    /// `node.isHidden || node.yogaNode?.getDisplay() === 1` (display:none). Kept
    /// separate from `accessibility.hidden`; either one skips the node.
    pub hidden: bool,
    pub children: Vec<AxNode>,
}

impl AxNode {
    /// A `#text` leaf.
    pub fn text(value: impl Into<String>) -> Self {
        AxNode {
            kind: NodeKind::Text(value.into()),
            accessibility: None,
            hidden: false,
            children: Vec::new(),
        }
    }

    /// An `ink-text` run over the given children.
    pub fn text_run(children: Vec<AxNode>) -> Self {
        AxNode {
            kind: NodeKind::TextRun,
            accessibility: None,
            hidden: false,
            children,
        }
    }

    /// An `ink-box` with the given orientation and children.
    pub fn boxed(orientation: Orientation, children: Vec<AxNode>) -> Self {
        AxNode {
            kind: NodeKind::Box(orientation),
            accessibility: None,
            hidden: false,
            children,
        }
    }

    /// Attach an accessibility bag (builder style).
    pub fn with_accessibility(mut self, accessibility: Accessibility) -> Self {
        self.accessibility = Some(accessibility);
        self
    }

    /// Mark the node display:none / isHidden (builder style).
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Serialize this node (as a render root) to the flat accessibility string.
    ///
    /// Equivalent to the binary's top-level `U3t(root, undefined)` — there is no
    /// inherited parent role at the root.
    pub fn to_flat_text(&self) -> String {
        serialize_node(self, None)
    }
}

/// `U3t(e, t)`: serialize one node given the inherited parent role `t`.
fn serialize_node(node: &AxNode, parent_role: Option<&str>) -> String {
    // (0) `#text` returns its value directly, before any accessibility handling.
    if let NodeKind::Text(value) = &node.kind {
        return value.clone();
    }

    let acc = node.accessibility.as_ref();

    // (1) `n?.hidden` and `isHidden || display:none` → skip (empty string).
    if acc.is_some_and(|a| a.hidden) || node.hidden {
        return String::new();
    }

    // The role context handed to descendants: a node's own role shadows the
    // inherited one (`n?.role ?? t`).
    let child_role = acc.and_then(|a| a.role.as_deref()).or(parent_role);

    // Build the base content `r`.
    let mut r = if let Some(label) = acc.and_then(|a| a.label.as_ref()) {
        // (2) label substitution wins over the node's own children.
        label.clone()
    } else {
        match &node.kind {
            NodeKind::TextRun => {
                // Concatenate children with no separator.
                let mut buf = String::new();
                for child in &node.children {
                    buf.push_str(&serialize_node(child, child_role));
                }
                buf
            }
            // (5) box flatten.
            NodeKind::Box(orientation) => flatten_box(node, *orientation, child_role),
            NodeKind::Text(_) => unreachable!("handled above"),
        }
    };

    if let Some(a) = acc {
        // (4) state prefix — applied first (innermost).
        let active = a.active_states();
        if !active.is_empty() {
            r = format!("({}) {}", active.join(", "), r);
        }
        // (3) role prefix — applied last (outermost), only when role != parent.
        if let Some(role) = &a.role {
            if Some(role.as_str()) != parent_role {
                r = format!("{role}: {r}");
            }
        }
    }

    r
}

/// `Xop(e, t)`: flatten a box's children, dropping empties, joining on the axis
/// separator, reversing when the flex direction is reversed.
fn flatten_box(node: &AxNode, orientation: Orientation, role_context: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for child in &node.children {
        let serialized = serialize_node(child, role_context);
        if !serialized.is_empty() {
            parts.push(serialized);
        }
    }
    if orientation.is_reverse() {
        parts.reverse();
    }
    parts.join(orientation.separator())
}

// ---------------------------------------------------------------------------
// Transcript symbol labeling (`Oao` aria-label table)
// ---------------------------------------------------------------------------

/// The status symbol kinds the transcript renders (the binary's `Oao` map). In
/// screen-reader mode the glyph is replaced by its `ariaLabel` so the reader
/// speaks a word instead of an unpronounceable icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Success,
    Error,
    Warning,
    Info,
    Pending,
    Loading,
}

impl SymbolKind {
    /// The `ariaLabel` for this symbol, copied byte-for-byte from `Oao`.
    pub fn aria_label(self) -> &'static str {
        match self {
            SymbolKind::Success => "done:",
            SymbolKind::Error => "failed:",
            SymbolKind::Warning => "warning:",
            SymbolKind::Info => "note:",
            SymbolKind::Pending => "pending:",
            SymbolKind::Loading => "loading:",
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.aria_label())
    }
}

// ---------------------------------------------------------------------------
// Word wrap + line diff (screen-reader line management helpers)
// ---------------------------------------------------------------------------

/// Wrap flat text to `width` columns for the screen-reader park region. Existing
/// newlines are preserved as hard breaks; each logical line is greedily wrapped
/// on ASCII spaces, and any single word longer than `width` is hard-split.
///
/// `width == 0` is treated as "no wrapping" (returns the input split on newlines).
pub fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        if width == 0 {
            out.push(line.to_string());
            continue;
        }
        wrap_line_into(line, width, &mut out);
    }
    out
}

fn wrap_line_into(line: &str, width: usize, out: &mut Vec<String>) {
    if line.is_empty() {
        out.push(String::new());
        return;
    }
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in line.split(' ') {
        let word_len = word.chars().count();
        // A word longer than the whole width is hard-split across lines.
        if word_len > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for ch in word.chars() {
                if chunk_width == width {
                    out.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(ch);
                chunk_width += 1;
            }
            current = chunk;
            current_width = chunk_width;
            continue;
        }
        // +1 for the joining space, unless this is the first word on the line.
        let needed = if current.is_empty() {
            word_len
        } else {
            current_width + 1 + word_len
        };
        if needed > width {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_len;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_len;
        }
    }
    out.push(current);
}

/// A single line's change between two rendered frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineChange {
    /// A line present in `previous` was replaced with new content at this index.
    Changed { index: usize, text: String },
    /// A line that did not exist in `previous` was appended at this index.
    Added { index: usize, text: String },
    /// A trailing line present in `previous` was removed at this index.
    Removed { index: usize },
}

/// Positional line diff of a re-render against the previous frame. This models
/// the incremental announcement the screen-reader path emits: only lines that
/// actually changed are spoken, avoiding re-reading the whole screen every tick.
///
/// The diff is index-aligned (not a full LCS) — matching the binary's
/// cheap positional comparison of the parked line buffer.
pub fn diff_lines(previous: &[String], current: &[String]) -> Vec<LineChange> {
    let mut changes = Vec::new();
    let max = previous.len().max(current.len());
    for index in 0..max {
        match (previous.get(index), current.get(index)) {
            (Some(old), Some(new)) if old != new => changes.push(LineChange::Changed {
                index,
                text: new.clone(),
            }),
            (Some(_), Some(_)) => {}
            (None, Some(new)) => changes.push(LineChange::Added {
                index,
                text: new.clone(),
            }),
            (Some(_), None) => changes.push(LineChange::Removed { index }),
            (None, None) => unreachable!("index < max implies at least one side present"),
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::EditDelta;

    // ---- (0) raw text leaf ----------------------------------------------

    #[test]
    fn text_leaf_returns_value_verbatim() {
        assert_eq!(AxNode::text("hello world").to_flat_text(), "hello world");
    }

    #[test]
    fn input_announcement_speaks_only_the_current_edit_delta() {
        assert_eq!(
            input_announcement(&EditDelta::inserted("x")),
            Some("x".to_string())
        );
        assert_eq!(
            input_announcement(&EditDelta::inserted(" ")),
            Some("space".to_string())
        );
        assert_eq!(
            input_announcement(&EditDelta::inserted("\n")),
            Some("new line".to_string())
        );
        assert_eq!(
            input_announcement(&EditDelta::deleted("two words")),
            Some("Deleted two words".to_string())
        );
        assert_eq!(input_announcement(&EditDelta::default()), None);
    }

    // ---- (1) hidden → skip ----------------------------------------------

    #[test]
    fn accessibility_hidden_serializes_empty() {
        let node =
            AxNode::text_run(vec![AxNode::text("secret")]).with_accessibility(Accessibility {
                hidden: true,
                ..Default::default()
            });
        assert_eq!(node.to_flat_text(), "");
    }

    #[test]
    fn display_none_serializes_empty() {
        let node = AxNode::text_run(vec![AxNode::text("gone")]).hidden();
        assert_eq!(node.to_flat_text(), "");
    }

    #[test]
    fn hidden_child_is_dropped_from_box() {
        let node = AxNode::boxed(
            Orientation::Row,
            vec![
                AxNode::text_run(vec![AxNode::text("A")]),
                AxNode::text_run(vec![AxNode::text("B")]).with_accessibility(Accessibility {
                    hidden: true,
                    ..Default::default()
                }),
                AxNode::text_run(vec![AxNode::text("C")]),
            ],
        );
        // The empty (hidden) middle child is filtered before the join.
        assert_eq!(node.to_flat_text(), "A C");
    }

    // ---- (2) label substitution -----------------------------------------

    #[test]
    fn label_replaces_children() {
        let node = AxNode::text_run(vec![AxNode::text("raw glyph ⣿")])
            .with_accessibility(Accessibility::label("progress bar 40%"));
        assert_eq!(node.to_flat_text(), "progress bar 40%");
    }

    // ---- (3) role prefix -------------------------------------------------

    #[test]
    fn role_prefixes_content_when_differs_from_parent() {
        let node = AxNode::text_run(vec![AxNode::text("value")])
            .with_accessibility(Accessibility::role("Header"));
        assert_eq!(node.to_flat_text(), "Header: value");
    }

    #[test]
    fn role_not_prefixed_when_same_as_parent_role() {
        // Outer box carries role "row"; inner child carries the SAME role → the
        // inner role is not re-emitted (n.role === t).
        let inner = AxNode::text_run(vec![AxNode::text("cell")])
            .with_accessibility(Accessibility::role("row"));
        let node = AxNode::boxed(Orientation::Row, vec![inner])
            .with_accessibility(Accessibility::role("row"));
        // Outer emits "row: " once; inner sees parent role "row" == its own → no repeat.
        assert_eq!(node.to_flat_text(), "row: cell");
    }

    // ---- (4) state prefix + ordering ------------------------------------

    #[test]
    fn state_prefix_lists_active_keys_in_order() {
        let node = AxNode::text_run(vec![AxNode::text("Item")]).with_accessibility(Accessibility {
            state: vec![
                ("selected".into(), true),
                ("disabled".into(), false),
                ("focused".into(), true),
            ],
            ..Default::default()
        });
        // Only truthy keys, insertion order, joined by ", ".
        assert_eq!(node.to_flat_text(), "(selected, focused) Item");
    }

    #[test]
    fn role_wraps_outside_state() {
        // Final shape is `role: (states) text` — role outermost, state inner.
        let node = AxNode::text_run(vec![AxNode::text("Item")]).with_accessibility(Accessibility {
            role: Some("Option".into()),
            state: vec![("selected".into(), true)],
            ..Default::default()
        });
        assert_eq!(node.to_flat_text(), "Option: (selected) Item");
    }

    #[test]
    fn no_state_prefix_when_all_inactive() {
        let node = AxNode::text_run(vec![AxNode::text("Item")]).with_accessibility(Accessibility {
            state: vec![("selected".into(), false)],
            ..Default::default()
        });
        assert_eq!(node.to_flat_text(), "Item");
    }

    // ---- (5) box flatten: orientation + reverse -------------------------

    #[test]
    fn row_box_joins_with_space() {
        let node = AxNode::boxed(
            Orientation::Row,
            vec![
                AxNode::text_run(vec![AxNode::text("one")]),
                AxNode::text_run(vec![AxNode::text("two")]),
            ],
        );
        assert_eq!(node.to_flat_text(), "one two");
    }

    #[test]
    fn column_box_joins_with_newline() {
        let node = AxNode::boxed(
            Orientation::Column,
            vec![
                AxNode::text_run(vec![AxNode::text("line1")]),
                AxNode::text_run(vec![AxNode::text("line2")]),
            ],
        );
        assert_eq!(node.to_flat_text(), "line1\nline2");
    }

    #[test]
    fn row_reverse_reverses_children() {
        let node = AxNode::boxed(
            Orientation::RowReverse,
            vec![
                AxNode::text_run(vec![AxNode::text("first")]),
                AxNode::text_run(vec![AxNode::text("second")]),
            ],
        );
        assert_eq!(node.to_flat_text(), "second first");
    }

    #[test]
    fn column_reverse_reverses_and_newline_joins() {
        let node = AxNode::boxed(
            Orientation::ColumnReverse,
            vec![
                AxNode::text_run(vec![AxNode::text("top")]),
                AxNode::text_run(vec![AxNode::text("bottom")]),
            ],
        );
        assert_eq!(node.to_flat_text(), "bottom\ntop");
    }

    // ---- nested table linearization -------------------------------------

    #[test]
    fn nested_table_linearizes_rows_and_cells() {
        // A column-of-rows table:
        //   [ Name | Age ]
        //   [ Ada  | 36  ]
        // Rows join with "\n"; cells within a row join with " ".
        let header = AxNode::boxed(
            Orientation::Row,
            vec![
                AxNode::text_run(vec![AxNode::text("Name")]),
                AxNode::text_run(vec![AxNode::text("Age")]),
            ],
        );
        let row = AxNode::boxed(
            Orientation::Row,
            vec![
                AxNode::text_run(vec![AxNode::text("Ada")]),
                AxNode::text_run(vec![AxNode::text("36")]),
            ],
        );
        let table = AxNode::boxed(Orientation::Column, vec![header, row]);
        assert_eq!(table.to_flat_text(), "Name Age\nAda 36");
    }

    #[test]
    fn nested_table_with_role_and_state_on_selected_row() {
        // A selected row inside a table, with a table role on the container.
        let header = AxNode::boxed(
            Orientation::Row,
            vec![AxNode::text_run(vec![AxNode::text("Task")])],
        );
        let selected_row = AxNode::boxed(
            Orientation::Row,
            vec![AxNode::text_run(vec![AxNode::text("Deploy")])],
        )
        .with_accessibility(Accessibility {
            role: Some("row".into()),
            state: vec![("selected".into(), true)],
            ..Default::default()
        });
        let table = AxNode::boxed(Orientation::Column, vec![header, selected_row])
            .with_accessibility(Accessibility::role("table"));
        // Outer: "table: " prefix. Header cell inherits parent role "table" (its
        // own boxes have no role) → plain "Task". Selected row: role "row" != "table"
        // → "row: ", plus "(selected) ".
        assert_eq!(table.to_flat_text(), "table: Task\nrow: (selected) Deploy");
    }

    // ---- role inheritance shadowing -------------------------------------

    #[test]
    fn child_role_shadows_parent_for_grandchildren() {
        // Parent role "list"; a middle box with role "item"; leaf has role "item"
        // too → leaf's role == inherited "item" → not re-emitted.
        let leaf = AxNode::text_run(vec![AxNode::text("x")])
            .with_accessibility(Accessibility::role("item"));
        let middle = AxNode::boxed(Orientation::Row, vec![leaf])
            .with_accessibility(Accessibility::role("item"));
        let root = AxNode::boxed(Orientation::Column, vec![middle])
            .with_accessibility(Accessibility::role("list"));
        // root: "list: ". middle: role "item" != parent "list" → "item: ". leaf:
        // role "item" == inherited "item" → plain "x".
        assert_eq!(root.to_flat_text(), "list: item: x");
    }

    // ---- transcript symbol labels (`Oao`) -------------------------------

    #[test]
    fn symbol_aria_labels_match_oracle() {
        assert_eq!(SymbolKind::Success.aria_label(), "done:");
        assert_eq!(SymbolKind::Error.aria_label(), "failed:");
        assert_eq!(SymbolKind::Warning.aria_label(), "warning:");
        assert_eq!(SymbolKind::Info.aria_label(), "note:");
        assert_eq!(SymbolKind::Pending.aria_label(), "pending:");
        assert_eq!(SymbolKind::Loading.aria_label(), "loading:");
        // Display forwards to aria_label.
        assert_eq!(format!("{} built", SymbolKind::Success), "done: built");
    }

    // ---- word wrap -------------------------------------------------------

    #[test]
    fn word_wrap_greedy_on_spaces() {
        assert_eq!(
            word_wrap("the quick brown fox", 9),
            vec!["the quick".to_string(), "brown fox".to_string()]
        );
    }

    #[test]
    fn word_wrap_preserves_existing_newlines() {
        assert_eq!(
            word_wrap("a b\nc d", 3),
            vec!["a b".to_string(), "c d".to_string()]
        );
    }

    #[test]
    fn word_wrap_hard_splits_overlong_word() {
        assert_eq!(
            word_wrap("abcdefgh", 3),
            vec!["abc".to_string(), "def".to_string(), "gh".to_string()]
        );
    }

    #[test]
    fn word_wrap_zero_width_is_noop_split() {
        assert_eq!(
            word_wrap("a b\nc", 0),
            vec!["a b".to_string(), "c".to_string()]
        );
    }

    // ---- line diff -------------------------------------------------------

    #[test]
    fn diff_lines_reports_changed_added_removed() {
        let prev = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let cur = vec!["alpha".to_string(), "BETA".to_string()];
        let changes = diff_lines(&prev, &cur);
        assert_eq!(
            changes,
            vec![
                LineChange::Changed {
                    index: 1,
                    text: "BETA".to_string()
                },
                LineChange::Removed { index: 2 },
            ]
        );
    }

    #[test]
    fn diff_lines_reports_appended_line() {
        let prev = vec!["one".to_string()];
        let cur = vec!["one".to_string(), "two".to_string()];
        assert_eq!(
            diff_lines(&prev, &cur),
            vec![LineChange::Added {
                index: 1,
                text: "two".to_string()
            }]
        );
    }

    #[test]
    fn diff_lines_identical_frames_empty() {
        let frame = vec!["x".to_string(), "y".to_string()];
        assert!(diff_lines(&frame, &frame).is_empty());
    }
}
