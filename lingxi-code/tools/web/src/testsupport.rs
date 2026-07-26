//! Test-only shared state guards.

/// Serializes every test that touches the PROCESS-GLOBAL web caches
/// (`cache::URL_CACHE`, `blocklist::DOMAIN_CHECK_CACHE`).
///
/// Both are `static`s shared by the whole test binary, and tests across
/// `web_fetch_test`, `cache` and `blocklist` clear them at entry. Clearing at
/// the top of a test does NOT isolate it: a test running in parallel can clear
/// the entry this one just populated, turning an expected cache HIT into a real
/// fetch that then consumes the wrong mocked response. That is precisely how
/// `second_call_is_served_from_cache` failed — it read the preflight body
/// (`{"can_fetch":true}`) instead of `"cached body"` — and it reproduced only
/// under a saturated full-workspace run, which is how it survived this long.
///
/// Lives here rather than in one test module so the three modules that share
/// the caches also share ONE lock; two separate locks would serialize each
/// module against itself and leave them racing each other.
///
/// A `tokio::sync::Mutex` so the guard can be held across the `.await`s these
/// tests contain without tripping `await_holding_lock`.
pub(crate) async fn web_cache_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// [`web_cache_lock`] for a SYNC test. Blocks on the same lock, so a
/// non-`async` test that clears the caches still serializes against the async
/// ones — skipping it there would leave the exact hole the lock exists to
/// close.
pub(crate) fn block_on_web_cache_lock() -> tokio::sync::MutexGuard<'static, ()> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime")
        .block_on(web_cache_lock())
}
