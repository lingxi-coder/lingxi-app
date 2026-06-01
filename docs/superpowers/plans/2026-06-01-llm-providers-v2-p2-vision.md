# LLM Providers v2 — P2: Vision (Image Input) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add image input across Anthropic / OpenAI / Gemini by introducing a canonical `ContentBlock::Image` block, encoding it per provider, gating it on a vision capability, and (best-effort) wiring the TUI paste path into it.

**Architecture:** `protocol::ContentBlock` gains an additive `Image { source: ImageSource }` variant whose serde shape *equals* Anthropic's wire (`{"type":"image","source":{…}}`). Because `api-client` serializes `Vec<ConversationMessage>` directly via serde (`anthropic.rs:225`), Anthropic image encoding is automatic. The OpenAI codec switches user `content` to the parts-array form when an image is present; the Gemini codec emits an `inlineData`/`fileData` part. A fail-fast guardrail in `ProviderApiAdapter` rejects images sent to a non-vision model.

**Tech Stack:** Rust 1.82.0, serde, serde_json. Run all cargo from `lingxi-code/`.

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-v2-design.md` §3.1. Branch `llm-providers-v2` (P1 complete, tag `llm-v2-p1`).

**Blast radius (verified by recon — the P1 lesson: grep the whole workspace).** Adding the `ContentBlock` variant breaks exactly **6 exhaustive matches**: `protocol/src/message_size.rs`, `engine/src/prompt.rs`, and the 4 codec helpers (OpenAI/Gemini `encode_user`/`encode_assistant`). All other references are constructions, `if let`, `matches!`, `_`-arm matches, or matches on the *separate* `api_client::ContentBlockApi` wire type — **0 test sites break.** Tasks below cover all 6.

**Parity gate:** image-free conversations stay byte-identical (OpenAI text-only user messages keep the string `content` form; Anthropic/Gemini unchanged). The existing `test-harness` parity suite must stay green. Do NOT modify the frozen `traits/` crate.

---

## File Structure

| File | Change |
|---|---|
| `lingxi-code/protocol/src/messages.rs` | Add `ContentBlock::Image { source: ImageSource }` + `ImageSource` enum |
| `lingxi-code/protocol/src/lib.rs` | Re-export `ImageSource` (if exports are explicit) |
| `lingxi-code/protocol/src/message_size.rs` | `content_block_size`: `Image` arm (byte size) |
| `lingxi-code/engine/src/prompt.rs` | `content_blocks_to_api`: `Image` arm (image JSON) |
| `lingxi-code/providers/src/openai/encode.rs` | `encode_user` parts-array when image present + `openai_image_url` helper; `encode_assistant` drops `Image` |
| `lingxi-code/providers/src/gemini/encode.rs` | `encode_user` `inlineData`/`fileData` part; `encode_assistant` drops `Image` |
| `lingxi-code/providers/src/capabilities.rs` | `openai()` / `gemini()` `vision: true` |
| `lingxi-code/orchestrator/src/provider_adapter.rs` | Fail-fast vision guardrail in `messages_create` + `stream` |
| `lingxi-code/tui/...` | (Task 6, trace-first) paste → `ContentBlock::Image` on submit, or documented fallback |

---

## Task 1: Canonical `ContentBlock::Image` + `ImageSource` (protocol)

**Files:**
- Modify: `lingxi-code/protocol/src/messages.rs`
- Modify: `lingxi-code/protocol/src/lib.rs` (re-export, if explicit)
- Modify: `lingxi-code/protocol/src/message_size.rs`

- [ ] **Step 1: Add the `ImageSource` enum + `Image` variant in `messages.rs`**

In `lingxi-code/protocol/src/messages.rs`, add the `Image` variant to `ContentBlock` (after the `Thinking` variant, before the closing `}`):

```rust
    /// An image input (vision). Serializes to the Anthropic image-block wire
    /// shape; the OpenAI/Gemini codecs translate it to their native forms.
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
```

And add this enum immediately after the `ContentBlock` enum definition:

```rust
/// Source of a [`ContentBlock::Image`]. Serializes to Anthropic's
/// `source` wire shape (`{"type":"base64",…}` / `{"type":"url",…}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Inline base64-encoded image bytes.
    Base64 {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Base64-encoded image bytes (no `data:` prefix).
        data: String,
    },
    /// A remote image URL the provider fetches.
    Url {
        /// The image URL.
        url: String,
    },
}
```

- [ ] **Step 2: Re-export `ImageSource` from the crate root**

Check `lingxi-code/protocol/src/lib.rs`. If it re-exports message types explicitly (e.g. `pub use messages::{ContentBlock, ConversationMessage, …};`), add `ImageSource` to that list. If it uses `pub use messages::*;`, no change is needed. The goal: `protocol::ImageSource` must resolve (the codecs import it).

- [ ] **Step 3: Add the `Image` arm to `content_block_size` in `message_size.rs`**

In `lingxi-code/protocol/src/message_size.rs`, add this arm to the `content_block_size` match (after the `Thinking` arm):

```rust
        ContentBlock::Image { source } => match source {
            crate::ImageSource::Base64 { data, .. } => data.len() as u64,
            crate::ImageSource::Url { url } => url.len() as u64,
        },
