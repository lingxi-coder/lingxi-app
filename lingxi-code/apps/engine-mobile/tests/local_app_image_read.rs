//! Phase 1a — the annotation pipeline hands the agent a cropped JPEG BY PATH,
//! so the mobile Read tool must decode images. Without the `image-read`
//! feature it falls to the NUL scan and returns `format_binary`: the tool call
//! succeeds and the agent sees no picture.

use std::path::Path;

/// Regression guard: verifies engine-mobile's OWN manifest declares `image-read`.
///
/// This test is immune to workspace feature unification. When CI runs
/// `cargo test --workspace`, engine-desktop requests tool-file with
/// `features = ["image-read"]` making the feature available globally,
/// which would hide the bug if we relied on build-time const checks alone.
/// Asserting the manifest itself is the only gate that actually fails when
/// the feature is stripped from engine-mobile's dependency.
#[test]
fn engine_mobile_manifest_declares_tool_file_image_read() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest_path = Path::new(manifest_dir).join("Cargo.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path).expect(
        "Failed to read engine-mobile Cargo.toml; test must run via `cargo test -p engine-mobile`"
    );

    let tool_file_line = manifest_text
        .lines()
        .find(|line| line.trim().starts_with("tool-file"))
        .expect("tool-file dependency not found in engine-mobile Cargo.toml");

    assert!(
        tool_file_line.contains("image-read"),
        "engine-mobile's tool-file dependency is missing `features = [\"image-read\"]`. \
         Without it, the annotation pipeline falls to the NUL scan and Read returns \
         the binary notice instead of a picture — a silent failure where the call \
         succeeds and the agent sees nothing. Workspace feature unification from \
         engine-desktop will hide this under `cargo test --workspace`, so the \
         manifest assertion is the only gate that catches the regression.\n\
         \n\
         Found: {}\n\
         Expected to contain: image-read",
        tool_file_line
    );
}

/// Documentation test: verifies the build has the feature available.
///
/// This test documents the build-time state but does NOT protect against
/// regression; see `engine_mobile_manifest_declares_tool_file_image_read` for
/// the actual guard. Under workspace builds, engine-desktop's feature
/// declaration will make this pass even if engine-mobile strips its own.
#[test]
fn engine_mobile_builds_tool_file_with_image_read() {
    assert!(
        tool_file::IMAGE_READ_ENABLED,
        "engine-mobile depends on tool-file without the `image-read` feature, \
         so Read on an annotation .jpg returns the binary notice instead of an \
         image. Add features = [\"image-read\"] to the tool-file dependency."
    );
}
