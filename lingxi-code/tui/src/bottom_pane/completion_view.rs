//! The composer completion popup: a small list of candidates shown just above
//! the composer while a `/command` or `@file` token is being typed (ported
//! from the former `palette::CompletionPopup`, plan Phase 4).
//!
//! Presentational + selection only — the app decides how to APPLY the chosen
//! item (a `/command` replaces the whole buffer; an `@file` replaces just the
//! token). Modeled on codex's `bottom_pane` completion popup, anchored above
//! the composer rather than a full-screen modal. Unlike the modal
//! [`crate::bottom_pane::view::BottomPaneView`]s it is NOT stacked: it
//! coexists with the composer (typing keeps filtering), so its state stays
//! with the composer owner (`RataApp` today, `BottomPane` in plan Phase 5).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Rows shown in the popup before it stops growing (and starts scrolling to
/// follow the highlight).
const MAX_ROWS: usize = 6;

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// The text shown in the popup's left column.
    pub label: String,
    /// The text inserted when the item is chosen.
    pub insert: String,
    /// A dim right-column hint (command description, or `""`).
    pub desc: String,
}

/// The registry commands matching `prefix` (a `/`-led token), as completion
/// items — derived from the single [`crate::command::BUILTIN`] registry (plan
/// Phase 8). Empty when `prefix` is not a command fragment.
///
/// Ordering and matching are a port of claude-code 2.1.205's
/// `generateCommandSuggestions`:
/// - bare `/` lists every advertised command **alphabetically**;
/// - a query ranks candidates exact-name > exact-alias > prefix-name (shorter
///   first) > prefix-alias (shorter first) > fuzzy, with the tie-break falling
///   back to alphabetical (the reference tie-breaks on Fuse score + usage;
///   deterministic alphabetical stands in for that fuzzy-score tail);
/// - fuzzy candidacy is name/alias substring or description word-prefix (a
///   deterministic stand-in for the reference's Fuse.js index over the same
///   keys);
/// - a hidden command surfaces when its exact name is typed (the reference's
///   `hiddenExact` rule);
/// - the matched alias is shown in parens only when the user typed it
///   (`findMatchedAlias`), e.g. `/quit` → `/exit (quit)`.
#[must_use]
pub fn command_items(prefix: &str) -> Vec<CompletionItem> {
    if !prefix.starts_with('/') {
        return Vec::new();
    }
    let rest = &prefix[1..];
    // `hasCommandArgs`: once arguments are being typed there are no command
    // suggestions.
    if rest.contains(char::is_whitespace) {
        return Vec::new();
    }
    let query = rest.to_lowercase();

    // Bare "/": every advertised command, alphabetically.
    if query.is_empty() {
        let mut commands: Vec<_> = crate::command::advertised().collect();
        commands.sort_by_key(|command| command.name);
        return commands.into_iter().map(|c| item(c, None)).collect();
    }

    // Strip the leading slash from a registry name/alias for matching.
    let bare = |name: &str| name[1..].to_lowercase();

    // `hiddenExact`: an unadvertised command typed out in full surfaces —
    // unless a visible command shares the name (impossible in one registry,
    // kept as a guard for parity with the reference).
    let hidden_exact = crate::command::BUILTIN
        .iter()
        .filter(|c| !c.advertised || crate::command::is_runtime_hidden(c.name))
        .find(|c| bare(c.name) == query);

    // Candidates: advertised commands the reference's Fuse index would match —
    // name/alias substring, name-part prefix, or description word prefix.
    let mut candidates: Vec<_> = crate::command::advertised()
        .filter(|c| {
            bare(c.name).contains(&query)
                || c.aliases.iter().any(|a| bare(a).contains(&query))
                || bare(c.name)
                    .split(['-', '_', ':'])
                    .any(|part| part.starts_with(&query))
                || c.describe()
                    .to_lowercase()
                    .split_whitespace()
                    .any(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()).starts_with(&query))
        })
        .collect();

    // Rank tiers (the reference comparator, minus the Fuse-score tail).
    let tier = |c: &crate::command::SlashCommand| -> (u8, usize) {
        let name = bare(c.name);
        if name == query {
            return (0, 0);
        }
        if c.aliases.iter().any(|a| bare(a) == query) {
            return (1, 0);
        }
        if name.starts_with(&query) {
            return (2, name.len());
        }
        if let Some(alias) = c
            .aliases
            .iter()
            .filter(|a| bare(a).starts_with(&query))
            .min_by_key(|a| a.len())
        {
            return (3, alias.len());
        }
        (4, 0)
    };
    candidates.sort_by(|a, b| tier(a).cmp(&tier(b)).then(a.name.cmp(b.name)));

    let mut items: Vec<CompletionItem> = candidates
        .into_iter()
        .map(|c| {
            // Show the alias in parens only when the user typed it.
            let matched_alias = c
                .aliases
                .iter()
                .find(|a| bare(a).starts_with(&query))
                .map(|a| &a[1..]);
            item(c, matched_alias)
        })
        .collect();
    if let Some(hidden) = hidden_exact {
        if !items.iter().any(|i| i.insert == hidden.name) {
            items.insert(0, item(hidden, None));
        }
    }
    items
}

