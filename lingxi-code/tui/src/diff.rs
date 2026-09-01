//! `/diff`: a live, scrollable view of uncommitted working-tree changes.
//!
//! This follows the current-changes half of claude-code's `DiffDialog`
//! (`src/commands/diff/` + `src/components/diff/DiffDialog.tsx`), whose source
//! is `src/utils/gitDiff.ts::fetchGitDiffHunks` and runs
//! `git --no-optional-locks diff HEAD`. The view intentionally keeps the raw
//! patch as its detail body; per-turn diff history remains outside LingXi's
//! provider-neutral transcript model.
//!
//! Read-only: no index, ref, or working-tree state is mutated. The initial
//! snapshot is collected when the view opens; later snapshots are collected by
//! the regular view tick at a small debounce interval, never from sizing or
//! rendering. The view requests a redraw only after a refreshed snapshot
//! changes, so an external branch switch, commit, or edit becomes visible
//! without spawning a git child process for every frame.

use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;

const DIFF_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Formatted `/diff` result plus whether it should render in the error color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOutput {
    /// The current diff body shown by the view.
    pub body: String,
    /// Render in the error color (a git failure) rather than as neutral output.
    pub is_error: bool,
}

/// Run `git --no-optional-locks diff HEAD` in `cwd` and format the result.
///
/// Mirrors claude-code `fetchGitDiffHunks` (`--no-optional-locks` avoids taking
/// the index lock for a pure read). An empty diff yields a friendly "no
/// changes" note; a non-zero exit (not a git repo, no commits yet, …) or a
/// spawn failure yields the captured stderr in the error color.
pub fn collect_diff(cwd: &Path) -> DiffOutput {
    match Command::new("git")
        .args(["--no-optional-locks", "diff", "HEAD"])
        .current_dir(cwd)
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                DiffOutput {
                    body: "No uncommitted changes.".to_string(),
                    is_error: false,
                }
            } else {
                DiffOutput {
                    body: trimmed.to_string(),
                    is_error: false,
                }
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = stderr.trim();
            let body = if detail.is_empty() {
                "Failed to compute diff (is this a git repository?)".to_string()
            } else {
                format!("Failed to compute diff: {detail}")
            };
            DiffOutput {
                body,
                is_error: true,
            }
        }
        Err(err) => DiffOutput {
            body: format!("Failed to run git: {err}"),
            is_error: true,
        },
    }
}

struct DiffViewState {
    output: DiffOutput,
    scroll: u16,
    next_refresh_at: Instant,
    redraw_requested: bool,
}

type DiffRunner = Box<dyn Fn(&Path) -> DiffOutput>;

/// A full-frame, scrollable `/diff` overlay with a cached git snapshot.
/// Keeping the source path and refresh state here lets the view follow
/// external branch/commit changes while the TUI remains mounted without doing
/// synchronous process work in the render or sizing hot paths.
pub struct DiffView {
    cwd: PathBuf,
    runner: DiffRunner,
    state: RefCell<DiffViewState>,
}

impl DiffView {
    /// Open the diff view rooted at `cwd`, taking an initial read immediately
    /// so command dispatch can report a useful body before the first draw.
    #[must_use]
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_runner(cwd, collect_diff)
    }

    /// Construct a view with an injected snapshot runner. The production
    /// constructor uses [`collect_diff`]; injection keeps refresh cadence
    /// tests independent of a real git process and makes process-count
    /// regressions observable.
    #[must_use]
    pub fn with_runner<F>(cwd: PathBuf, runner: F) -> Self
    where
        F: Fn(&Path) -> DiffOutput + 'static,
    {
        let output = runner(&cwd);
        let now = Instant::now();
        Self {
            state: RefCell::new(DiffViewState {
                output,
                scroll: 0,
                next_refresh_at: now + DIFF_REFRESH_INTERVAL,
                redraw_requested: false,
            }),
            cwd,
            runner: Box::new(runner),
        }
    }

    fn refresh_at(&self, now: Instant, force: bool) -> bool {
        {
            let mut state = self.state.borrow_mut();
            if !force && now < state.next_refresh_at {
                return false;
            }
            // Advance the deadline before running the injected callback. This
            // coalesces repeated tick calls even if the callback itself is
            // slow, and avoids a tight retry loop after a git failure.
            state.next_refresh_at = now + DIFF_REFRESH_INTERVAL;
        }

        let output = (self.runner)(&self.cwd);
        let mut state = self.state.borrow_mut();
        if state.output == output {
            return false;
        }
        state.output = output;
        let max = self.max_scroll(&state.output.body);
        state.scroll = state.scroll.min(max);
        state.redraw_requested = true;
        true
    }

    /// Force-refresh the current git diff. Regular idle updates go through
    /// [`Self::handle_tick`] so rendering and sizing never perform a process
    /// spawn; this method remains available to explicit callers/tests that
    /// need a synchronous refresh.
    pub fn refresh(&self) -> bool {
        self.refresh_at(Instant::now(), true)
    }

    /// Current cached body text. Refreshes are driven by the view tick (or an
    /// explicit [`Self::refresh`]), so inspecting the body never runs git.
    #[must_use]
    pub fn body_text(&self) -> String {
        self.state.borrow().output.body.clone()
    }

    /// Current vertical scroll offset (top line index).
    #[must_use]
    pub fn scroll(&self) -> u16 {
        self.state.borrow().scroll
    }

    fn max_scroll(&self, body: &str) -> u16 {
        u16::try_from(body.lines().count().saturating_sub(1)).unwrap_or(u16::MAX)
    }

    fn body_lines(body: &str) -> Vec<Line<'static>> {
        body.lines()
            .map(|line| Line::from(line.to_string()))
            .collect()
    }
}

