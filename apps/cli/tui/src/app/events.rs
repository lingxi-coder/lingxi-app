use super::RataApp;
use permission::computer_access::ComputerAccessExchange;
use tool_api::ask_user_question::AskUserQuestionExchange;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;

impl<'cb> RataApp<'cb> {
    /// Fold one streaming event from the orchestrator bridge into the chat
    /// widget's transcript/turn state (the `events_rx` drain path; see
    /// [`crate::chat_widget::ChatWidget::apply_turn_event`]).
    pub(super) fn apply_turn_event(&mut self, event: TurnEvent) {
        if matches!(&event, TurnEvent::SessionCleared { .. }) {
            self.reset_mod_ui_render_cache();
        }
        self.chat_widget.apply_turn_event(event);
        if let Some((args, token)) = self.chat_widget.take_ready_compact() {
            (self.callbacks.on_compact)(args, token);
        }
    }
    /// Open a permission prompt for `exchange` — or queue it when one is
    /// already open; prompts are serialized inside the widget (the
    /// `permission_rx` drain path; see [`crate::chat_widget::ChatWidget::open_permission`]).
    pub(super) fn open_permission(&mut self, exchange: PermissionExchange) {
        self.chat_widget.open_permission(exchange);
    }
    pub(super) fn open_ask_user_question(&mut self, exchange: AskUserQuestionExchange) {
        self.chat_widget.open_ask_user_question(exchange);
    }
    pub(super) fn open_computer_access(&mut self, exchange: ComputerAccessExchange) {
        self.chat_widget.open_computer_access(exchange);
    }
}
