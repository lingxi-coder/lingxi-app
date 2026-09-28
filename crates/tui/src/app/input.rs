use super::RataApp;
use crate::chat_widget::ChatOutcome;
use crate::RataTerminal;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

impl<'cb> RataApp<'cb> {
    /// Route one key press into the chat widget.
    pub(super) fn on_key(&mut self, key: KeyEvent) -> ChatOutcome {
        self.chat_widget.handle_key(key)
    }
    /// Route a bracketed paste into the chat widget.
    pub(super) fn on_paste(&mut self, text: &str) -> ChatOutcome {
        self.chat_widget.handle_paste(text)
    }
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent, terminal: &RataTerminal) {
        let position = Position::new(mouse.column, mouse.row);
        let area = terminal.current_buffer().area;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.selection.begin(position, area),
            MouseEventKind::Drag(MouseButton::Left) => self.selection.update(position, area),
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(text) =
                    self.selection
                        .finish_at(position, area, terminal.current_buffer())
                else {
                    return;
                };
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
