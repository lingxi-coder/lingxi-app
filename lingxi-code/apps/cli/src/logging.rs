//! `--debug` → `tracing-subscriber` setup writing to stderr.
//!
//! Without `--debug`, only WARN+ are shown. With `--debug`, DEBUG+ is shown
//! filtered by `RUST_LOG` if set, otherwise defaulting to `lingxi=debug,info`.
//! Output goes to stderr so it doesn't interfere with the conversation
//! transcript on stdout. The `NO_COLOR` env var suppresses ANSI escapes.

use std::io::IsTerminal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise the global tracing subscriber. Idempotent — subsequent calls
/// (e.g. from a second test in the same process) are silent no-ops.
pub fn init(debug: bool) {
    let default_filter = if debug { "lingxi=debug,info" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    // `try_init` returns Err if the global subscriber is already set; we
    // swallow that because tests may call `init` multiple times.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(supports_color()),
        )
        .try_init();
}

fn supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_with_debug_does_not_panic() {
        init(true);
    }

    #[test]
    fn init_twice_does_not_panic() {
        init(false);
        init(true);
    }
}
