//! macOS TCC permission probes (Accessibility + Screen Recording).
//!
//! Parity target: `cu.tcc.checkAccessibility()` / `checkScreenRecording()`
//! (Swift, via a private-ish but Apple-documented pair of public C functions).
//! Both are simple no-argument boolean queries, so a raw `extern "C"` binding
//! is simpler and lighter than pulling in a whole accessibility crate for two
//! functions.

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// `AXIsProcessTrusted()` — `ApplicationServices/HIServices`. Does NOT
    /// prompt; a caller that wants the system prompt uses
    /// `AXIsProcessTrustedWithOptions` with the prompt option (not needed
    /// here — the tool surfaces its own guidance instead).
    ///
    /// Declared returning `u8`, NOT Rust `bool`: the real header return type
    /// is `Boolean` (`MacTypes.h`'s `typedef unsigned char Boolean`), a
    /// C `unsigned char` — not C99 `_Bool`. Binding it as Rust `bool` would
    /// be UB per `bool`'s validity invariant (only bit patterns `0x00`/`0x01`
    /// are valid) if the ABI-level byte the callee returns is ever anything
    /// else in the unused high bits of the underlying register/byte.
    fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// `CGPreflightScreenCaptureAccess()` — macOS 10.15+. Read-only query;
    /// does not prompt (that's `CGRequestScreenCaptureAccess`). Genuinely a
    /// C99 `bool` in the real header, unlike `AXIsProcessTrusted`'s legacy
    /// `Boolean` — Rust `bool` is the correct ABI type here.
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// `(accessibility_granted, screen_recording_granted)`.
#[must_use]
pub fn check_os_permissions() -> (bool, bool) {
    // Safety: both functions take no arguments, return a plain boolean-ish
    // value, and have no documented preconditions — trivially safe to call
    // from any thread at any time.
    let accessibility = unsafe { AXIsProcessTrusted() } != 0;
    let screen_recording = unsafe { CGPreflightScreenCaptureAccess() };
    (accessibility, screen_recording)
}
