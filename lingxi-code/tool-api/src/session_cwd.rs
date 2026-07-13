//! `SessionCwd` — a shared, lock-free-read, switchable cell holding the
//! session's current working directory and its trusted-directory set.
//!
//! Backed by `arc-swap` so reads (`cwd()`, `trusted_dirs()`) never block on a
//! writer; `swap` atomically publishes a new `(cwd, trusted)` pair and fires
//! an optional on-swap callback (used by later tasks to invalidate
//! cwd-dependent caches). Until `set_on_swap` is called the callback is a
//! no-op, so constructing a `SessionCwd` and never calling `swap` is fully
//! inert.
//!
//! The cwd and its trusted-directory set are published together as a single
//! `Arc<CwdState>` so a concurrent reader can never observe a new cwd paired
//! with a stale trusted-directory set (or vice-versa) — see [`CwdState`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

/// Callback invoked with the new cwd at the end of every [`SessionCwd::swap`].
type OnSwapCallback = Arc<dyn Fn(&Path) + Send + Sync>;

/// The cwd + trusted-directory set as they are published together, so a
/// single `ArcSwap` load always returns a matched pair — never cwd from one
/// generation paired with trusted_dirs from another.
struct CwdState {
    cwd: PathBuf,
    trusted_dirs: Vec<PathBuf>,
}

/// Shared, switchable session cwd + trusted-directory set.
///
/// Always held behind an `Arc` (see [`SessionCwd::new`]) so every tool
/// invocation can clone a handle and observe the latest swap.
pub struct SessionCwd {
    state: ArcSwap<CwdState>,
    on_swap: Mutex<Option<OnSwapCallback>>,
}