```

- [ ] **Step 4: Add serde + size lock tests**

In `messages.rs`'s `#[cfg(test)] mod tests` (create one if absent; it already imports the types), add:

```rust
    #[test]
    fn image_base64_block_matches_anthropic_wire() {
        let block = ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}
            })
        );
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn image_url_block_matches_anthropic_wire() {
        let block = ContentBlock::Image {
            source: ImageSource::Url { url: "https://x/y.png".to_string() },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type":"image","source":{"type":"url","url":"https://x/y.png"}})
        );
    }

    #[test]
    fn conversation_message_with_image_roundtrips_jsonl() {
        let m = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/jpeg".to_string(),
                    data: "Zm9v".to_string(),
                },
            }],
        };
        let line = serde_json::to_string(&m).unwrap();
        let back: ConversationMessage = serde_json::from_str(&line).unwrap();
        assert_eq!(back, m);
    }
```

(If `messages.rs` has no test module, add `#[cfg(test)] mod tests { use super::*; use crate::MessageId; ... }`. Confirm the `MessageId`/`ImageSource`/`ContentBlock`/`ConversationMessage` imports resolve.)

In `message_size.rs`'s test module add:

```rust
    #[test]
    fn image_block_sized_by_base64_data_len() {
        let m = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: crate::ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YWJj".to_string(),
                },
            }],
        };
        assert_eq!(text_byte_size(&m), 4); // "YWJj".len()
    }
```

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p protocol
cargo clippy -p protocol --all-targets -- -D warnings
```
Expected: all pass; the 3 serde tests + size test green.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/protocol/src/messages.rs lingxi-code/protocol/src/lib.rs lingxi-code/protocol/src/message_size.rs
git commit -m "feat(llm-v2 P2): add canonical ContentBlock::Image + ImageSource (Anthropic-wire serde)"
```

---

## Task 2: `engine/prompt.rs` image arm

**Files:** Modify `lingxi-code/engine/src/prompt.rs`.

The `content_blocks_to_api` match is exhaustive and now fails to compile. Add the `Image` arm.

- [ ] **Step 1: Add the `Image` arm**

In `content_blocks_to_api` (inside the `.map(|b| match b { … })`), add after the `Thinking` arm:

```rust
            ContentBlock::Image { source } => {
                json!({"type": "image", "source": source})
            }
```

(`source` is `&ImageSource`, which `Serialize`s to `{"type":"base64",…}` / `{"type":"url",…}` — matching Anthropic.)

- [ ] **Step 2: Add a test**

In `prompt.rs`'s `#[cfg(test)] mod tests`, add (it can call the private `content_blocks_to_api` since it's in the same module):

```rust
    #[test]
    fn image_block_encodes_to_anthropic_image_shape() {
        use protocol::{ContentBlock, ImageSource};
        let v = content_blocks_to_api(&[ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "YQ==".to_string(),
            },
        }]);
        assert_eq!(v[0]["type"], "image");
        assert_eq!(v[0]["source"]["type"], "base64");
        assert_eq!(v[0]["source"]["media_type"], "image/png");
        assert_eq!(v[0]["source"]["data"], "YQ==");
    }
```

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p engine prompt
cargo clippy -p engine --all-targets -- -D warnings
```
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/engine/src/prompt.rs
git commit -m "feat(llm-v2 P2): encode ContentBlock::Image in engine prompt assembler"
```

---

## Task 3: OpenAI codec — image_url parts

**Files:** Modify `lingxi-code/providers/src/openai/encode.rs`.

OpenAI carries images as content-*parts* (`{"type":"image_url","image_url":{"url":…}}`). A user message with images must switch `content` from a string to a parts array; **text-only messages keep the string form (byte-identical to today)**.

- [ ] **Step 1: Import `ImageSource`**

Change the import at the top of `encode.rs`:
```rust
use protocol::{ContentBlock, ConversationMessage, ImageSource};
```

