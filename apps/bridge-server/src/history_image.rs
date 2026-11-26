use base64::{Engine, engine::general_purpose::STANDARD};
use client::protocol::message::MessageDto;
use lingxi_core::types::utf16_json::{JsonValueView, Utf16JsonProjection};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

const PREFIX: &str = "lingxi-history-image:";
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub(super) const MAX_IMAGE_OUTPUT: usize = 28 * 1024 * 1024;
const THUMBNAIL_EDGE: u32 = 320;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ImageSize {
    Thumbnail,
    Original,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageRequest {
    reference: String,
    size: ImageSize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImagePayload {
    media_type: String,
    base64: String,
}

fn image_ref(message: uuid::Uuid, index: usize, media_type: &str) -> Option<String> {
    let suffix = match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    Some(format!("{PREFIX}{message}:{index}:{suffix}"))
}

fn parse_ref(reference: &str) -> Option<(uuid::Uuid, usize, &'static str)> {
    let mut parts = reference.strip_prefix(PREFIX)?.split(':');
    let message = uuid::Uuid::parse_str(parts.next()?).ok()?;
    let index = parts.next()?.parse::<usize>().ok()?;
    let media_type = match parts.next()? {
        "png" => "image/png",
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    (parts.next().is_none() && index < 32).then_some((message, index, media_type))
}

/// The image reference belongs to the durable message, so it is unchanged by
/// repartitioning or compacting its transcript page.
pub(crate) fn reference_images(message: &mut MessageDto) {
    let id = message.transcript_uuid.as_deref().and_then(|id| uuid::Uuid::parse_str(id).ok());
    for (index, image) in message.images.iter_mut().enumerate() {
        if let Some((_, _, media_type)) = parse_ref(&image.url) {
            image.media_type = media_type.into();
        } else if image.url.starts_with("data:") {
            if let Some(reference) = id.and_then(|id| image_ref(id, index, &image.media_type)) {
                image.url = reference;
            }
        }
    }
}

/// Replace embedded binary strings before retaining a page of routed rows.
/// Exact UTF-16 transcript text stays in the native projection.
pub(super) fn project_images(row: &mut session::jsonl::JsonlMessage) -> Result<bool, String> {
    let Some(id) = uuid::Uuid::parse_str(&row.uuid).ok() else { return Ok(false) };
    if row.message_type != "user" {
        return Ok(false);
    }
    let mut changed = false;
    let replacements: Vec<_> = image_sources(&row.message).enumerate().filter_map(|(index, (pointer, source))| {
        if source.get_property("type")?.as_str()? != "base64" { return None; }
        let reference = image_ref(id, index, source.get_property("media_type")?.as_str()?)?;
        Some((pointer, reference))
    }).collect();
    for (pointer, reference) in replacements {
        let projected = Utf16JsonProjection::from_serde_value(&serde_json::json!({
            "type": "url",
            "url": reference,
        })).map_err(|error| error.to_string())?;
        row.message.replace_pointer_projection(&pointer, projected).map_err(|error| error.to_string())?;
        changed = true;
    }
    Ok(changed)
}

fn image_sources(message: &Utf16JsonProjection) -> impl Iterator<Item = (String, JsonValueView<'_>)> + '_ {
    (0..message.array_len("/content").unwrap_or_default()).filter_map(|index| {
        let prefix = format!("/content/{index}");
        if message.string_at(&format!("{prefix}/type")) != Some("image") {
            return None;
        }
        let pointer = format!("{prefix}/source");
        let source = message.at_pointer(&pointer)?;
        Some((pointer, source))
    })
}

pub(super) fn read(
    source: super::history_page::LatestSource,
    request: ImageRequest,
) -> Result<ImagePayload, String> {
    let (id, index, media_type) = parse_ref(&request.reference).ok_or("Invalid history image reference")?;
    let row = source.read_message(id)?;
    let (_, image) = image_sources(&row.message).nth(index).ok_or("History image is unavailable")?;
    if image.get_property("type").and_then(|value| value.as_str()) != Some("base64")
        || image.get_property("media_type").and_then(|value| value.as_str()) != Some(media_type)
    {
        return Err("History image has changed".into());
    }
    let data = image.get_property("data").and_then(|value| value.as_str())
        .filter(|data| !data.is_empty() && data.len() <= MAX_IMAGE_OUTPUT - 4096)
        .ok_or("History image exceeds the attachment size limit")?;
    let bytes = STANDARD.decode(data).map_err(|_| "Invalid history image")?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("History image exceeds the attachment size limit".into());
    }
    if matches!(request.size, ImageSize::Original) {
        return Ok(ImagePayload { media_type: media_type.into(), base64: data.into() });
    }
    let format = image::ImageFormat::from_mime_type(media_type).ok_or("Invalid history image type")?;
    let mut image = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    image.limits(limits);
    let thumbnail = image.decode().map_err(|_| "Unable to decode history image")?
        .thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
    let mut output = Cursor::new(Vec::new());
    thumbnail.write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| "Unable to encode history thumbnail")?;
    Ok(ImagePayload { media_type: "image/png".into(), base64: STANDARD.encode(output.into_inner()) })
}
