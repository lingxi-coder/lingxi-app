//! Pure MCP tool-result content transform — the dependency-free cases.
//!
//! Ports the JSON-reshaping half of claude-code's `transformResultContent` /
//! `transformMCPResult` (`services/mcp/client.ts:2478-2706`). For each content
//! block an MCP tool returns, that TS switch converts the raw server block into
//! the model-facing form. This module implements ONLY the cases that need no
//! image/codec dependency and no disk side effects (so the function stays
//! pure):
//!
//! - `text`          → the text string, as a `{type:"text", text}` block
//!   (passthrough; `client.ts:2483-2489`).
//! - `resource` with `text` → a `{type:"text"}` block prefixed
//!   `[Resource from <server> at <uri>] ` (`client.ts:2528-2534`).
//! - `resource_link` → a `{type:"text"}` block
//!   `[Resource link: <name>] <uri>` plus optional ` (<description>)`
//!   (`client.ts:2575-2587`).
//!
//! DEFERRED (left as a verbatim passthrough of the original block):
//! - `image` (`client.ts:2503-2523`) — `maybeResizeAndDownsampleImageBuffer`
//!   needs an image codec dependency.
//! - `audio` (`client.ts:2490-2502`) — base64-decode + `persistBlobToTextBlock`
//!   writes the bytes to disk.
//! - `resource` with `blob` (`client.ts:2535-2572`) — the image sub-case needs
//!   the same codec; the non-image sub-case routes through
//!   `persistBlobToTextBlock`, which decodes base64 and writes the bytes to
//!   disk under a NON-DETERMINISTIC `persistId` (`Date.now()` + `Math.random()`,
//!   `client.ts:2604`). Disk I/O and a random id can't live in a pure Value
//!   transform, so both are deferred alongside `image`/`audio`.
//!
//! Transform scope mirrors TS exactly: `transformMCPResult` only walks
//! `result.content` when it is an ARRAY (`client.ts:2686`). A non-array
//! `content` (e.g. a bare `toolResult` string) is left untouched here so the
//! caller forwards it verbatim, matching the TS branches that never reach
//! `transformResultContent`.

use serde_json::{json, Value};

/// Transform an MCP tool-result `content` Value into its model-facing form.
///
/// When `content` is a JSON array, each element is run through
/// [`transform_block`] and the (flattened) results are collected — mirroring
/// `transformMCPResult`'s `result.content.map(transformResultContent).flat()`
/// (`client.ts:2686-2697`). Any other Value (bare string, object, etc.) is
/// returned unchanged, because the TS only reshapes the array form.
#[must_use]
pub fn transform_result_content(content: &Value, server_name: &str) -> Value {
    let Some(items) = content.as_array() else {
        return content.clone();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.extend(transform_block(item, server_name));
    }
    Value::Array(out)
}

/// Transform ONE content block, returning the model-facing block(s).
///
/// Faithful to `transformResultContent`'s `switch (resultContent.type)`
/// (`client.ts:2482-2590`):
/// - `text` → `[{type:"text", text}]`
/// - `resource` with `text` → `[{type:"text", text: "<prefix><text>"}]`
/// - `resource_link` → `[{type:"text", text: "<link form>"}]`
/// - `image` / `audio` / `resource`-with-`blob` (DEFERRED) → the original block
///   verbatim (a single-element vec), so the caller forwards it unchanged.
/// - any unrecognized block → the original block verbatim (the TS `default`
///   returns `[]`, but here we preserve the block so deferred/unknown shapes
///   survive the round-trip rather than vanishing).
fn transform_block(block: &Value, server_name: &str) -> Vec<Value> {
    let kind = block.get("type").and_then(Value::as_str);
    match kind {
        // case 'text': return [{ type:'text', text: resultContent.text }]
        Some("text") => {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
            vec![text_block(text)]
        }
        // case 'resource': { ... 'text' in resource → prefixed text block ... }
        Some("resource") => transform_resource(block, server_name),
        // case 'resource_link': { let text = `[Resource link: ...] ...` }
        Some("resource_link") => {
            vec![text_block(&resource_link_text(block))]
        }
        // DEFERRED ('image', 'audio') and any other/unknown block: pass the
        // original block through verbatim (see fn-doc for why TS `default`'s
        // `[]` is intentionally not mirrored).
        _ => vec![block.clone()],
    }
}

/// Transform a `resource` block. Only the `text` sub-case is reshaped here;
/// the `blob` sub-case is DEFERRED (persists to disk / needs a codec) and the
/// block is passed through verbatim. Mirrors `client.ts:2524-2573`.
fn transform_resource(block: &Value, server_name: &str) -> Vec<Value> {
    let Some(resource) = block.get("resource") else {
        // No `resource` field — not a shape TS produces; pass through.
        return vec![block.clone()];
    };
    let uri = resource.get("uri").and_then(Value::as_str).unwrap_or("");
    // prefix = `[Resource from ${serverName} at ${resource.uri}] `
    let prefix = format!("[Resource from {server_name} at {uri}] ");

    // if ('text' in resource) → `${prefix}${resource.text}`
    if let Some(text) = resource.get("text").and_then(Value::as_str) {
        return vec![text_block(&format!("{prefix}{text}"))];
    }
    // 'blob' in resource → DEFERRED (image codec or disk persistence). Anything
    // without `text` is passed through verbatim.
    vec![block.clone()]
}