- [ ] **Step 2: Replace `encode_user` with the image-aware version**

Replace the entire `encode_user` fn with:

```rust
/// User content: text + images → one `user` message (string `content` when
/// text-only, else an array of `text`/`image_url` parts); tool results → `tool`
/// messages (each answers one `tool_call_id`).
fn encode_user(content: &[ContentBlock], out: &mut Vec<Value>) {
    let mut text = String::new();
    let mut images: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::Image { source } => {
                images.push(json!({
                    "type": "image_url",
                    "image_url": {"url": openai_image_url(source)},
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content: result,
                is_error,
            } => {
                // OpenAI has no is_error flag; prefix on error so the model sees it.
                let body = if *is_error {
                    format!("[error] {result}")
                } else {
                    result.clone()
                };
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id.as_uuid().to_string(),
                    "content": body,
                }));
            }
            // Thinking has no OpenAI equivalent; ToolUse in a user message is malformed — drop both.
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if images.is_empty() {
        if !text.is_empty() {
            out.push(json!({"role": "user", "content": text}));
        }
    } else {
        let mut parts: Vec<Value> = Vec::new();
        if !text.is_empty() {
            parts.push(json!({"type": "text", "text": text}));
        }
        parts.extend(images);
        out.push(json!({"role": "user", "content": Value::Array(parts)}));
    }
}

/// Build an OpenAI `image_url.url` from a canonical image source: base64 → a
/// `data:` URL; a remote URL passes through unchanged.
fn openai_image_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
    }
}
```

- [ ] **Step 3: Add `Image` to `encode_assistant`'s drop arm**

In `encode_assistant`, change the drop arm from:
```rust
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
```
to:
```rust
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => {}
```

- [ ] **Step 4: Add tests**

In `encode.rs`'s test module add (the module imports `protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId}` — add `ImageSource`):

```rust
    #[test]
    fn text_only_user_stays_string_content() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![user_text("hi")];
        let body = encode_chat_body(&req);
        assert_eq!(body["messages"][0]["content"], "hi"); // string, not array
    }

    #[test]
    fn user_image_emits_parts_array_with_data_url() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text { text: "look".to_string() },
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: "YWJj".to_string(),
                    },
                },
            ],
        }];
        let body = encode_chat_body(&req);
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "look");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,YWJj");
    }

    #[test]
    fn user_image_url_passes_through() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url { url: "https://x/y.png".to_string() },
            }],
        }];
        let body = encode_chat_body(&req);
        assert_eq!(
            body["messages"][0]["content"][0]["image_url"]["url"],
            "https://x/y.png"
        );
    }
```

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p providers openai
cargo clippy -p providers --all-targets -- -D warnings
```
Expected: pass (incl. the existing `system_prompt_becomes_first_message` etc., which prove text-only stays a string).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/providers/src/openai/encode.rs
git commit -m "feat(llm-v2 P2): OpenAI codec encodes images as image_url content parts"
```

---

## Task 4: Gemini codec — inlineData / fileData

**Files:** Modify `lingxi-code/providers/src/gemini/encode.rs`.

- [ ] **Step 1: Import `ImageSource`**

Change the top import:
```rust
use protocol::{ContentBlock, ConversationMessage, ImageSource};
```

- [ ] **Step 2: Add the `Image` arm in `encode_user`**

In `encode_user`, add this arm (before the `Thinking | ToolUse` drop arm):

```rust
            ContentBlock::Image { source } => match source {
                ImageSource::Base64 { media_type, data } => {
                    parts.push(json!({"inlineData": {"mimeType": media_type, "data": data}}));
                }
                ImageSource::Url { url } => {
                    parts.push(json!({"fileData": {"fileUri": url}}));
                }
            },
```

(Base64 → `inlineData` is the fully-supported path. `fileData` for a remote URL omits `mimeType` — a documented limitation; base64 is the recommended path for Gemini.)

- [ ] **Step 3: Add `Image` to `encode_assistant`'s drop arm**

Change:
```rust
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
```
to:
```rust
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => {}
```

- [ ] **Step 4: Add a test**

In the test module (imports `protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId}` — add `ImageSource`):

```rust
    #[test]
    fn user_image_emits_inline_data_part() {
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YWJj".to_string(),
                },
            }],
        }];
        let body = encode_generate_body(&req);
        let part = &body["contents"][0]["parts"][0];
        assert_eq!(part["inlineData"]["mimeType"], "image/png");
        assert_eq!(part["inlineData"]["data"], "YWJj");
    }
```

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p providers gemini
cargo clippy -p providers --all-targets -- -D warnings
```
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/providers/src/gemini/encode.rs
git commit -m "feat(llm-v2 P2): Gemini codec encodes images as inlineData/fileData parts"
```

