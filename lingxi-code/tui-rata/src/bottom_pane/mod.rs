//! The bottom pane of the chat UI: transient [`BottomPaneView`]s stacked over
//! the composer (plan Phase 4).
//!
//! Modeled on codex-rs `tui/src/bottom_pane/mod.rs`: the pane owns a stack of
//! keyboard-owning views (permission prompt, model picker, read-only screens)
//! that temporarily take input away from the composer. Input routing is
//! layered — the stack decides which local surface receives a key (top view
//! vs composer), while higher-level intent (interrupt/quit, turn submission,
//! model switching) is decided by the owner acting on the returned
//! [`ViewOutcome`].
//!
//! This phase introduces the view stack itself ([`ViewStack`]); plan Phase 5
//! adds the full `BottomPane` (composer + completion + status ownership)
//! around it.

pub mod completion_view;
pub mod dialog_view;
pub mod model_picker_view;
pub mod permission_view;
pub mod screen_view;
pub mod view;

use crossterm::event::KeyEvent;
pub use view::{BottomPaneView, CommandAction, ViewAction, ViewOutcome};

/// The transient view stack: the TOP view owns the keyboard; views pop
/// themselves off through the [`ViewOutcome`] they return.
#[derive(Default)]
pub struct ViewStack {
    views: Vec<Box<dyn BottomPaneView>>,
}

