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

/// A registry-backed slash command (user command, skill, plugin, or bundled
/// skill) as a popup candidate — the NON-builtin half of claude-code's
/// `generateCommandSuggestions` candidate set that lives in the
/// `CommandRegistry` rather than the static [`crate::command::BUILTIN`] table.
///
/// Snapshotted once when the registry is wired (and refreshed on
/// `/reload-skills`) so the per-keystroke popup never has to touch the async
/// registry lock. Names/aliases carry the leading `/`, matching [`SlashCommand`].
///
/// [`SlashCommand`]: crate::command::SlashCommand
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySlashRow {
    /// Command name WITH the leading `/` (e.g. `/loop`).
    pub name: String,
    /// Full description (the model-facing text; used for match candidacy).
    pub description: String,
    /// Compact `/`-menu label (reference `menuDescription`). When present the
    /// popup shows this instead of `description` (`menuDescription ??
    /// description`); `None` falls back to `description`. Matching still uses
    /// `description`.
    pub menu_description: Option<String>,
    /// Alternate names WITH the leading `/`.
    pub aliases: Vec<String>,
}

/// A popup candidate: either a static builtin or a registry-backed command.
/// Lets the ranking in [`command_items_merged`] operate over the two surfaces
/// as one unified list, exactly as the reference's single Fuse index does.
enum Cand<'a> {
    Builtin(&'a crate::command::SlashCommand),
    Registry(&'a RegistrySlashRow),
}

impl Cand<'_> {
    /// Command name WITH the leading `/`.
    fn name(&self) -> &str {
        match self {
            Cand::Builtin(c) => c.name,
            Cand::Registry(r) => &r.name,
        }
    }

    /// Full description — used for match candidacy (the searchable text).
    fn describe(&self) -> String {
        match self {
            Cand::Builtin(c) => c.describe(),
            Cand::Registry(r) => r.description.clone(),
        }
    }

    /// The text shown in the popup's dim right column: the reference's
    /// `menuDescription ?? description`. Builtins have no menu label, so this is
    /// their [`Self::describe`]; a registry row prefers its `menu_description`.
    fn display_desc(&self) -> String {
        match self {
            Cand::Builtin(c) => c.describe(),
            Cand::Registry(r) => r
                .menu_description
                .clone()
                .unwrap_or_else(|| r.description.clone()),
        }
    }

    /// Alternate names WITH the leading `/`.
    fn aliases(&self) -> Vec<&str> {
        match self {
            Cand::Builtin(c) => c.aliases.iter().copied().collect(),
            Cand::Registry(r) => r.aliases.iter().map(String::as_str).collect(),
        }
    }
}

/// The builtin commands matching `prefix` (a `/`-led token), as completion
/// items — derived from the single [`crate::command::BUILTIN`] registry.
/// Empty when `prefix` is not a command fragment. Equivalent to
/// [`command_items_merged`] with an empty registry snapshot.
#[must_use]
pub fn command_items(prefix: &str) -> Vec<CompletionItem> {
    command_items_merged(prefix, &[])
}

