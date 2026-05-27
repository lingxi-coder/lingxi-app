//! `InteractivePromptingGate` — stdin/stderr prompt loop.
//!
//! Scaffolded in M5-05 Task 1 (struct shell + constructor signature only);
//! the prompt-format / parse / retry surfaces land in Tasks 5-10.
#![forbid(unsafe_code)]

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

/// Interactive permission gate driven by stdin/stderr.
///
/// Production wiring (M5-12) constructs with `tokio::io::stdin()` +
/// `tokio::io::stderr()`. Tests use `tokio::io::duplex` scripts.
///
/// The body of the [`crate::PromptingGate`] / [`crate::PermissionGate`]
/// impls lands in M5-05 Tasks 5-11.
pub struct InteractivePromptingGate {
    #[allow(dead_code)]
    pub(crate) stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
    #[allow(dead_code)]
    pub(crate) stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
}

impl InteractivePromptingGate {
    /// Construct a new interactive gate over the given stdin / stderr endpoints.
    #[must_use]
    pub fn new(
        stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
        stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
    ) -> Self {
        Self { stdin, stderr }
    }
}
