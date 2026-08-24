//! Phase 1a — the annotation pipeline hands the agent a cropped JPEG BY PATH,
//! so the mobile Read tool must decode images. Without the `image-read`
//! feature it falls to the NUL scan and returns `format_binary`: the tool call
//! succeeds and the agent sees no picture.

#[test]
fn engine_mobile_builds_tool_file_with_image_read() {
    assert!(
        tool_file::IMAGE_READ_ENABLED,
        "engine-mobile depends on tool-file without the `image-read` feature, \
         so Read on an annotation .jpg returns the binary notice instead of an \
         image. Add features = [\"image-read\"] to the tool-file dependency."
    );
}