---

## Task 5: Capabilities + fail-fast vision guardrail

**Files:**
- Modify: `lingxi-code/providers/src/capabilities.rs`
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`

- [ ] **Step 1: Flip `vision: true` for the OpenAI + Gemini capability defaults**

In `capabilities.rs`, in `openai()` change `vision: false,` → `vision: true,`, and update its doc comment to "text + image + native tools". In `gemini()` change `vision: false,` → `vision: true,` and update its doc comment likewise. (`anthropic()` already has `vision: true`.)

- [ ] **Step 2: Add a capability assertion test**

In `capabilities.rs`'s test module add:
```rust
    #[test]
    fn openai_and_gemini_support_vision() {
        assert!(Capabilities::openai().vision);
        assert!(Capabilities::gemini().vision);
    }
```

- [ ] **Step 3: Add the guardrail helper + checks in `provider_adapter.rs`**

In `provider_adapter.rs`, extend the import:
```rust
use protocol::{ContentBlock, ConversationMessage};
```
Add this free function above `impl OrchestratorApiClient`:
```rust
/// Whether any message carries an image content block.
fn messages_contain_image(msgs: &[ConversationMessage]) -> bool {
    msgs.iter().any(|m| match m {
        ConversationMessage::User { content, .. } | ConversationMessage::Assistant { content, .. } => {
            content.iter().any(|b| matches!(b, ContentBlock::Image { .. }))
        }
        ConversationMessage::System { .. } => false,
    })
}
```
In `messages_create`, after `let resolved = self.router.resolve(model)?;` add:
```rust
        if messages_contain_image(&msgs) && !resolved.provider.capabilities().vision {
            return Err(ApiError::Http(traits::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support image input; \
                 select a vision-capable model or remove images",
                resolved.provider.id()
            ))));
        }
```
In `stream`, after `let resolved = self.router.resolve(model)?;` (and alongside the existing tools guardrail) add the same block but referencing `&messages`:
```rust
        if messages_contain_image(&messages) && !resolved.provider.capabilities().vision {
            return Err(ApiError::Http(traits::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support image input; \
                 select a vision-capable model or remove images",
                resolved.provider.id()
            ))));
        }
```

- [ ] **Step 4: Add guardrail tests**

In `provider_adapter.rs`'s test module add (reuse the `NoToolsProvider`/`FixedRouter` pattern; `NoToolsProvider` already has `vision: false`):

```rust
    #[tokio::test]
    async fn image_to_non_vision_model_fails_fast() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YWJj".to_string(),
                },
            }],
        }];
        let result = adapter.messages_create("custom/x", None, msgs).await;
        assert!(result.is_err(), "image to a non-vision model must fail fast");
        assert!(matches!(
            result,
            Err(ApiError::Http(traits::HttpError::InvalidRequest(_)))
        ));
    }

    #[tokio::test]
    async fn image_to_vision_model_is_allowed() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        // StubProvider has vision via Capabilities::anthropic() (vision: true).
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url { url: "https://x/y.png".to_string() },
            }],
        }];
        adapter.messages_create("anthropic/claude", None, msgs).await.expect("vision model accepts image");
    }
```

- [ ] **Step 5: Run tests + clippy**

```bash
cargo test -p providers capabilities
cargo test -p orchestrator provider_adapter
cargo clippy -p providers --all-targets -- -D warnings
cargo clippy -p orchestrator --all-targets -- -D warnings
```
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/providers/src/capabilities.rs lingxi-code/orchestrator/src/provider_adapter.rs
git commit -m "feat(llm-v2 P2): vision capability for OpenAI/Gemini + fail-fast image guardrail"
```

---

## Task 6: TUI paste → image (trace-first; wire or document)

**Files:** TBD by trace — under `lingxi-code/tui/src/` (start at `root.rs` `apply_paste_to_prompt` / the submit handler, and `components/prompt_input.rs` `apply_paste_block` / `PasteCoalescer` / paste state).

The M7-10 paste coalescer turns pasted image lines into `[Image #N]` placeholders and records them in a `paste` state. This task connects that to `ContentBlock::Image` on submit. Its exact shape depends on a trace, so it is **trace-first with a documented fallback** (sanctioned by spec §3.1).

- [ ] **Step 1: Trace the paste→submit→message path**