impl Renderable for DiffView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let state = self.state.borrow();
        let lines = Self::body_lines(&state.output.body);
        let footer = if state.output.is_error {
            "esc to close · ↑/↓ scroll · auto-refreshes · git error"
        } else {
            "esc to close · ↑/↓ scroll · auto-refreshes"
        };
        Clear.render(area, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .title(Span::styled(
                "Diff",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::from(Span::styled(
                footer,
                Style::default().add_modifier(Modifier::DIM),
            )));
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(lines)
            .scroll((state.scroll, 0))
            .render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        self.state
            .borrow()
            .output
            .body
            .lines()
            .count()
            .try_into()
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for DiffView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        let mut state = self.state.borrow_mut();
        let max = self.max_scroll(&state.output.body);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewOutcome::Cancelled,
            KeyCode::Up => {
                state.scroll = state.scroll.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                state.scroll = state.scroll.saturating_add(1).min(max);
                ViewOutcome::Pending
            }
            KeyCode::PageUp => {
                state.scroll = state.scroll.saturating_sub(10);
                ViewOutcome::Pending
            }
            KeyCode::PageDown => {
                state.scroll = state.scroll.saturating_add(10).min(max);
                ViewOutcome::Pending
            }
            KeyCode::Home => {
                state.scroll = 0;
                ViewOutcome::Pending
            }
            KeyCode::End => {
                state.scroll = max;
                ViewOutcome::Pending
            }
            _ => ViewOutcome::Pending,
        }
    }

    fn wants_status_line(&self) -> bool {
        false
    }

    fn handle_tick(&mut self, now: Instant) -> ViewOutcome {
        // Clear the one-shot redraw request before checking the debounced
        // source. A changed snapshot sets it again and wakes the app's next
        // render; an unchanged snapshot leaves the idle loop with zero git
        // process work until the next deadline.
        self.state.borrow_mut().redraw_requested = false;
        self.refresh_at(now, false);
        ViewOutcome::Pending
    }

    fn needs_redraw(&self) -> bool {
        self.state.borrow().redraw_requested
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Outside a git repository the command exits non-zero, so the helper
    /// reports an error rather than pretending there are no changes.
    #[test]
    fn non_git_dir_is_reported_as_error() {
        let tmp = std::env::temp_dir().join(format!("lingxi-diff-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let out = collect_diff(&tmp);
        assert!(out.is_error, "non-git dir should surface a git error");
        assert!(out.body.starts_with("Failed"), "body: {}", out.body);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A freshly-`git init`ed repo with a committed HEAD and no edits reports
    /// the empty-diff note (not an error).
    #[test]
    fn clean_repo_reports_no_changes() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        git(tmp.path(), &["init", "-q"]);
        git(tmp.path(), &["config", "user.email", "t@t"]);
        git(tmp.path(), &["config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "hello\n").expect("write file");
        git(tmp.path(), &["add", "a.txt"]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);
        let out = collect_diff(tmp.path());
        assert!(!out.is_error, "clean repo should not error: {}", out.body);
        assert_eq!(out.body, "No uncommitted changes.");
    }

    #[test]
    fn diff_view_refreshes_after_external_edit_and_commit() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        git(tmp.path(), &["init", "-q"]);
        git(tmp.path(), &["config", "user.email", "t@t"]);
        git(tmp.path(), &["config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "before\n").expect("write file");
        git(tmp.path(), &["add", "a.txt"]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);

        let view = DiffView::new(tmp.path().to_path_buf());
        assert_eq!(view.body_text(), "No uncommitted changes.");

        std::fs::write(tmp.path().join("a.txt"), "after\n").expect("edit file");
        assert!(view.refresh(), "external edit changes the view source");
        let edited = view.body_text();
        assert!(edited.contains("-before"), "edited diff: {edited}");
        assert!(edited.contains("+after"), "edited diff: {edited}");

        git(tmp.path(), &["add", "a.txt"]);
        git(tmp.path(), &["commit", "-q", "-m", "update"]);
        assert!(view.refresh(), "external commit changes the view source");
        assert_eq!(view.body_text(), "No uncommitted changes.");
    }

    #[test]
    fn diff_view_caches_render_and_debounces_snapshot_runner() {
        use std::cell::Cell;
        use std::rc::Rc;

        let calls = Rc::new(Cell::new(0));
        let calls_for_runner = Rc::clone(&calls);
        let view = DiffView::with_runner(PathBuf::from("."), move |_| {
            let next = calls_for_runner.get() + 1;
            calls_for_runner.set(next);
            DiffOutput {
                body: format!("snapshot {next}"),
                is_error: false,
            }
        });
        assert_eq!(calls.get(), 1, "opening the view takes one snapshot");

        // Sizing and drawing consume the cached snapshot; neither path may
        // launch another runner while the view is idle.
        let _ = view.desired_height(80);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
        view.render(Rect::new(0, 0, 40, 8), &mut buffer);
        assert_eq!(calls.get(), 1, "render paths must not spawn git");

        let due = Instant::now() + DIFF_REFRESH_INTERVAL + Duration::from_millis(1);
        let mut view = view;
        view.handle_tick(due);
        assert_eq!(calls.get(), 2, "one refresh runs when the debounce expires");
        assert!(
            view.needs_redraw(),
            "a changed snapshot requests one redraw"
        );

        // Replaying the same tick, or ticking inside the next debounce window,
        // coalesces the request and leaves the cached body untouched.
        view.handle_tick(due);
        assert_eq!(calls.get(), 2);
        assert!(!view.needs_redraw());
        view.handle_tick(due + Duration::from_millis(1));
        assert_eq!(calls.get(), 2);

        view.handle_tick(due + DIFF_REFRESH_INTERVAL + Duration::from_millis(1));
        assert_eq!(calls.get(), 3, "the next interval permits one refresh");
    }

    #[test]
    fn diff_view_large_snapshot_stays_cached_across_render_and_sizing() {
        use std::cell::Cell;
        use std::rc::Rc;

        let calls = Rc::new(Cell::new(0));
        let calls_for_runner = Rc::clone(&calls);
        let view = DiffView::with_runner(PathBuf::from("."), move |_| {
            calls_for_runner.set(calls_for_runner.get() + 1);
            DiffOutput {
                body: (0..10_000)
                    .map(|line| format!("+generated line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_error: false,
            }
        });
        assert_eq!(calls.get(), 1);

        let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 40));
        for _ in 0..5 {
            assert_eq!(view.desired_height(120), 10_002);
            view.render(Rect::new(0, 0, 120, 40), &mut buffer);
        }
        assert_eq!(
            calls.get(),
            1,
            "large cached snapshots must not spawn a runner from hot paths"
        );
    }

    #[test]
    fn diff_view_scrolls_and_closes_as_a_full_frame_overlay() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        git(tmp.path(), &["init", "-q"]);
        git(tmp.path(), &["config", "user.email", "t@t"]);
        git(tmp.path(), &["config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\n").expect("write file");
        git(tmp.path(), &["add", "a.txt"]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(tmp.path().join("a.txt"), "one\ntwo\nthree\nfour\n").expect("edit file");

        let mut view = DiffView::new(tmp.path().to_path_buf());
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            ViewOutcome::Pending
        ));
        assert!(view.scroll() > 0, "end moves through the patch");
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ViewOutcome::Cancelled
        ));
    }
}
