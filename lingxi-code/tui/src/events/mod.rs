//! Unified event type fed into the TUI event loop.
//!
//! Three async sources fan into a single `mpsc::Receiver<TuiEvent>`:
//! 1. crossterm `EventStream` → `TuiEvent::Key` / `TuiEvent::Resize`.
//! 2. orchestrator-bridge channel → `TuiEvent::OrchestratorMessage`.
//! 3. 100ms `tokio::time::interval` → `TuiEvent::Tick`.

pub mod keymap;
pub mod orchestrator_bridge;

use crossterm::event::KeyEvent;
use traits::OutputEvent;

/// Newtype around `OutputEvent` retained from M6-01 for the legacy
/// `TuiEvent::OrchestratorMessage` payload. M6-03 introduces the richer
/// [`orchestrator_bridge::TurnEvent`] enum that the render loop actually
/// consumes; this newtype remains so the locked `TuiEvent` variant shape
/// stays stable for downstream tests.
#[derive(Debug, Clone, PartialEq)]
pub struct OrchestratorOutputEvent(pub OutputEvent);

impl From<OutputEvent> for OrchestratorOutputEvent {
    fn from(e: OutputEvent) -> Self {
        Self(e)
    }
}

/// Unified TUI event. The event loop `select!`s on a single channel of
/// these and dispatches each one into the iocraft state machine.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// A keyboard event from crossterm.
    Key(KeyEvent),
    /// Terminal resize: (cols, rows).
    Resize(u16, u16),
    /// A message from the orchestrator (text delta, tool call, end-of-turn).
    OrchestratorMessage(OrchestratorOutputEvent),
    /// 100ms animation tick (used by spinner/streaming throttle in later sub-plans).
    Tick,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use traits::OutputEvent;

    #[test]
    fn key_variant_constructs() {
        let k = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let e = TuiEvent::Key(k);
        match e {
            TuiEvent::Key(_) => {}
            _ => panic!("expected Key"),
        }
    }

    #[test]
    fn resize_variant_carries_dims() {
        let e = TuiEvent::Resize(80, 24);
        match e {
            TuiEvent::Resize(c, r) => {
                assert_eq!(c, 80);
                assert_eq!(r, 24);
            }
            _ => panic!("expected Resize"),
        }
    }

    #[test]
    fn tick_variant_constructs() {
        let e = TuiEvent::Tick;
        match e {
            TuiEvent::Tick => {}
            _ => panic!("expected Tick"),
        }
    }

    #[test]
    fn output_event_wraps_into_orchestrator_message() {
        let out = OutputEvent::Text {
            text: "hello".into(),
        };
        let te: OrchestratorOutputEvent = out.into();
        let e = TuiEvent::OrchestratorMessage(te);
        match e {
            TuiEvent::OrchestratorMessage(OrchestratorOutputEvent(OutputEvent::Text { text })) => {
                assert_eq!(text, "hello");
            }
            _ => panic!("expected OrchestratorMessage(Text)"),
        }
    }
}
