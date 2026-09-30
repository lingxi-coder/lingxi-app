//! `--debug` → `tracing-subscriber` setup writing to stderr and, when debug is
//! on, to a per-run file under `<config-home>/debug/`.
//!
//! Without `--debug`, only WARN+ are shown. With `--debug`, DEBUG+ is shown
//! filtered by `RUST_LOG` if set, otherwise defaulting to `lingxi=debug,info`.
//! Output goes to stderr so it doesn't interfere with the conversation
//! transcript on stdout. The `NO_COLOR` env var suppresses ANSI escapes.

use std::fs::File;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Directory the per-run debug log lives in, and the name of the pointer file
/// `memory::retention` preserves while sweeping stale siblings.
const DEBUG_DIR: &str = "debug";
const LATEST: &str = "latest";

/// A `MakeWriter` over one shared file handle.
///
/// `tracing-appender` is not a dependency here and a rotating writer would be
/// more machinery than this needs: one file per run, swept by retention.
#[derive(Clone)]
struct FileSink(Arc<Mutex<File>>);

impl std::io::Write for FileSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_or(Ok(buf.len()), |mut file| file.write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().map_or(Ok(()), |mut file| file.flush())
    }
}

impl<'a> fmt::MakeWriter<'a> for FileSink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Resolve `<config-home>/debug`, honouring the config-dir override.
fn debug_dir() -> Option<PathBuf> {
    let home = std::env::var_os(branding::CONFIG_DIR_ENV).map_or_else(
        || dirs_home().map(|h| branding::config_home(&h, None)),
        |dir| Some(PathBuf::from(dir)),
    )?;
    Some(home.join(DEBUG_DIR))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Open this run's debug log and refresh the `latest` pointer.
///
/// Returns `None` on any filesystem failure: a debug log that cannot be opened
/// must never stop the session from starting.
fn open_debug_log() -> Option<FileSink> {
    open_debug_log_in(&debug_dir()?)
}

/// The testable half: everything except resolving the directory.
///
/// The env read stays in [`debug_dir`] so tests never need `set_var`, which
/// would flake the moment the suite runs in parallel.
fn open_debug_log_in(dir: &std::path::Path) -> Option<FileSink> {
    std::fs::create_dir_all(dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let path = dir.join(format!("{stamp}-{}.log", std::process::id()));
    let file = File::options().create(true).append(true).open(&path).ok()?;
    // `latest` is the name `memory::retention` preserves, so it is the stable
    // entry point for "the log for the run I am in".
    let latest = dir.join(LATEST);
    let _ = std::fs::remove_file(&latest);
    let _ = std::fs::write(&latest, path.to_string_lossy().as_bytes());
    Some(FileSink(Arc::new(Mutex::new(file))))
}

/// Initialise the global tracing subscriber. Idempotent — subsequent calls
/// (e.g. from a second test in the same process) are silent no-ops.
pub fn init(debug: bool, suppress_terminal: bool) {
    init_with_sink(debug, suppress_terminal, &open_debug_log);
}

/// The testable half of [`init`]: everything except resolving the sink.
///
/// ⚠️ Exists because `init` installs a PROCESS-GLOBAL subscriber exactly once,
/// so a test cannot call it twice and observe anything. An audit unwired the
/// file sink in BOTH branches of `init` — i.e. `--debug` wrote nothing, the
/// exact defect this module exists to fix — and every test stayed green,
/// because they all called `open_debug_log_in` directly and never reached here.
fn init_with_sink(
    debug: bool,
    suppress_terminal: bool,
    resolve_sink: &dyn Fn() -> Option<FileSink>,
) {
    // The `debug` gate lives HERE and only here. Splitting it between caller
    // and callee is what let the first version of the wiring test catch a
    // resolver being consulted with `--debug` off.
    let file_layer_for = |resolve: &dyn Fn() -> Option<FileSink>| {
        if debug {
            resolve()
        } else {
            None
        }
    };
    // The fullscreen TUI reconciler owns the terminal, so routing tracing to
    // stderr corrupts the rendered frame (stray WARN lines drawn over the input
    // box / borders). When the interactive TUI is about to mount, install an
    // inert "off" subscriber: log macros stay cheap no-ops and nothing reaches
    // the screen. Use `--no-tui` or `--print` to see logs on stderr.
    // 🚨 The TUI owns the terminal, so stderr logging corrupts the frame. That
    // is exactly the session where a user most needs a log they can read
    // afterwards, so the FILE sink still runs here — previously this branch
    // discarded everything and left nothing to diagnose with.
    if suppress_terminal {
        let file_layer = file_layer_for(resolve_sink).map(|sink| {
            fmt::layer()
                .with_writer(sink)
                .with_ansi(false)
                .with_target(true)
        });
        let filter = if debug {
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lingxi=debug,info"))
        } else {
            EnvFilter::new("off")
        };
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .try_init();
        return;
    }
    let default_filter = if debug { "lingxi=debug,info" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    // `try_init` returns Err if the global subscriber is already set; we
    // swallow that because tests may call `init` multiple times.
    // The file sink is additive: stderr keeps its existing behaviour, and
    // `--debug` additionally leaves a log behind for the `debug` skill and for
    // after-the-fact diagnosis.
    let file_layer = file_layer_for(resolve_sink).map(|sink| {
        fmt::layer()
            .with_writer(sink)
            .with_ansi(false)
            .with_target(true)
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(supports_color()),
        )
        .with(file_layer)
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
        init(true, false);
    }

    #[test]
    fn init_twice_does_not_panic() {
        init(false, false);
        init(true, false);
    }

    #[test]
    fn init_suppressed_does_not_panic() {
        init(false, true);
    }

    /// 🚨 The point of the file sink. `memory::retention` already sweeps
    /// `<config-home>/debug/` and preserves `latest` — it has done so while
    /// NOTHING ever wrote there. This asserts a run actually leaves a log.
    #[test]
    fn a_debug_run_leaves_a_log_and_a_latest_pointer() {
        let dir = std::env::temp_dir().join(format!("lingxi-debuglog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sink = open_debug_log_in(&dir).expect("the log must open");

        {
            use std::io::Write as _;
            let mut sink = sink.clone();
            sink.write_all(b"hello from the run\n").unwrap();
            sink.flush().unwrap();
        }

        let latest = dir.join(LATEST);
        assert!(latest.is_file(), "`latest` is what retention preserves");
        let pointed = std::fs::read_to_string(&latest).unwrap();
        let body = std::fs::read_to_string(pointed.trim()).expect("latest must name a real file");
        assert!(body.contains("hello from the run"), "got {body:?}");

        let logs: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != LATEST)
            .collect();
        assert_eq!(logs.len(), 1, "one log per run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 🚨 The wiring test the audit found missing. Every other test here calls
    /// `open_debug_log_in` directly, so unwiring the sink inside `init` — which
    /// is the ONLY caller in production (`apps/cli/src/lib.rs`) — left them all
    /// green while `--debug` wrote nothing at all.
    ///
    /// Asserts the sink is CONSULTED, in both branches, exactly when `debug` is
    /// on. It cannot assert the subscriber itself: that is process-global and
    /// installed once, which is why the sink is injected here.
    #[test]
    fn init_consults_the_file_sink_exactly_when_debug_is_on() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for suppress_terminal in [false, true] {
            let calls = AtomicUsize::new(0);
            let probe = || -> Option<FileSink> {
                calls.fetch_add(1, Ordering::SeqCst);
                None
            };
            init_with_sink(true, suppress_terminal, &probe);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "`--debug` must reach the file sink (suppress_terminal={suppress_terminal}) — \
                 the TUI branch especially, since it is where stderr is discarded"
            );

            let quiet = AtomicUsize::new(0);
            let never = || -> Option<FileSink> {
                quiet.fetch_add(1, Ordering::SeqCst);
                None
            };
            init_with_sink(false, suppress_terminal, &never);
            assert_eq!(
                quiet.load(Ordering::SeqCst),
                0,
                "without `--debug` nothing may be written"
            );
        }
    }

    /// An unwritable directory must not stop the session: the log is a
    /// diagnostic, never a prerequisite.
    #[test]
    fn an_unopenable_debug_log_is_survivable() {
        let blocked = std::env::temp_dir()
            .join(format!("lingxi-blocked-{}", std::process::id()))
            .join("not-a-dir");
        let _ = std::fs::remove_dir_all(blocked.parent().unwrap());
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        std::fs::write(&blocked, b"I am a file").unwrap();
        assert!(
            open_debug_log_in(&blocked).is_none(),
            "a file where the directory should be must yield None, not panic"
        );
        let _ = std::fs::remove_dir_all(blocked.parent().unwrap());
    }
}