impl ViewStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push `view`; it becomes the active (keyboard-owning) view.
    pub fn push(&mut self, view: Box<dyn BottomPaneView>) {
        self.views.push(view);
    }

    /// Whether no view is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// How many views are stacked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.views.len()
    }

    /// The active (top) view, if any.
    #[must_use]
    pub fn active(&self) -> Option<&dyn BottomPaneView> {
        self.views.last().map(std::convert::AsRef::as_ref)
    }

    /// Whether any stacked view (not just the top) is a `V` — e.g. "is a
    /// permission prompt open somewhere?".
    #[must_use]
    pub fn contains<V: 'static>(&self) -> bool {
        self.views.iter().any(|view| view.as_any().is::<V>())
    }

    /// All stacked views bottom-to-top, for painting them in order.
    #[must_use]
    pub fn views(&self) -> &[Box<dyn BottomPaneView>] {
        &self.views
    }

    /// Route a key to the active view. Returns `None` when no view is open
    /// (the caller should route the key to the composer instead); otherwise
    /// the stack has already performed the pop/push bookkeeping the outcome
    /// demands and the caller only acts on app-level effects.
    pub fn route_key(&mut self, key: KeyEvent) -> Option<ViewOutcome> {
        let view = self.views.last_mut()?;
        let outcome = view.handle_key(key);
        Some(self.apply(outcome))
    }

    /// Route a bracketed paste to the active view (same contract as
    /// [`Self::route_key`]).
    pub fn route_paste(&mut self, text: &str) -> Option<ViewOutcome> {
        let view = self.views.last_mut()?;
        let outcome = view.handle_paste(text);
        Some(self.apply(outcome))
    }

    /// Perform the stack bookkeeping `outcome` demands — pop the completed
    /// view, cascade parent dismissal on accept, push opened child views —
    /// and return the outcome the owner still has to act on.
    fn apply(&mut self, outcome: ViewOutcome) -> ViewOutcome {
        match outcome {
            ViewOutcome::Pending => ViewOutcome::Pending,
            ViewOutcome::Cancelled => {
                // A cancelled child pops alone: parents stay open (codex
                // parity — cancel returns to the parent flow).
                self.views.pop();
                ViewOutcome::Cancelled
            }
            ViewOutcome::OpenView(child) => {
                // Consumed locally: the requesting view stays open beneath.
                self.views.push(child);
                ViewOutcome::Pending
            }
            accepted => {
                // Accepted / SubmitPrompt / SwitchModel / PermissionResponse /
                // RunCommand all complete the active view acceptingly: pop it,
                // then every parent that asked to dismiss with its child.
                self.views.pop();
                while self
                    .views
                    .last()
                    .is_some_and(|view| view.dismiss_after_child_accept())
                {
                    self.views.pop();
                }
                accepted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::renderable::Renderable;

    /// A scripted view: returns the next queued outcome per key/paste.
    struct StubView {
        outcomes: Vec<ViewOutcome>,
        dismiss_after_child_accept: bool,
        keys_seen: usize,
        pastes_seen: usize,
    }

    impl StubView {
        fn returning(outcomes: Vec<ViewOutcome>) -> Self {
            Self {
                outcomes,
                dismiss_after_child_accept: false,
                keys_seen: 0,
                pastes_seen: 0,
            }
        }

        fn dismissing_parent() -> Self {
            Self {
                outcomes: Vec::new(),
                dismiss_after_child_accept: true,
                keys_seen: 0,
                pastes_seen: 0,
            }
        }

        fn next_outcome(&mut self) -> ViewOutcome {
            if self.outcomes.is_empty() {
                ViewOutcome::Pending
            } else {
                self.outcomes.remove(0)
            }
        }
    }

    impl Renderable for StubView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            1
        }
    }

    impl BottomPaneView for StubView {
        fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
            self.keys_seen += 1;
            self.next_outcome()
        }

        fn handle_paste(&mut self, _text: &str) -> ViewOutcome {
            self.pastes_seen += 1;
            self.next_outcome()
        }

        fn dismiss_after_child_accept(&self) -> bool {
            self.dismiss_after_child_accept
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// A second concrete type so `contains::<V>()` has something to miss.
    struct OtherView;

    impl Renderable for OtherView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            1
        }
    }

    impl BottomPaneView for OtherView {
        fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
            ViewOutcome::Pending
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn route_key_is_none_when_no_view_is_open() {
        let mut stack = ViewStack::new();
        assert!(stack.is_empty());
        assert!(stack.route_key(key(KeyCode::Enter)).is_none());
        assert!(stack.route_paste("x").is_none());
    }

    #[test]
    fn push_makes_the_view_active_and_pending_keeps_it_open() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Pending])));
        assert_eq!(stack.len(), 1);
        assert!(stack.active().is_some());
        let outcome = stack.route_key(key(KeyCode::Down)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::Pending));
        assert_eq!(stack.len(), 1, "pending view stays open");
    }

    #[test]
    fn only_the_top_view_receives_keys() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView));
        stack.route_key(key(KeyCode::Char('x')));
        let bottom = stack.views()[0]
            .as_any()
            .downcast_ref::<StubView>()
            .expect("bottom stub");
        assert_eq!(bottom.keys_seen, 0, "keys never reach covered views");
    }

    #[test]
    fn cancelled_pops_only_the_active_view_even_over_a_dismissing_parent() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Cancelled])));
        let outcome = stack.route_key(key(KeyCode::Esc)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::Cancelled));
        assert_eq!(stack.len(), 1, "cancel returns to the parent flow");
    }

    #[test]
    fn accepted_pops_the_view_and_keeps_a_non_dismissing_parent() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Accepted(
            ViewAction::Selected(2),
        )])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(
            outcome,
            ViewOutcome::Accepted(ViewAction::Selected(2))
        ));
        assert_eq!(stack.len(), 1, "parent without the dismiss flag stays");
    }

    #[test]
    fn child_accept_dismisses_every_flagged_parent_in_a_row() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new()))); // unflagged root
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Accepted(
            ViewAction::Selected(0),
        )])));
        stack.route_key(key(KeyCode::Enter));
        assert_eq!(stack.len(), 1, "both flagged parents dismissed with child");
    }

    #[test]
    fn open_view_pushes_a_child_on_top_of_the_requester() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::OpenView(
            Box::new(OtherView),
        )])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(
            matches!(outcome, ViewOutcome::Pending),
            "OpenView is consumed by the stack"
        );
        assert_eq!(stack.len(), 2);
        assert!(stack.active().expect("child").as_any().is::<OtherView>());
    }

    #[test]
    fn app_level_outcomes_are_forwarded_and_pop_the_view() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![
            ViewOutcome::RunCommand(CommandAction::Quit),
        ])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(
            outcome,
            ViewOutcome::RunCommand(CommandAction::Quit)
        ));
        assert!(stack.is_empty());

        stack.push(Box::new(StubView::returning(vec![
            ViewOutcome::SubmitPrompt("hi".to_string()),
        ])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::SubmitPrompt(ref p) if p == "hi"));
        assert!(stack.is_empty());
    }

    #[test]
    fn paste_routes_to_the_active_view_and_defaults_to_swallowed() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView)); // default handle_paste → Pending
        let outcome = stack.route_paste("pasted").expect("view active");
        assert!(matches!(outcome, ViewOutcome::Pending));
        assert_eq!(stack.len(), 2, "swallowed paste closes nothing");
        let bottom = stack.views()[0]
            .as_any()
            .downcast_ref::<StubView>()
            .expect("bottom stub");
        assert_eq!(bottom.pastes_seen, 0, "paste never reaches covered views");
    }

    #[test]
    fn contains_finds_views_anywhere_in_the_stack() {
        let mut stack = ViewStack::new();
        assert!(!stack.contains::<StubView>());
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView));
        assert!(stack.contains::<StubView>(), "buried view is still found");
        assert!(stack.contains::<OtherView>());
    }
}