impl SessionCwd {
    /// Construct a new cell initialized to the process boot cwd and its
    /// initial trusted-directory set.
    #[must_use]
    pub fn new(boot_cwd: PathBuf, trusted: Vec<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            state: ArcSwap::from_pointee(CwdState {
                cwd: boot_cwd,
                trusted_dirs: trusted,
            }),
            on_swap: Mutex::new(None),
        })
    }

    /// Current cwd (cloned out of the cell).
    #[must_use]
    pub fn cwd(&self) -> PathBuf {
        self.state.load().cwd.clone()
    }

    /// Current trusted-directory set (cloned out of the cell).
    #[must_use]
    pub fn trusted_dirs(&self) -> Vec<PathBuf> {
        self.state.load().trusted_dirs.clone()
    }

    /// Current `(cwd, trusted_dirs)` pair from a SINGLE `load()` of the shared
    /// state cell — the same generation for both fields.
    ///
    /// Prefer this over calling [`Self::cwd`] and [`Self::trusted_dirs`] back
    /// to back at any call site that needs BOTH: two separate accessor calls
    /// each do their own `load()`, so a concurrent `swap()` in between them
    /// could hand back a cwd from one generation paired with trusted_dirs from
    /// the next (or previous) generation. `snapshot()` loads the `CwdState`
    /// once and reads both fields off that single `Arc`, so it can never
    /// straddle a swap.
    #[must_use]
    pub fn snapshot(&self) -> (PathBuf, Vec<PathBuf>) {
        let state = self.state.load();
        (state.cwd.clone(), state.trusted_dirs.clone())
    }

    /// Atomically publish a new cwd + trusted-directory set as a single
    /// pair, then invoke the on-swap callback (if one has been registered)
    /// with the new cwd.
    ///
    /// The callback is invoked *after* the internal lock guarding it has
    /// been released: we only hold the lock long enough to clone out the
    /// `Arc<dyn Fn>`. This means a panicking callback unwinds without
    /// poisoning the mutex (so subsequent `swap()` calls keep working), and
    /// a reentrant callback that itself calls `swap()` or `set_on_swap()`
    /// cannot self-deadlock.
    pub fn swap(&self, cwd: PathBuf, trusted: Vec<PathBuf>) {
        self.state.store(Arc::new(CwdState {
            cwd: cwd.clone(),
            trusted_dirs: trusted,
        }));

        let cb = self.on_swap.lock().expect("on_swap mutex poisoned").clone();
        if let Some(cb) = cb {
            cb(&cwd);
        }
    }

    /// Register a callback invoked (with the new cwd) at the end of every
    /// [`SessionCwd::swap`]. No-op until called; replaces any prior callback.
    pub fn set_on_swap(&self, cb: Box<dyn Fn(&Path) + Send + Sync>) {
        *self.on_swap.lock().expect("on_swap mutex poisoned") = Some(Arc::from(cb));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn boot() -> PathBuf {
        PathBuf::from("/boot/cwd")
    }

    #[test]
    fn new_cwd_equals_boot() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        assert_eq!(sc.cwd(), boot());
    }

    #[test]
    fn new_trusted_dirs_equals_initial() {
        let trusted = vec![boot(), PathBuf::from("/other")];
        let sc = SessionCwd::new(boot(), trusted.clone());
        assert_eq!(sc.trusted_dirs(), trusted);
    }

    #[test]
    fn swap_then_cwd_equals_new() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let new_cwd = PathBuf::from("/worktree/foo");
        sc.swap(new_cwd.clone(), vec![new_cwd.clone()]);
        assert_eq!(sc.cwd(), new_cwd);
    }

    #[test]
    fn trusted_dirs_tracks_swap() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let new_cwd = PathBuf::from("/worktree/foo");
        let new_trusted = vec![new_cwd.clone(), PathBuf::from("/worktree/bar")];
        sc.swap(new_cwd, new_trusted.clone());
        assert_eq!(sc.trusted_dirs(), new_trusted);
    }

    #[test]
    fn no_swap_is_inert() {
        // Constructing and never swapping must leave cwd/trusted untouched —
        // the INERT INVARIANT this whole plan depends on.
        let sc = SessionCwd::new(boot(), vec![boot()]);
        assert_eq!(sc.cwd(), boot());
        assert_eq!(sc.trusted_dirs(), vec![boot()]);
    }

    #[test]
    fn on_swap_callback_fires_with_new_cwd() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let seen: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let seen_clone = Arc::clone(&seen);
        sc.set_on_swap(Box::new(move |p: &Path| {
            *seen_clone.lock().unwrap() = Some(p.to_path_buf());
        }));
        let new_cwd = PathBuf::from("/worktree/foo");
        sc.swap(new_cwd.clone(), vec![new_cwd.clone()]);
        assert_eq!(*seen.lock().unwrap(), Some(new_cwd));
    }

    #[test]
    fn on_swap_callback_is_noop_until_set() {
        // No callback registered — swap must not panic and must still take
        // effect.
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let new_cwd = PathBuf::from("/worktree/foo");
        sc.swap(new_cwd.clone(), vec![new_cwd.clone()]);
        assert_eq!(sc.cwd(), new_cwd);
    }

    #[test]
    fn snapshot_returns_boot_pair_before_any_swap() {
        let trusted = vec![boot(), PathBuf::from("/other")];
        let sc = SessionCwd::new(boot(), trusted.clone());
        assert_eq!(sc.snapshot(), (boot(), trusted));
    }

    #[test]
    fn snapshot_reflects_swap_as_one_matched_pair() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let new_cwd = PathBuf::from("/worktree/foo");
        let new_trusted = vec![new_cwd.clone(), PathBuf::from("/worktree/bar")];
        sc.swap(new_cwd.clone(), new_trusted.clone());
        assert_eq!(sc.snapshot(), (new_cwd, new_trusted));
    }

    #[test]
    fn swap_publishes_atomic_pair() {
        // After a swap, cwd() and trusted_dirs() must both reflect the new
        // values together — the pair is published as one unit, never with
        // one field lagging the other.
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let new_cwd = PathBuf::from("/worktree/foo");
        let new_trusted = vec![new_cwd.clone(), PathBuf::from("/worktree/bar")];
        sc.swap(new_cwd.clone(), new_trusted.clone());
        assert_eq!(sc.cwd(), new_cwd);
        assert_eq!(sc.trusted_dirs(), new_trusted);
    }

    #[test]
    fn concurrent_swaps_never_expose_torn_pair() {
        // Each swap in this test publishes trusted_dirs == vec![cwd], so any
        // single load of the shared `CwdState` must show that exact
        // invariant — proving cwd and trusted_dirs are always published
        // together as one atomic pair, never observed from two different
        // generations. This directly exercises the fix that replaced two
        // independent ArcSwap cells with a single ArcSwap<CwdState>.
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let n_swaps = 500;

        let writer = {
            let sc = Arc::clone(&sc);
            thread::spawn(move || {
                for i in 0..n_swaps {
                    let p = PathBuf::from(format!("/worktree/{i}"));
                    sc.swap(p.clone(), vec![p]);
                }
            })
        };

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let sc = Arc::clone(&sc);
                thread::spawn(move || {
                    for _ in 0..2000 {
                        // Load the single state cell once so cwd and
                        // trusted_dirs are guaranteed to come from the same
                        // published generation.
                        let state = sc.state.load();
                        assert_eq!(state.trusted_dirs, vec![state.cwd.clone()]);
                    }
                })
            })
            .collect();

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }
    }

    #[test]
    fn panicking_on_swap_callback_does_not_poison_mutex() {
        // A panicking callback must unwind without poisoning the internal
        // on_swap mutex, and without leaving the callback lock held (which
        // would also self-deadlock a reentrant callback). A later swap —
        // even with a fresh, non-panicking callback — must still succeed.
        let sc = SessionCwd::new(boot(), vec![boot()]);
        sc.set_on_swap(Box::new(|_p: &Path| {
            panic!("boom");
        }));

        let new_cwd = PathBuf::from("/worktree/foo");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sc.swap(new_cwd.clone(), vec![new_cwd.clone()]);
        }));
        assert!(result.is_err(), "expected the panicking callback to unwind");

        // The swap's new state must have been published even though the
        // callback afterwards panicked.
        assert_eq!(sc.cwd(), new_cwd);

        let saw_later: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let saw_later_clone = Arc::clone(&saw_later);
        sc.set_on_swap(Box::new(move |p: &Path| {
            *saw_later_clone.lock().unwrap() = Some(p.to_path_buf());
        }));
        let later_cwd = PathBuf::from("/worktree/bar");
        sc.swap(later_cwd.clone(), vec![later_cwd.clone()]);
        assert_eq!(sc.cwd(), later_cwd);
        assert_eq!(*saw_later.lock().unwrap(), Some(later_cwd));
    }

    #[test]
    fn concurrent_reads_see_last_swap() {
        let sc = SessionCwd::new(boot(), vec![boot()]);
        let n_swaps = 50;
        let swap_count = Arc::new(AtomicUsize::new(0));

        let writer = {
            let sc = Arc::clone(&sc);
            let swap_count = Arc::clone(&swap_count);
            thread::spawn(move || {
                for i in 0..n_swaps {
                    let p = PathBuf::from(format!("/worktree/{i}"));
                    sc.swap(p.clone(), vec![p]);
                    swap_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        };

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let sc = Arc::clone(&sc);
                thread::spawn(move || {
                    // `cwd()` and `trusted_dirs()` each do their own
                    // `load()` of the single shared state cell, so back-to-
                    // back calls may still straddle two different swaps if
                    // a writer publishes in between them — that race is
                    // inherent to exposing two separate accessor methods
                    // and is not what this test checks. This test only
                    // asserts each individual read is well-formed
                    // (never torn/garbage) and that trusted_dirs always
                    // carries exactly one entry, as every swap in this test
                    // publishes.
                    for _ in 0..200 {
                        let cwd = sc.cwd();
                        let trusted = sc.trusted_dirs();
                        assert!(cwd == boot() || cwd.starts_with("/worktree/"));
                        assert_eq!(trusted.len(), 1);
                    }
                })
            })
            .collect();

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }

        assert_eq!(swap_count.load(Ordering::SeqCst), n_swaps);
        // Final state must reflect the very last swap.
        assert_eq!(sc.cwd(), PathBuf::from(format!("/worktree/{}", n_swaps - 1)));
    }
}
