//! `run_jailed`: fork + jail + exec one command, capture output, enforce the
//! timeout by killing the process group, reap. The only fork+exec path in the
//! engine (spec r3 D6: in-engine `minijail_run_*`). All unsafe FFI is here.
//!
//! Host builds cannot jail, so `run_jailed` reports an enforcement failure and
//! callers stay fail-closed. The Android body is `#[cfg(target_os = "android")]`
//! and is verified by the `arm64`/`x86_64` cross-build plus the on-device
//! instrumentation gate — it cannot execute on the macOS host.

use crate::{JailSpec, JailedOutput};

/// Run `spec` to completion under Minijail. Host builds cannot jail — they
/// report enforcement failure so callers stay fail-closed.
#[must_use]
pub fn run_jailed(spec: &JailSpec) -> JailedOutput {
    #[cfg(not(target_os = "android"))]
    {
        let _ = spec;
        JailedOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: false,
            enforcement_failed: Some("jailed execution requires an Android device".into()),
        }
    }
    #[cfg(target_os = "android")]
    {
        android_impl::run(spec)
    }
}

#[cfg(target_os = "android")]
mod android_impl {
    // LINK ANCHOR — do not remove. `platform-android-libcap` is a build-only
    // crate whose rlib BUNDLES the static libcap objects that resolve
    // libminijail's `cap_*` references. rustc only links crates that are
    // actually referenced, and a `-shared` (cdylib) link does not error on the
    // resulting undefined symbols — without this reference the `.so` builds
    // fine but `dlopen` fails on-device with `cannot locate symbol
    // "cap_get_proc"`. The smoke path in lib.rs has its own anchor; keep this
    // one too so the run module stays self-sufficient if linking changes.
    use platform_android_libcap as _;

    use crate::{JailSpec, JailedOutput};
    use std::ffi::CString;
    use std::io::Read;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::io::{FromRawFd, RawFd};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Opaque `struct minijail` handle from `libminijail.h`.
    #[repr(C)]
    struct RawMinijail {
        _opaque: [u8; 0],
    }

    /// `MINIJAIL_HOOK_EVENT_PRE_EXECVE` — the hook runs just before `execve(2)`.
    /// `minijail_hook_event_t` is a plain (unannotated) C enum in
    /// `libminijail.h`, so its members number from 0 in declaration order:
    /// `PRE_DROP_CAPS=0`, `PRE_EXECVE=1`, `PRE_CHROOT=2`, `MAX=3`. Verified
    /// against the vendored header (`third_party/minijail/libminijail.h:89-101`).
    const MINIJAIL_HOOK_EVENT_PRE_EXECVE: c_int = 1;

    // Hand-written bindings for the exact `libminijail.h` prototypes the runner
    // needs (libminijail.a is built and linked by build.rs). Each prototype is
    // reproduced from the vendored header so reviewers can diff it 1:1:
    //   struct minijail *minijail_new(void);
    //   void minijail_no_new_privs(struct minijail *j);
    //   int  minijail_rlimit(struct minijail *j, int type,
    //                        rlim_t cur, rlim_t max);            // :277
    //   void minijail_use_seccomp_filter(struct minijail *j);    // :129
    //   void minijail_set_seccomp_filter_tsync(struct minijail *j); // :131
    //   void minijail_set_seccomp_filters(struct minijail *j,
    //            const struct sock_fprog *filter);               // :168
    //   int  minijail_create_session(struct minijail *j);        // :343
    //   int  minijail_add_hook(struct minijail *j, minijail_hook_t hook,
    //            void *payload, minijail_hook_event_t event);    // :467
    //       where minijail_hook_t = int (*)(void *context)       // :83
    //   int  minijail_run_env_pid_pipes_no_preload(struct minijail *j,
    //            const char *filename, char *const argv[], char *const envp[],
    //            pid_t *pchild_pid, int *pstdin_fd, int *pstdout_fd,
    //            int *pstderr_fd);                                // :623
    //   int  minijail_wait(struct minijail *j);
    //   void minijail_destroy(struct minijail *j);
    // NOTE: `minijail_log_seccomp_filter_failures` is deliberately NOT bound —
    // libminijail.c die()s if it was called together with set_seccomp_filters.
    #[allow(unsafe_code)]
    extern "C" {
        fn minijail_new() -> *mut RawMinijail;
        fn minijail_no_new_privs(j: *mut RawMinijail);
        fn minijail_rlimit(
            j: *mut RawMinijail,
            r#type: c_int,
            cur: libc::rlim_t,
            max: libc::rlim_t,
        ) -> c_int;
        fn minijail_use_seccomp_filter(j: *mut RawMinijail);
        fn minijail_set_seccomp_filter_tsync(j: *mut RawMinijail);
        fn minijail_set_seccomp_filters(j: *mut RawMinijail, filter: *const libc::sock_fprog);
        fn minijail_create_session(j: *mut RawMinijail) -> c_int;
        fn minijail_add_hook(
            j: *mut RawMinijail,
            hook: extern "C" fn(*mut c_void) -> c_int,
            payload: *mut c_void,
            event: c_int,
        ) -> c_int;
        fn minijail_run_env_pid_pipes_no_preload(
            j: *mut RawMinijail,
            filename: *const c_char,
            argv: *const *mut c_char,
            envp: *const *mut c_char,
            pchild_pid: *mut libc::pid_t,
            pstdin_fd: *mut c_int,
            pstdout_fd: *mut c_int,
            pstderr_fd: *mut c_int,
        ) -> c_int;
        fn minijail_wait(j: *mut RawMinijail) -> c_int;
        fn minijail_destroy(j: *mut RawMinijail);
    }

