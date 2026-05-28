//! Message renderers — one component per `RenderedMessage` variant.
//!
//! M6-02 ships two renderers (`UserTextMessage`, `AssistantTextMessage`).
//! Tool / permission / streaming variants arrive in M6-03..M6-05.

pub mod assistant_text;
pub mod user_text;