/// The commands matching `prefix`, merging the static builtins with the
/// registry-backed `registry` snapshot (user commands, skills, plugin/bundled
/// commands) into ONE ranked surface — the full claude-code 2.1.205
/// `generateCommandSuggestions` candidate set. Passing `&[]` reproduces
/// [`command_items`] byte-for-byte.
///
/// A registry row is dropped when a builtin already owns its name — the builtin
/// surface is authoritative and never duplicated.
///
/// Ordering and matching are a port of `generateCommandSuggestions`:
/// - bare `/` lists every command **alphabetically**;
/// - a query ranks candidates exact-name > exact-alias > prefix-name (shorter
///   first) > prefix-alias (shorter first) > fuzzy, with the tie-break falling
///   back to alphabetical (the reference tie-breaks on Fuse score + usage;
///   deterministic alphabetical stands in for that fuzzy-score tail);
/// - fuzzy candidacy is name/alias substring or description word-prefix (a
///   deterministic stand-in for the reference's Fuse.js index over the same
///   keys);
/// - a hidden builtin surfaces when its exact name is typed (the reference's
///   `hiddenExact` rule);
/// - the matched alias is shown in parens only when the user typed it
///   (`findMatchedAlias`), e.g. `/quit` → `/exit (quit)`.
#[must_use]
pub fn command_items_merged(prefix: &str, registry: &[RegistrySlashRow]) -> Vec<CompletionItem> {
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

    // Registry rows whose name a builtin already owns are dropped (the builtin
    // table is authoritative). Inlined at both use sites below — a shared
    // closure confuses lifetime inference on the borrowed `registry` slice.

    // Bare "/": every command, alphabetically. The registry iterator leads the
    // `chain` so the item type binds to the borrowed `registry` lifetime and the
    // `'static` builtins coerce down (covariance); order is irrelevant — the
    // result is sorted by name below.
    if query.is_empty() {
        let mut commands: Vec<Cand> = registry
            .iter()
            .filter(|r| !crate::command::BUILTIN.iter().any(|b| b.name == r.name))
            .map(|r| Cand::Registry(r))
            .chain(crate::command::advertised().map(|c| Cand::Builtin(c)))
            .collect();
        commands.sort_by(|a, b| a.name().cmp(b.name()));
        return commands
            .into_iter()
            .map(|c| item(c.name(), c.display_desc(), None))
            .collect();
    }

    // Strip the leading slash from a registry name/alias for matching.
    let bare = |name: &str| name[1..].to_lowercase();

    // `hiddenExact`: an unadvertised builtin typed out in full surfaces — unless
    // a visible command shares the name. Only builtins carry a hidden flag;
    // registry rows are always visible.
    let hidden_exact = crate::command::BUILTIN
        .iter()
        .filter(|c| !c.advertised || crate::command::is_runtime_hidden(c.name))
        .find(|c| bare(c.name) == query);

    // Candidates: commands the reference's Fuse index would match — name/alias
    // substring, name-part prefix, or description word prefix.
    let mut candidates: Vec<Cand> = registry
        .iter()
        .filter(|r| !crate::command::BUILTIN.iter().any(|b| b.name == r.name))
        .map(|r| Cand::Registry(r))
        .chain(crate::command::advertised().map(|c| Cand::Builtin(c)))
        .filter(|c| {
            bare(c.name()).contains(&query)
                || c.aliases().iter().any(|a| bare(a).contains(&query))
                || bare(c.name())
                    .split(['-', '_', ':'])
                    .any(|part| part.starts_with(&query))
                || c.describe()
                    .to_lowercase()
                    .split_whitespace()
                    .any(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()).starts_with(&query))
        })
        .collect();

    // Rank tiers (the reference comparator, minus the Fuse-score tail).
    let tier = |c: &Cand| -> (u8, usize) {
        let name = bare(c.name());
        if name == query {
            return (0, 0);
        }
        if c.aliases().iter().any(|a| bare(a) == query) {
            return (1, 0);
        }
        if name.starts_with(&query) {
            return (2, name.len());
        }
        if let Some(alias) = c
            .aliases()
            .iter()
            .filter(|a| bare(a).starts_with(&query))
            .min_by_key(|a| a.len())
        {
            return (3, alias.len());
        }
        (4, 0)
    };
    candidates.sort_by(|a, b| tier(a).cmp(&tier(b)).then(a.name().cmp(b.name())));

    let mut items: Vec<CompletionItem> = candidates
        .into_iter()
        .map(|c| {
            // Show the alias in parens only when the user typed it.
            let matched_alias = c
                .aliases()
                .into_iter()
                .find(|a| bare(a).starts_with(&query))
                .map(|a| a[1..].to_string());
            item(c.name(), c.display_desc(), matched_alias.as_deref())
        })
        .collect();
    if let Some(hidden) = hidden_exact {
        if !items.iter().any(|i| i.insert == hidden.name) {
            items.insert(0, item(hidden.name, hidden.describe(), None));
        }
    }
    items
}

