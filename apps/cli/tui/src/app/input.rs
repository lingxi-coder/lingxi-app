use super::RataApp;
use crate::chat_widget::ChatOutcome;
use crate::RataTerminal;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use lingxi_core::host::ModUiSelection;
use ratatui::layout::Position;

impl<'cb> RataApp<'cb> {
    /// Route one key press into the chat widget.
    pub(super) fn on_key(&mut self, key: KeyEvent) -> ChatOutcome {
        if self.chat_widget.mod_ui_button_press_allowed() {
            if let Some(action) =
                crate::mod_ui_render::button_for_key(&self.mod_ui_button_actions, key, true, false)
            {
                self.press_mod_ui_button(action);
                return ChatOutcome::Continue;
            }
        }
        let was_open = self.chat_widget.command_completion_is_open();
        let outcome = self.chat_widget.handle_key(key);
        if !was_open && self.chat_widget.command_completion_is_open() {
            (self.callbacks.on_refresh_command_catalog)();
        }
        outcome
    }
    /// Route a bracketed paste into the chat widget.
    pub(super) fn on_paste(&mut self, text: &str) -> ChatOutcome {
        let was_open = self.chat_widget.command_completion_is_open();
        let outcome = self.chat_widget.handle_paste(text);
        if !was_open && self.chat_widget.command_completion_is_open() {
            (self.callbacks.on_refresh_command_catalog)();
        }
        outcome
    }
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent, terminal: &RataTerminal) {
        let position = Position::new(mouse.column, mouse.row);
        let area = terminal.current_buffer().area;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.selection.begin(position, area),
            MouseEventKind::Drag(MouseButton::Left) => {
                self.selection.update(position, area);
                if let Some(text) = self.selection.current_text(terminal.current_buffer()) {
                    let request_id = self.selection.selected_rows().and_then(|(first, last)| {
                        self.chat_widget
                            .fullscreen_selection_request_id(first, last, area)
                    });
                    self.chat_widget
                        .set_mod_ui_selection(Some(ModUiSelection { text, request_id }));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.selection.update(position, area);
                let request_id = self.selection.selected_rows().and_then(|(first, last)| {
                    self.chat_widget
                        .fullscreen_selection_request_id(first, last, area)
                });
                let selected = self
                    .selection
                    .finish_at(position, area, terminal.current_buffer());
                let Some(text) = selected else {
                    return;
                };
                self.chat_widget.set_mod_ui_selection(Some(ModUiSelection {
                    text: text.clone(),
                    request_id,
                }));
                if !self.copy_on_select {
                    return;
                }
                let tx = self.copy_tx.clone();
                std::thread::spawn(move || {
                    let result = crate::copy::copy_to_clipboard(&text).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
            }
            _ => {}
        }
    }
}
