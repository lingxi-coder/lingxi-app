//! Minijail FFI wrapper (P0a): the smoke probe proving libminijail links and
//! a jailed `sh -c true` survives on-device. The P2 plan adds the full
//! `minijail_run_pid_pipes` spawn path here.
//!
//! This is the only workspace crate allowed to touch minijail FFI. The
//! Android-only module below binds the minimal set of `libminijail.h` entry
//! points directly (the upstream `minijail`/`minijail-sys` crates are
//! bypassed — their build script cannot cross-compile from a macOS host; see
//! `build.rs`). Host builds compile no unsafe code at all and report the
//! smoke as structurally unavailable.

pub mod seccomp;
pub use seccomp::{net_deny_policy_hash, net_deny_policy_name, net_deny_policy_text};

use serde::Serialize;

/// Result of the on-device minijail smoke (serialized to the instrumentation
/// test through the `android_sandbox_smoke()` `UniFFI` export).
#[derive(Debug, Clone, Serialize)]
pub struct SmokeResult {
    /// Overall pass.
    pub ok: bool,
    /// `no_new_privs` was applied.
    pub no_new_privs: bool,
    /// The jailed child ran and exited 0.
    pub child_exit_zero: bool,
    /// Failure detail when `ok == false`.
    pub reason: Option<String>,
}

impl SmokeResult {
    /// Failure result captured before/after `no_new_privs` was applied.
    fn fail(no_new_privs: bool, reason: String) -> Self {
        Self {
            ok: false,
            no_new_privs,
            child_exit_zero: false,
            reason: Some(reason),
        }
    }
}

/// Run the minijail smoke. Host builds report a structural "not android".
#[must_use]
pub fn minijail_smoke() -> SmokeResult {
    #[cfg(not(target_os = "android"))]
    {
        SmokeResult::fail(false, "minijail smoke requires an Android device".into())
    }
    #[cfg(target_os = "android")]
    {
        android_impl::smoke()
    }
}

#[cfg(target_os = "android")]
mod android_impl {
    // LINK ANCHOR — do not remove. `platform-android-libcap` is a build-only
    // crate (empty lib.rs) whose rlib BUNDLES the static libcap objects that
    // resolve libminijail's `cap_*` references. rustc only links crates that
    // are actually referenced, and a `-shared` (cdylib) link does not error on
    // the resulting undefined symbols — without this reference the `.so`
    // builds fine but `dlopen` fails on-device with
    // `cannot locate symbol "cap_get_proc"` (caught by the P0a smoke gate).
    use platform_android_libcap as _;

    use super::SmokeResult;
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};
    use std::ptr;

    /// Opaque `struct minijail` handle from `libminijail.h`.
    #[repr(C)]
    struct RawMinijail {
        _opaque: [u8; 0],
    }

    // Hand-written bindings for the exact `libminijail.h` prototypes the
    // smoke needs (libminijail.a is built and linked by build.rs):
    //   struct minijail *minijail_new(void);
    //   void minijail_no_new_privs(struct minijail *j);
    //   int minijail_preserve_fd(struct minijail *j, int parent_fd,
    //                            int child_fd);
    //   int minijail_run_pid_pipes_no_preload(struct minijail *j,
    //       const char *filename, char *const argv[], pid_t *pchild_pid,
    //       int *pstdin_fd, int *pstdout_fd, int *pstderr_fd);
    //   int minijail_wait(struct minijail *j);
    //   void minijail_destroy(struct minijail *j);
    #[allow(unsafe_code)]
    extern "C" {
        fn minijail_new() -> *mut RawMinijail;
        fn minijail_no_new_privs(j: *mut RawMinijail);
        fn minijail_preserve_fd(j: *mut RawMinijail, parent_fd: c_int, child_fd: c_int) -> c_int;
        fn minijail_run_pid_pipes_no_preload(
            j: *mut RawMinijail,
            filename: *const c_char,
            argv: *const *mut c_char,
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
            // SAFETY: `self.0` is a non-null handle returned by
            // `minijail_new` (checked at construction) and destroyed once.
            #[allow(unsafe_code)]
            unsafe {
                minijail_destroy(self.0);
            }
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn smoke() -> SmokeResult {
        // (a) Construct a jail and set no_new_privs.
        // SAFETY: plain constructor; the NULL result is checked.
        let jail = unsafe { minijail_new() };
        if jail.is_null() {
            return SmokeResult::fail(false, "minijail_new returned NULL".into());
        }
        let jail = Jail(jail);
        // SAFETY: `jail.0` is valid; setter has no other preconditions.
        unsafe { minijail_no_new_privs(jail.0) };

        // (b) Inherit stdio: preserve fds 0/1/2 into the child as-is.
        for fd in 0..3 {
            // SAFETY: `jail.0` is valid; fds are plain integers.
            let rc = unsafe { minijail_preserve_fd(jail.0, fd, fd) };
            if rc != 0 {
                return SmokeResult::fail(true, format!("minijail_preserve_fd({fd}) -> {rc}"));
            }
        }

        // (c) Fork + exec `/system/bin/sh -c true` inside the jail.
        let filename = CString::new("/system/bin/sh").expect("static path");
        let arg_cstrings = ["sh", "-c", "true"].map(|a| CString::new(a).expect("static arg"));
        let mut argv: Vec<*mut c_char> = arg_cstrings
            .iter()
            .map(|a| a.as_ptr().cast_mut())
            .chain(std::iter::once(ptr::null_mut()))
            .collect();
        let mut pid: libc::pid_t = 0;
        // SAFETY: `filename`/`arg_cstrings` CStrings outlive the call, `argv` is
        // NULL-terminated, `pid` is a valid out-pointer, and the NULL pipe
        // pointers mean "no pipes" per libminijail.h.
        let rc = unsafe {
            minijail_run_pid_pipes_no_preload(
                jail.0,
                filename.as_ptr(),
                argv.as_mut_ptr(),
                &raw mut pid,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if rc != 0 {
            return SmokeResult::fail(true, format!("minijail_run -> {rc}"));
        }
        if pid <= 0 {
            return SmokeResult::fail(true, format!("minijail_run returned pid {pid}"));
        }

        // (d) Wait: minijail_wait returns the child's nonnegative exit
        // status, or a negative error.
        // SAFETY: `jail.0` is valid and its child has not been waited yet.
        let status = unsafe { minijail_wait(jail.0) };
        let exited_zero = status == 0;
        SmokeResult {
            ok: exited_zero,
            no_new_privs: true,
            child_exit_zero: exited_zero,
            reason: if exited_zero {
                None
            } else {
                Some(format!("minijail_wait -> {status} (pid {pid})"))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_smoke_reports_structurally_unavailable() {
        let r = minijail_smoke();
        assert!(!r.ok);
        assert!(!r.no_new_privs);
        assert!(!r.child_exit_zero);
        assert!(r.reason.unwrap().contains("Android"));
    }
}
