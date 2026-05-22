//! Tool system — `Tool` trait, registry, and concurrency-partitioned dispatcher.
//!
//! M1.4 ships the `Tool` trait + context + error types. Subsequent tasks add
//! registry, dispatcher, progress channel, content replacement, etc.

#![forbid(unsafe_code)]

pub mod content_replacement;
pub mod context;
pub mod progress;
pub mod tool_trait;

pub use context::{ToolUseContext, ToolUseOptions};
pub use tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, SearchReadInfo, Tool, ToolCallResult,
    ToolError, ToolStaticContext, ValidationError,
};