/// Build one popup row (claude-code `createCommandSuggestionItem`).
fn item(command: &crate::command::SlashCommand, matched_alias: Option<&str>) -> CompletionItem {
    let label = match matched_alias {
        Some(alias) => format!("{} ({alias})", command.name),
        None => command.name.to_string(),
    };
    CompletionItem {
        label,
        insert: command.name.to_string(),
        desc: command.describe(),
    }
}

/// A completion popup over a candidate list.
pub struct CompletionView {
    items: Vec<CompletionItem>,
    selected: usize,
    /// Top item index of the visible [`MAX_ROWS`] window (follows the
    /// highlight so it can never scroll out of view).
    offset: usize,
}

impl CompletionView {
    /// Build a popup over `items` (highlight at the top). Returns `None` when
    /// there is nothing to show.
    #[must_use]
    pub fn new(items: Vec<CompletionItem>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self {
                items,
                selected: 0,
                offset: 0,
            })
        }
    }

    /// The text the highlighted item inserts.
    #[must_use]
    pub fn selected_insert(&self) -> &str {
        &self.items[self.selected].insert
    }

    /// Rows the popup wants on screen: the visible item window (at most
    /// [`MAX_ROWS`]) plus the 2 border rows. [`BottomPane`] reserves exactly
    /// this many rows above the composer so the popup is never squeezed
    /// against the pane top (plan Phase 13 layout fix).
    ///
    /// [`BottomPane`]: crate::bottom_pane::BottomPane
    #[must_use]
    pub fn desired_height(&self) -> u16 {
        u16::try_from(self.items.len().min(MAX_ROWS) + 2).unwrap_or(u16::MAX)
    }

    /// Highlighted row index (exposed for tests).
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the highlight up (clamped).
    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.follow();
    }

    /// Move the highlight down (clamped).
    pub fn next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
        self.follow();
    }

    /// Keep the highlighted row inside the visible window.
    fn follow(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + MAX_ROWS {
            self.offset = self.selected + 1 - MAX_ROWS;
        }
    }

    /// Draw the popup anchored just above `composer` (bordered list, cleared
    /// beneath), rendering into `buf` (`(Rect, &mut Buffer)` contract). Grows
    /// upward from the composer's top edge.
    pub fn render(&self, composer: Rect, buf: &mut Buffer) {
        let height = self.desired_height();
        let y = composer.y.saturating_sub(height);
        let rect = Rect {
            x: composer.x,
            y,
            width: composer.width,
            height: height.min(composer.y),
        };
        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Complete");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(MAX_ROWS)
            .map(|(i, item)| {
                let caret = if i == self.selected { "› " } else { "  " };
                let style = if i == self.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                let mut spans = vec![Span::styled(format!("{caret}{}", item.label), style)];
                if !item.desc.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", item.desc),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                Line::from(spans)
            })
            .collect();
        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_items_filter_by_prefix() {
        // A bare "/" matches every ADVERTISED registry command, in order.
        let all = command_items("/");
        assert_eq!(all.len(), crate::command::advertised().count());
        // "/m" matches /model + /mcp.
        let m = command_items("/m");
        assert!(m.iter().any(|i| i.insert == "/model"));
        assert!(m.iter().any(|i| i.insert == "/mcp"));
        assert!(!m.iter().any(|i| i.insert == "/help"));
        // Non-slash input yields nothing.
        assert!(command_items("model").is_empty());
        assert!(command_items("/zzz").is_empty());
        // Unadvertised registry entries never surface.
        assert!(!all.iter().any(|i| i.insert == "/image"));
    }

    #[test]
    fn new_is_none_when_empty() {
        assert!(CompletionView::new(Vec::new()).is_none());
        assert!(CompletionView::new(command_items("/h")).is_some());
    }

    #[test]
    fn completion_navigation_clamps_at_edges_by_design() {
        // Plan Phase 12 decision: clamp-at-edges is the deliberate LingXi
        // navigation behavior (no wrap-around) across dialog/picker/completion.
        // The bare-"/" popup lists advertised commands alphabetically
        // (claude-code order).
        let mut names: Vec<_> = crate::command::advertised().map(|c| c.name).collect();
        names.sort_unstable();
        let total = names.len();
        let mut p = CompletionView::new(command_items("/")).unwrap();
        assert_eq!(p.selected(), 0);
        p.prev(); // clamps at 0 — does NOT wrap to the last item
        assert_eq!(p.selected(), 0);
        p.next();
        assert_eq!(p.selected(), 1);
        assert_eq!(p.selected_insert(), names[1]);
        // Walk past the end: the highlight clamps on the last item.
        for _ in 0..total {
            p.next();
        }
        assert_eq!(p.selected(), total - 1);
        assert_eq!(p.selected_insert(), names[total - 1]);
    }

    #[test]
    fn window_follows_the_highlight_past_max_rows() {
        // 10 items, 6 visible: walking to the end scrolls the window so the
        // highlighted row is always rendered.
        let items: Vec<CompletionItem> = (0..10)
            .map(|i| CompletionItem {
                label: format!("/cmd{i}"),
                insert: format!("/cmd{i}"),
                desc: String::new(),
            })
            .collect();
        let mut p = CompletionView::new(items).unwrap();
        for _ in 0..9 {
            p.next();
        }
        assert_eq!(p.selected(), 9);
        let screen = Rect::new(0, 0, 30, 12);
        let composer = Rect::new(0, 10, 30, 2);
        let mut buf = Buffer::empty(screen);
        p.render(composer, &mut buf);
        let text: String = (screen.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("› /cmd9"), "highlight visible: {text}");
        assert!(!text.contains("/cmd0"), "top rows scrolled out: {text}");
        // Walking back up scrolls the window back to the top.
        for _ in 0..9 {
            p.prev();
        }
        let mut buf = Buffer::empty(screen);
        p.render(composer, &mut buf);
        let text: String = (screen.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("› /cmd0"), "{text}");
    }

    #[test]
    fn render_anchors_above_composer_rect_in_buffer() {
        let p = CompletionView::new(command_items("/m")).unwrap();
        let screen = Rect::new(0, 0, 40, 12);
        // Composer occupies the bottom rows; the popup grows upward from its top.
        let composer = Rect::new(0, 8, 40, 4);
        let mut buf = Buffer::empty(screen);
        p.render(composer, &mut buf);
        let text: String = (screen.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Complete"), "{text}");
        // Prefix matches rank shorter-name first: /mcp is highlighted.
        assert!(text.contains("› /mcp"), "highlighted match: {text}");
        assert!(text.contains("/model"), "{text}");
        // Everything the popup drew sits strictly above the composer rows.
        let composer_rows: String = (composer.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect();
        assert!(
            composer_rows.trim().is_empty(),
            "popup stays above composer"
        );
    }
}