/// Build the `resource_link` text form, byte-faithful to `client.ts:2576-2580`:
/// `[Resource link: ${name}] ${uri}` with an optional ` (${description})`
/// suffix when `description` is present.
fn resource_link_text(block: &Value) -> String {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
    let mut text = format!("[Resource link: {name}] {uri}");
    if let Some(description) = block.get("description").and_then(Value::as_str) {
        // TS appends only when truthy; an empty string is falsy in JS and is
        // therefore NOT appended.
        if !description.is_empty() {
            text.push_str(&format!(" ({description})"));
        }
    }
    text
}

/// A `{ "type": "text", "text": <text> }` content block.
fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_block_passthrough() {
        let content = json!([{ "type": "text", "text": "hello world" }]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([{ "type": "text", "text": "hello world" }]));
    }

    #[test]
    fn text_block_missing_text_defaults_empty() {
        // `resultContent.text` of `undefined` would stringify to "" via the
        // reshape; we default a missing/non-string `text` to "".
        let content = json!([{ "type": "text" }]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([{ "type": "text", "text": "" }]));
    }

    #[test]
    fn resource_with_text_is_prefixed() {
        let content = json!([{
            "type": "resource",
            "resource": { "uri": "file:///a.txt", "text": "BODY" }
        }]);
        let got = transform_result_content(&content, "myserver");
        assert_eq!(
            got,
            json!([{
                "type": "text",
                "text": "[Resource from myserver at file:///a.txt] BODY"
            }])
        );
    }

    #[test]
    fn resource_with_text_empty_uri() {
        // Missing uri → empty string in the prefix (matches `resource.uri`
        // being absent producing `at ] ` per the template literal).
        let content = json!([{
            "type": "resource",
            "resource": { "text": "X" }
        }]);
        let got = transform_result_content(&content, "s");
        assert_eq!(
            got,
            json!([{ "type": "text", "text": "[Resource from s at ] X" }])
        );
    }

    #[test]
    fn resource_link_without_description() {
        let content = json!([{
            "type": "resource_link",
            "name": "Docs",
            "uri": "https://example.com/docs"
        }]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(
            got,
            json!([{
                "type": "text",
                "text": "[Resource link: Docs] https://example.com/docs"
            }])
        );
    }

    #[test]
    fn resource_link_with_description() {
        let content = json!([{
            "type": "resource_link",
            "name": "Docs",
            "uri": "https://example.com/docs",
            "description": "the docs"
        }]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(
            got,
            json!([{
                "type": "text",
                "text": "[Resource link: Docs] https://example.com/docs (the docs)"
            }])
        );
    }

    #[test]
    fn resource_link_empty_description_not_appended() {
        // JS treats "" as falsy, so the ` (...)` suffix is omitted.
        let content = json!([{
            "type": "resource_link",
            "name": "N",
            "uri": "u",
            "description": ""
        }]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(
            got,
            json!([{ "type": "text", "text": "[Resource link: N] u" }])
        );
    }

    #[test]
    fn multiple_blocks_flattened_in_order() {
        let content = json!([
            { "type": "text", "text": "one" },
            { "type": "resource_link", "name": "L", "uri": "u" },
            { "type": "resource", "resource": { "uri": "r", "text": "two" } }
        ]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(
            got,
            json!([
                { "type": "text", "text": "one" },
                { "type": "text", "text": "[Resource link: L] u" },
                { "type": "text", "text": "[Resource from srv at r] two" }
            ])
        );
    }

    #[test]
    fn deferred_image_block_passthrough() {
        // The image case is DEFERRED — the block is forwarded verbatim.
        let img = json!({ "type": "image", "data": "AAAA", "mimeType": "image/png" });
        let content = json!([img.clone()]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([img]));
    }

    #[test]
    fn deferred_audio_block_passthrough() {
        let audio = json!({ "type": "audio", "data": "AAAA", "mimeType": "audio/mpeg" });
        let content = json!([audio.clone()]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([audio]));
    }

    #[test]
    fn deferred_resource_blob_passthrough() {
        // resource-with-blob is DEFERRED (disk persistence / codec) → verbatim.
        let block = json!({
            "type": "resource",
            "resource": { "uri": "r", "blob": "AAAA", "mimeType": "application/pdf" }
        });
        let content = json!([block.clone()]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([block]));
    }

    #[test]
    fn unknown_block_passthrough() {
        let block = json!({ "type": "future_thing", "x": 1 });
        let content = json!([block.clone()]);
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!([block]));
    }

    #[test]
    fn non_array_content_passthrough() {
        // TS only reshapes when `result.content` is an array; a bare string
        // (the locked `parity_mcp_invocation` fixture's mock returns "ok")
        // must survive untouched.
        let content = json!("ok");
        let got = transform_result_content(&content, "srv");
        assert_eq!(got, json!("ok"));

        let obj = json!({ "structuredContent": { "a": 1 } });
        assert_eq!(transform_result_content(&obj, "srv"), obj);
    }
}
