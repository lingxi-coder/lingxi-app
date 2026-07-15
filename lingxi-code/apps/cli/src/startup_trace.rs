use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn start() {
    let _ = START.get_or_init(Instant::now);
}

pub fn init(debug_filter: Option<&str>) {
    start();
    let enabled = env_enabled() || debug_filter_matches_startup(debug_filter);
    ENABLED.store(enabled, Ordering::Relaxed);
    mark("trace_ready");
}

pub fn mark(phase: &'static str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let elapsed = START.get_or_init(Instant::now).elapsed();
    eprintln!(
        "startup phase={phase} elapsed_ms={}",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn env_enabled() -> bool {
    std::env::var("LINGXI_STARTUP_TRACE").map_or(false, |value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "startup"
        )
    })
}

fn debug_filter_matches_startup(filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return false;
    };
    filter
        .split(',')
        .map(str::trim)
        .any(|part| part.eq_ignore_ascii_case("startup"))
}