Read `components/prompt_input.rs` (`apply_paste_block`, the paste/`PasteState` type, `PasteCoalescer`) and `root.rs` (`apply_paste_to_prompt`, and the submit handler that builds the outgoing `ConversationMessage::User`). Determine: when the user submits, are the recorded pasted-image **bytes + media type** available at the point the user message is constructed? Write a 3-5 line finding (paste-state shape, submit site `file:line`, whether image bytes survive to submit).

- [ ] **Step 2a: IF image bytes are available at submit — wire them**

Build the user message's `content` as `Vec<ContentBlock>`: text → `ContentBlock::Text`, each recorded pasted image → `ContentBlock::Image { source: ImageSource::Base64 { media_type, data } }` (base64-encode the bytes; infer `media_type` from the paste record or default `image/png`). Add a focused unit test on the pure conversion fn (paste state + text → `Vec<ContentBlock>` containing the expected `Image` block). Keep text-only submits byte-identical (single `Text` block as today).

- [ ] **Step 2b: ELSE (bytes not retained to submit) — document + defer**

If the M7-10 path keeps only `[Image #N]` placeholder text (no bytes), do NOT fabricate data. Instead: (1) add a short note to `docs/LLM_PROVIDERS.md` under a "Vision" heading stating images are supported via the API/programmatic path and that TUI paste-to-image wiring is a follow-up; (2) leave the placeholder text behavior unchanged (no regression). Record the finding in the commit message.

- [ ] **Step 3: Run the relevant gate**

If code changed (2a): `cargo test -p tui <new test>` and `cargo clippy -p tui --all-targets -- -D warnings`. If docs-only (2b): no code gate.

- [ ] **Step 4: Commit**

```bash
git add -A lingxi-code/tui docs/LLM_PROVIDERS.md
git commit -m "feat(llm-v2 P2): wire TUI pasted images to ContentBlock::Image"   # 2a
# or, for 2b:
git commit -m "docs(llm-v2 P2): vision via API path; TUI paste-to-image deferred (trace finding)"
```

---

## Task 7: Phase gates + tag

**Files:** none (verification only).

- [ ] **Step 1: Provider + protocol + engine + orchestrator tests**

```bash
cargo test -p protocol -p engine -p providers -p orchestrator
```
Expected: all pass.

- [ ] **Step 2: Parity suite (image-free byte-locks stay green)**

```bash
cargo test -p test-harness
```
Expected: 0 failures.

- [ ] **Step 3: Workspace build**

```bash
cargo build --workspace
```
Expected: `Finished`.

- [ ] **Step 4: Per-crate clippy on touched crates**

```bash
cargo clippy -p protocol -p engine -p providers -p orchestrator --all-targets -- -D warnings
```
Expected: clean. (Note: a workspace-wide `clippy --all-targets` trips a pre-existing `doc_markdown` lint in the frozen `traits` crate — out of scope; do NOT modify `traits`.)

- [ ] **Step 5: Dependency-graph gate**

```bash
bash scripts/check-deps.sh
```
Expected: `OK — 73 workspace crates` (P2 adds no deps).

- [ ] **Step 6: Tag**

```bash
git tag -a llm-v2-p2 -m "LLM Providers v2 P2: vision (image input)"
```

---

## Self-Review

**Spec coverage (§3.1):** `ContentBlock::Image`+`ImageSource` (Task 1) ✓; Anthropic encode = free via serde, locked by Task 1 serde test ✓; OpenAI `image_url` parts (Task 3) ✓; Gemini `inlineData`/`fileData` (Task 4) ✓; `Capabilities.vision` (Task 5) ✓; fail-fast guardrail (Task 5) ✓; session JSONL round-trip (Task 1 `conversation_message_with_image_roundtrips_jsonl`) ✓; M7-10 paste trace + wire/fallback (Task 6) ✓; the `engine/prompt.rs` break site (Task 2) ✓; `message_size` break site (Task 1) ✓.

**Placeholder scan:** Task 6 is intentionally trace-first per spec, but its branches (2a/2b) are concrete with deliverables — not a vague placeholder. All other steps have full code + exact commands.

**Type consistency:** `ImageSource::{Base64{media_type,data}, Url{url}}` used identically in protocol (def), message_size, openai (`openai_image_url`), gemini, and all tests. `ContentBlock::Image { source }` field name `source` consistent across the protocol def, message_size, prompt, both codecs, the guardrail `matches!`, and tests. The OpenAI text-only→string / image→parts-array split preserves the existing string-content tests.