/// Build one popup row (claude-code `createCommandSuggestionItem`). `name`
/// carries the leading `/`; `matched_alias` (without slash) is appended in
/// parens when the user typed the alias.
fn item(name: &str, desc: String, matched_alias: Option<&str>) -> CompletionItem {
    let label = match matched_alias {
        Some(alias) => format!("{name} ({alias})"),
        None => name.to_string(),
    };
    CompletionItem {
        label,
        insert: name.to_string(),
        desc,
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

    fn rows(names: &[(&str, &str)]) -> Vec<RegistrySlashRow> {
        names
            .iter()
            .map(|(n, d)| RegistrySlashRow {
                name: (*n).to_string(),
                description: (*d).to_string(),
                menu_description: None,
                aliases: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn empty_registry_is_identical_to_builtin_only() {
        // The merged surface with an empty snapshot must be byte-for-byte the
        // builtin-only popup — the R1 ordering guarantee is untouched.
        for q in ["/", "/m", "/re", "/help", "/zzz", "model"] {
            assert_eq!(command_items_merged(q, &[]), command_items(q), "query {q:?}");
        }
    }

    #[test]
    fn registry_commands_merge_into_the_popup() {
        let reg = rows(&[
            ("/loop", "run a task on a loop"),
            ("/deploy", "ship it"),
        ]);
        // Bare "/" now includes the registry rows alongside the builtins,
        // alphabetically, and one more than the builtin-only count.
        let all = command_items_merged("/", &reg);
        assert_eq!(all.len(), crate::command::advertised().count() + reg.len());
        assert!(all.iter().any(|i| i.insert == "/loop"));
        assert!(all.iter().any(|i| i.insert == "/deploy"));
        // Sorted by name: /deploy precedes /loop.
        let d = all.iter().position(|i| i.insert == "/deploy").unwrap();
        let l = all.iter().position(|i| i.insert == "/loop").unwrap();
        assert!(d < l);
        // A query surfaces the registry command with its description.
        let lo = command_items_merged("/lo", &reg);
        let loop_row = lo.iter().find(|i| i.insert == "/loop").expect("/loop matches /lo");
        assert_eq!(loop_row.desc, "run a task on a loop");
    }

    #[test]
    fn menu_description_is_shown_but_matching_uses_description() {
        // A registry row with a compact menu label: the popup DISPLAYS the menu
        // label (reference `menuDescription ?? description`) but still MATCHES on
        // the full description.
        let reg = vec![RegistrySlashRow {
            name: "/simplify".into(),
            description: "Review the changed code for reuse and altitude cleanups".into(),
            menu_description: Some("Clean up the changed code".into()),
            aliases: Vec::new(),
        }];
        // Bare "/": the row shows the compact menu label, not the full description.
        let all = command_items_merged("/", &reg);
        let row = all.iter().find(|i| i.insert == "/simplify").unwrap();
        assert_eq!(row.desc, "Clean up the changed code");
        // Matching still works off the full description ("altitude" is only there).
        assert!(command_items_merged("/altitude", &reg)
            .iter()
            .any(|i| i.insert == "/simplify"));
        // A row without a menu label falls back to its description.
        let reg2 = rows(&[("/verify", "Verify a code change end-to-end")]);
        let v = command_items_merged("/", &reg2);
        assert_eq!(
            v.iter().find(|i| i.insert == "/verify").unwrap().desc,
            "Verify a code change end-to-end"
        );
    }

    #[test]
    fn registry_row_shadowing_a_builtin_is_dropped() {
        // A registry entry that reuses a builtin name never duplicates the
        // builtin row — the builtin surface is authoritative.
        let reg = rows(&[("/help", "SHOULD NOT WIN")]);
        let all = command_items_merged("/", &reg);
        assert_eq!(all.len(), crate::command::advertised().count());
        let help: Vec<_> = all.iter().filter(|i| i.insert == "/help").collect();
        assert_eq!(help.len(), 1);
        assert_ne!(help[0].desc, "SHOULD NOT WIN");
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