    /// Owned jail handle: destroys the `struct minijail` on drop so every
    /// early-return path below stays leak-free.
    struct Jail(*mut RawMinijail);

    impl Drop for Jail {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a non-null handle returned by `minijail_new`
            // (checked at construction) and destroyed exactly once (this Drop).
            #[allow(unsafe_code)]
            unsafe {
                minijail_destroy(self.0);
            }
        }
    }

    /// `PRE_EXECVE` hook: `chdir` into the requested cwd inside the jailed child,
    /// just before `execve`. `minijail` has no cwd API, so this is how
    /// `JailSpec::cwd` is applied. `payload` is a `*const c_char` to a NUL-terminated path owned
    /// by the caller (a `CString` kept alive across the whole run — see `run`).
    /// Returns 0 on success or `-errno` so minijail aborts the child if the
    /// directory is gone (fail-closed: never exec in the wrong directory).
    extern "C" fn chdir_hook(payload: *mut c_void) -> c_int {
        if payload.is_null() {
            return -libc::EINVAL;
        }
        // SAFETY: minijail invokes this hook in the forked child just before
        // execve. `payload` is the `cwd` CString pointer we passed to
        // `minijail_add_hook`; that CString is owned by `run`'s stack and
        // outlives the entire jail setup + run, so it is a valid NUL-terminated
        // C string here. `chdir` only reads it.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::chdir(payload.cast::<c_char>()) };
        if rc == 0 {
            0
        } else {
            // errno is positive; report it negated so minijail treats it as a
            // hook failure and aborts the child rather than exec'ing.
            #[allow(unsafe_code)]
            let e = unsafe { *libc::__errno() };
            -e
        }
    }

    /// Drain a captured pipe fd to EOF on its own thread, taking ownership of the
    /// fd (closed when the returned `File` drops inside the thread). Spawned for
    /// BOTH stdout and stderr so a child that fills one pipe buffer while we read
    /// the other cannot deadlock (the classic sequential-read hazard).
    fn spawn_reader(fd: RawFd) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            // SAFETY: `fd` is a freshly returned, owned pipe read-end from
            // `minijail_run_env_pid_pipes_no_preload`. We transfer sole
            // ownership into this `File`; it is closed exactly once when the
            // `File` drops at the end of this thread. No other code touches this
            // fd (the parent never reads/closes it directly).
            #[allow(unsafe_code)]
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            let mut buf = Vec::new();
            // Errors here mean a truncated read (e.g. the child was SIGKILLed by
            // the watchdog); keep whatever we captured.
            let _ = file.read_to_end(&mut buf);
            buf
        })
    }

    /// Decode `minijail_wait`'s status (P0a encoding): nonnegative is the child's
    /// exit code (`status & 0xFF`); a child terminated by signal `n` is reported
    /// by minijail as `128 + n` (>= 128). A SIGKILL (9) → 137, which we map to
    /// `exit_code` -1 and let the `timed_out` flag carry the real cause.
    ///
    /// CAVEAT — 126/127 are NOT guaranteed child exit codes. `minijail_wait`
    /// returns `MINIJAIL_ERR_NO_ACCESS` (126) / `MINIJAIL_ERR_NO_COMMAND` (127)
    /// when minijail itself could not `exec` the target (e.g. not executable /
    /// not found), so a 126/127 here may be a minijail exec-failure code rather
    /// than something the child returned. We surface them as-is: they coincide
    /// with bash's own 126 ("cannot execute") / 127 ("command not found")
    /// conventions, so the value is meaningful to callers either way and needs no
    /// special-casing.
    fn decode_exit(status: c_int) -> i32 {
        if status < 0 {
            // minijail internal error (e.g. wait failed).
            -1
        } else if status >= 128 {
            // Signalled child — not a real exit code.
            -1
        } else {
            status & 0xFF
        }
    }

    fn fail(reason: String) -> JailedOutput {
        JailedOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: false,
            enforcement_failed: Some(reason),
        }
    }

    /// Build the jail, fork+exec the command, capture stdio, enforce the timeout
    /// by killing the child's own process group, reap, and return the outcome.
    /// Any setup failure returns `enforcement_failed` — we NEVER run unconfined.
    ///
    /// This is one linear FFI sequence (build → filter → session → hook →
    /// marshal → run → capture → reap) whose steps share many keep-alive
    /// lifetimes (`CString`s, the filter `Vec`) that must all outlive the single run
    /// call; splitting it into helpers would only scatter those lifetime ties and
    /// obscure the ordering that makes the unsafe calls sound — so it stays one
    /// function with a documented length allowance.
    #[allow(unsafe_code, clippy::too_many_lines)]
    pub(crate) fn run(spec: &JailSpec) -> JailedOutput {
        // (1) Construct the jail. Drop guard destroys it on every return path.
        // SAFETY: plain constructor; NULL is checked immediately.
        let raw = unsafe { minijail_new() };
        if raw.is_null() {
            return fail("minijail_new returned NULL".into());
        }
        let jail = Jail(raw);

        // (2) no_new_privs — mandatory for an unprivileged seccomp filter.
        // SAFETY: `jail.0` is valid; the setter has no other preconditions.
        unsafe { minijail_no_new_privs(jail.0) };

        // (3) rlimits. Each resource int comes from the safe side (RLIMIT_*).
        for rl in &spec.rlimits {
            // SAFETY: `jail.0` is valid; the args are plain integers.
            let rc = unsafe {
                minijail_rlimit(
                    jail.0,
                    rl.resource as c_int,
                    rl.soft as libc::rlim_t,
                    rl.hard as libc::rlim_t,
                )
            };
            if rc != 0 {
                return fail(format!("minijail_rlimit(resource={}) -> {rc}", rl.resource));
            }
        }

        // (4) net-deny seccomp filter (raw classic-BPF, allow-by-default,
        // socket-family -> EPERM). We map the host-built `BpfInsn`s to
        // `libc::sock_filter` (identical 4-field layout) and inject the program
        // verbatim via `minijail_set_seccomp_filters` (the header notes it does
        // NOT take ownership of the filter — hence we keep the Vec alive across
        // the run call below). We intentionally do NOT call
        // `minijail_log_seccomp_filter_failures` (libminijail die()s if combined
        // with set_seccomp_filters).
        //
        // `filter_insns`/`filter_prog` MUST outlive the run call (the filter is
        // non-owning — the kernel reads it during the run). They are bound at
        // function scope so they live until `run` returns; both stay empty/NULL
        // when `!net_deny`. `filter_insns` holds the program alive for the whole
        // jailed lifetime even though minijail keeps only the raw pointer.
        let filter_insns: Vec<libc::sock_filter> = if spec.net_deny {
            spec.bpf
                .iter()
                .map(|i| libc::sock_filter {
                    code: i.code,
                    jt: i.jt,
                    jf: i.jf,
                    k: i.k,
                })
                .collect()
        } else {
            Vec::new()
        };
        // `filter_prog` lives at function scope (kept alive across the run call);
        // it points at `filter_insns` only on the net_deny path, NULL otherwise.
        let filter_prog = if spec.net_deny {
            if filter_insns.is_empty() {
                return fail("net_deny set but BPF program is empty".into());
            }
            let Ok(len) = u16::try_from(filter_insns.len()) else {
                return fail(format!("BPF program too long: {}", filter_insns.len()));
            };
            let prog = libc::sock_fprog {
                len,
                // `filter` is a non-owning pointer; `filter_insns` outlives the run.
                filter: filter_insns.as_ptr().cast_mut(),
            };
            // SAFETY: `jail.0` is valid. `set_seccomp_filters` does NOT take
            // ownership of the filter (header) — the kernel reads `prog` and the
            // `filter_insns` it points at during the run, and both remain alive
            // until after the run call returns (bound in this function's scope).
            // We pair it with use_seccomp_filter (selects seccomp-bpf mode) +
            // tsync (apply to all threads — not the log fn, compatible with
            // set_filters).
            unsafe {
                minijail_use_seccomp_filter(jail.0);
                minijail_set_seccomp_filter_tsync(jail.0);
                minijail_set_seccomp_filters(jail.0, &raw const prog);
            }
            prog
        } else {
            libc::sock_fprog {
                len: 0,
                filter: ptr::null_mut(),
            }
        };
        // Hold the program (and thus its backing `filter_insns`) alive across the
        // run call below even though minijail retains only the raw pointer.
        let _ = &filter_prog;

        // (5) Put the child in its own session/process-group so the watchdog can
        // kill(-pgid) WITHOUT touching the engine's own group. The child becomes
        // the leader of a new pgid == its pid.
        // SAFETY: `jail.0` is valid; create_session has no other preconditions.
        let rc = unsafe { minijail_create_session(jail.0) };
        if rc != 0 {
            return fail(format!("minijail_create_session -> {rc}"));
        }

        // (6) PRE_EXECVE chdir hook. `cwd_c` MUST outlive the run call (the hook
        // runs in the child during minijail_run, reading this pointer).
        let Ok(cwd_c) = CString::new(spec.cwd.as_str()) else {
            return fail("cwd contains an interior NUL byte".into());
        };
        // SAFETY: `jail.0` is valid; `chdir_hook` is an `extern "C"` fn matching
        // `minijail_hook_t = int (*)(void *)`. `payload` is the `cwd_c` pointer,
        // which lives on this stack frame past the run call, so it is valid when
        // the hook fires inside the child. PRE_EXECVE == 1 (verified above).
        let rc = unsafe {
            minijail_add_hook(
                jail.0,
                chdir_hook,
                cwd_c.as_ptr().cast::<c_void>().cast_mut(),
                MINIJAIL_HOOK_EVENT_PRE_EXECVE,
            )
        };
        if rc != 0 {
            return fail(format!("minijail_add_hook(chdir) -> {rc}"));
        }

        // (7) Marshal filename + argv + envp as NUL-terminated C arrays. The
        // CStrings and the pointer Vecs MUST all outlive the run call.
        let Ok(filename) = CString::new(spec.filename.as_str()) else {
            return fail("filename contains an interior NUL byte".into());
        };
        let Ok(argv_c) = spec
            .argv
            .iter()
            .map(|a| CString::new(a.as_str()))
            .collect::<Result<Vec<CString>, _>>()
        else {
            return fail("argv entry contains an interior NUL byte".into());
        };
        let Ok(envp_c) = spec
            .envp
            .iter()
            .map(|(k, v)| CString::new(format!("{k}={v}")))
            .collect::<Result<Vec<CString>, _>>()
        else {
            return fail("env entry contains an interior NUL byte".into());
        };
        // NULL-terminated arrays of `*mut c_char`.
        let mut argv: Vec<*mut c_char> = argv_c
            .iter()
            .map(|c| c.as_ptr().cast_mut())
            .chain(std::iter::once(ptr::null_mut()))
            .collect();
        let mut envp: Vec<*mut c_char> = envp_c
            .iter()
            .map(|c| c.as_ptr().cast_mut())
            .chain(std::iter::once(ptr::null_mut()))
            .collect();

        // (8) Fork + exec inside the jail. stdin is NULL (no stdin in v1).
        // stdin deferred — ProcessCommand.stdin exists but the Shell tool never
        // sets it; wire a stdin pipe in a later task.
        let mut pid: libc::pid_t = 0;
        let mut stdout_fd: c_int = -1;
        let mut stderr_fd: c_int = -1;
        // SAFETY: `jail.0` is valid. `filename`/`argv_c`/`envp_c` CStrings and
        // the `argv`/`envp` pointer Vecs all outlive this call (declared above,
        // dropped only at end of scope). Both pointer arrays are NUL-terminated.
        // `pid`/`stdout_fd`/`stderr_fd` are valid out-params; stdin is NULL
        // ("no stdin pipe" per libminijail.h). The seccomp filter (`_keep_insns`)
        // is still alive here, as required by set_seccomp_filters' non-ownership.
        let rc = unsafe {
            minijail_run_env_pid_pipes_no_preload(
                jail.0,
                filename.as_ptr(),
                argv.as_mut_ptr(),
                envp.as_mut_ptr(),
                &raw mut pid,
                ptr::null_mut(), // stdin deferred
                &raw mut stdout_fd,
                &raw mut stderr_fd,
            )
        };
        if rc != 0 {
            return fail(format!("minijail_run_env_pid_pipes_no_preload -> {rc}"));
        }
        if pid <= 0 {
            return fail(format!("minijail_run returned pid {pid}"));
        }
        if stdout_fd < 0 || stderr_fd < 0 {
            // Should never happen: minijail_run succeeded (rc == 0, pid > 0) but
            // handed back a bad fd. Stay fail-closed AND leak-free: close any
            // valid fd we did get, and reap the live child so we leave no zombie.
            // SAFETY: each `close` targets a fd value minijail just returned to
            // us by value; we only close the ones that are >= 0 (valid), and we
            // own them (the reader threads that would otherwise own them have not
            // been spawned on this path). `minijail_wait` reaps `jail.0`'s child,
            // which has not been waited on.
            unsafe {
                if stdout_fd >= 0 {
                    libc::close(stdout_fd);
                }
                if stderr_fd >= 0 {
                    libc::close(stderr_fd);
                }
                minijail_wait(jail.0);
            }
            return fail(format!(
                "minijail_run gave bad pipe fds (out={stdout_fd}, err={stderr_fd})"
            ));
        }

        // The child is the leader of its own pgid (== its pid) thanks to
        // create_session, so kill(-child_pgid) targets ONLY the child group and
        // never the engine's process group.
        let child_pgid = pid;

        // (9) Concurrent capture: drain stdout AND stderr on their own threads so
        // a child filling one pipe buffer while we read the other cannot deadlock.
        // Each thread owns and closes its fd exactly once.
        let stdout_reader = spawn_reader(stdout_fd);
        let stderr_reader = spawn_reader(stderr_fd);

        // (10) Watchdog: after timeout_ms, set `timed_out` and kill the child's
        // process group. It is CANCELLABLE via `done`, and — crucially — it is
        // ALWAYS joined BEFORE the reaping `minijail_wait` (see the ordering
        // proof at step 11), so a `kill(-pgid)` can never fire on a pgid that
        // `minijail_wait` has already reaped and the OS may have recycled. The
        // watchdog polls `done` on a short interval rather than sleeping the full
        // timeout, so cancellation is prompt (returns within one tick of `done`).
        let timed_out = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let timed_out = Arc::clone(&timed_out);
            let done = Arc::clone(&done);
            let timeout = Duration::from_millis(spec.timeout_ms);
            thread::spawn(move || {
                let start = Instant::now();
                let tick = Duration::from_millis(20);
                loop {
                    if done.load(Ordering::SeqCst) {
                        // Normal path: the child exited, the readers hit EOF and
                        // were joined, and `done` was set — all BEFORE the reaping
                        // minijail_wait. Returning here means no kill fires, so
                        // there is no recycled-pgid hazard.
                        return;
                    }
                    if start.elapsed() >= timeout {
                        // Timeout path: the child is genuinely hung (it ignored
                        // the deadline and has not closed its pipes), so it is
                        // still LIVE and UNREAPED — minijail_wait runs only after
                        // this watchdog is joined, and that join happens only
                        // after the readers hit EOF, which the kill below causes.
                        // So the kill always precedes the reap; the pgid cannot be
                        // recycled yet. Re-check `done` once more to lose the race
                        // to a child that exited in the last tick.
                        if done.load(Ordering::SeqCst) {
                            return;
                        }
                        // Mark BEFORE the kill so the result reports timed_out
                        // regardless of how wait() decodes the SIGKILL.
                        timed_out.store(true, Ordering::SeqCst);
                        // SAFETY: `child_pgid` is the child's own pgid (== its
                        // pid; it leads a fresh session from create_session).
                        // Negating it targets that group ONLY — never the
                        // engine's group (which has a different, positive pgid).
                        // The child is unreaped here (reap is the very last step,
                        // after this thread is joined), so the pgid is still ours
                        // and cannot have been recycled.
                        #[allow(unsafe_code)]
                        unsafe {
                            libc::kill(-child_pgid, libc::SIGKILL);
                        }
                        return;
                    }
                    thread::sleep(tick);
                }
            })
        };

        // (11) Tear down in an order that closes the kill/reap TOCTOU window.
        //
        // ORDERING GUARANTEE (why a kill can never hit a recycled pgid):
        // `minijail_wait` is the ONLY reap, and it is the LAST step below — it
        // runs strictly after the watchdog has been joined. The watchdog only
        // ever issues `kill(-pgid)` while the child is still UNREAPED, so the
        // pgid cannot have been recycled at kill time. Walk both paths:
        //
        //  - Normal exit: the child exits and closes its stdout/stderr write
        //    ends → the reader threads hit EOF and return → we join them, set
        //    `done`, and join the watchdog. The watchdog sees `done` (or its
        //    deadline has not elapsed) and returns WITHOUT killing. Only then do
        //    we call minijail_wait. No kill ever fires.
        //
        //  - Hang/timeout: the child ignores the deadline and keeps its pipes
        //    open, so the readers stay blocked. The watchdog's deadline elapses
        //    while the child is still live+unreaped; it sets `timed_out` and
        //    SIGKILLs the (still-ours) pgid. The kill closes the pipes → readers
        //    EOF → we join the readers, set `done`, join the watchdog (already
        //    returned post-kill), THEN minijail_wait reaps the killed child. The
        //    kill happens-before the readers' EOF, which happens-before the
        //    join+reap — so kill strictly precedes reap.
        //
        // Joining the readers FIRST is safe from deadlock: in both paths the
        // child's pipes get closed (natural exit, or the watchdog's kill), so
        // read_to_end returns.

        // Readers finish at EOF (child closed the pipes — it has exited or been
        // killed). Join both before touching the watchdog or reaping.
        let stdout_bytes = stdout_reader.join().unwrap_or_default();
        let stderr_bytes = stderr_reader.join().unwrap_or_default();

        // Cancel + join the watchdog BEFORE the reaping minijail_wait. Setting
        // `done` makes a not-yet-fired watchdog return on its next tick without
        // killing; if it already fired (timeout path) it has returned. Either
        // way, once this join completes no kill can ever run again — so the
        // subsequent reap cannot race a recycled pgid.
        done.store(true, Ordering::SeqCst);
        let _ = watchdog.join();

        // Reap LAST. minijail_wait blocks until the (only) child exits — prompt
        // because by now it has either exited naturally or been SIGKILLed.
        // SAFETY: `jail.0` is valid and its child has not been waited on yet.
        let status = unsafe { minijail_wait(jail.0) };

        let was_timed_out = timed_out.load(Ordering::SeqCst);
        let exit_code = if was_timed_out {
            -1
        } else {
            decode_exit(status)
        };

        JailedOutput {
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            exit_code,
            timed_out: was_timed_out,
            enforcement_failed: None,
        }
        // `jail` (Drop -> minijail_destroy), `_keep_insns`, `filter_prog`,
        // `cwd_c`, `filename`, `argv_c`/`envp_c`, `argv`/`envp` all drop here —
        // after the run + wait, so every pointer handed to minijail stayed valid
        // for the whole jailed lifetime.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_run_reports_enforcement_failure() {
        let spec = JailSpec {
            filename: "/system/bin/sh".into(),
            argv: vec!["sh".into()],
            envp: vec![],
            cwd: "/".into(),
            rlimits: vec![],
            net_deny: false,
            bpf: vec![],
            timeout_ms: 1000,
        };
        let out = run_jailed(&spec);
        assert!(out.enforcement_failed.is_some());
        assert_eq!(out.exit_code, -1);
    }
}
