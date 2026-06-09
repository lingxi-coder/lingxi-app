# WebFetch HTML→markdown + Side-Query Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `WebFetchTool` convert fetched HTML to markdown and process it with a small-fast (Haiku-in-Anthropic-mode) model using the caller's prompt, matching claude-code's WebFetch behavior — instead of returning the raw HTTP body.

**Architecture:** The mature `tools/web/src/web_fetch.rs` already fetches, caches, preflight-checks, and truncates. The gap is at `web_fetch.rs:504-518` (it caches/returns the raw body; a comment notes "the markdown/Haiku conversion lands in later batches"). We add (a) an `htmd`-backed HTML→markdown step behind a `web-markdown` cargo feature, (b) a "secondary model" apply step via the existing `sidequery::SideQueryClient`, threaded into `WebFetchTool` through a **tool-level builder** (not a `BuiltinToolContext` field — that struct is constructed in `engine-mobile`, the forbidden zone), wired at the desktop composition root using the `side_query_client` it already builds (`engine-desktop/lib.rs:1282`). Feature OFF → byte-identical to today.

**Tech Stack:** Rust 1.82, `htmd` (HTML→markdown, ≈ claude-code's `turndown`), `sidequery` (one-shot LLM side queries), `protocol::ConversationMessage`, async `Tool` trait.

**Reference of truth:** `claude-code/src/tools/WebFetchTool/{utils.ts,prompt.ts}`. Functional parity (the final output is model-generated, non-deterministic).

---

## File Structure

- **Modify** `lingxi-code/sidequery/src/purposes.rs` — add `QuerySource::WebFetchApply` (COGS tag).
- **Modify** `lingxi-code/tools/web/Cargo.toml` — add the `web-markdown` feature + optional `htmd` dep + `sidequery` dep.
- **Create** `lingxi-code/tools/web/src/markdown.rs` — content-type detection, HTML→markdown (feature-gated), markdown truncation, the two guideline strings, and the secondary-model prompt builder.
- **Modify** `lingxi-code/tools/web/src/lib.rs` — declare `mod markdown`; thread an optional `SideQueryClient` into `register_all` and onto `WebFetchTool`.
- **Modify** `lingxi-code/tools/web/src/web_fetch.rs` — add the `side_query` field + `with_side_query` builder; in `call()`, convert HTML→markdown, cache the markdown, and run the apply step when a prompt + client are present.
- **Modify** `lingxi-code/apps/engine-desktop/src/lib.rs` — thread `web_side_query` through `register_desktop_tools` (`:359`/`:342`/`:1551`) using the `side_query_client` it already builds (`:1284`).
- **Modify** `lingxi-code/apps/engine-mobile/src/lib.rs` — pass `None` to `tool_web::register_all` (`:105`).

**Constants ported from claude-code** (`WebFetchTool/utils.ts`): `MAX_MARKDOWN_LENGTH = 100_000`. The truncation suffix reuses the existing `WEBFETCH_TRUNCATION_SUFFIX`.

---

## Task 1: Add `QuerySource::WebFetchApply`

**Files:**
- Modify: `lingxi-code/sidequery/src/purposes.rs`
- Test: `lingxi-code/sidequery/src/purposes.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add to the bottom of `purposes.rs` (create the `tests` module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::QuerySource;

    #[test]
    fn web_fetch_apply_roundtrips() {
        let q = QuerySource::WebFetchApply;
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, "\"WebFetchApply\"");
        let back: QuerySource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, QuerySource::WebFetchApply);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sidequery web_fetch_apply_roundtrips`
Expected: FAIL — `no variant named WebFetchApply`.

- [ ] **Step 3: Add the variant**

In `purposes.rs`, inside `pub enum QuerySource { ... }`, add before `Custom(String)`:

```rust
    /// WebFetch's secondary "apply" call: process fetched markdown with the
    /// caller's prompt via a small-fast model (claude-code `web_fetch_apply`).
    WebFetchApply,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p sidequery web_fetch_apply_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/sidequery/src/purposes.rs
git commit -m "feat(sidequery): add QuerySource::WebFetchApply COGS tag"
```

---

## Task 2: Add the `web-markdown` feature + deps

**Files:**
- Modify: `lingxi-code/tools/web/Cargo.toml`

- [ ] **Step 1: Add the dependency + feature**

In `[dependencies]`, add (after the existing `api-client` line):

```toml
# Side-query seam for the WebFetch "apply" step (process fetched markdown with
# a small-fast model). The concrete client is injected at the desktop root.
sidequery = { path = "../../sidequery" }
# HTML→markdown for the WebFetch apply step (the Rust analogue of claude-code's
# `turndown`). Optional + gated behind `web-markdown` so engine-mobile/minimal
# never resolves it.
htmd = { version = "0.1", optional = true }
```

Add a new `[features]` section (after `[dependencies]`, before `[dev-dependencies]`):

```toml
[features]
# Enables HTML→markdown conversion + the small-fast apply step. OFF by default
# so the mobile/minimal builds (which never enable it) don't pull `htmd`.
web-markdown = ["dep:htmd"]
```

- [ ] **Step 2: Verify it resolves (both feature states)**

Run: `cargo check -p tool-web && cargo check -p tool-web --features web-markdown`
Expected: both succeed. Pin the exact `htmd` version cargo selected:

Run: `cargo tree -p tool-web --features web-markdown -i htmd`
Then set `htmd = { version = "=<resolved>", optional = true }` in `Cargo.toml` (exact pin, MSRV 1.82).

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/tools/web/Cargo.toml lingxi-code/Cargo.lock
git commit -m "build(tool-web): add web-markdown feature (htmd) + sidequery dep"
```

---

## Task 3: `markdown.rs` — content-type detection + conversion + truncation

**Files:**
- Create: `lingxi-code/tools/web/src/markdown.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Create `markdown.rs` with ONLY the tests first:

```rust
//! HTML→markdown conversion + the secondary-model prompt for the WebFetch apply
//! step. Ports `claude-code/src/tools/WebFetchTool/{utils.ts,prompt.ts}`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_html_content_type() {
        assert!(is_html_content_type("text/html; charset=utf-8"));
        assert!(is_html_content_type("TEXT/HTML"));
        assert!(!is_html_content_type("text/markdown"));
        assert!(!is_html_content_type("application/json"));
        assert!(!is_html_content_type(""));
    }

    #[test]
    fn truncates_markdown_at_cap() {
        let big = "a".repeat(MAX_MARKDOWN_LENGTH + 10);
        let out = truncate_markdown(big);
        assert!(out.ends_with(crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX));
        let body = &out[..out.len() - crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX.len()];
        assert_eq!(body.len(), MAX_MARKDOWN_LENGTH);
    }

    #[test]
    fn does_not_truncate_short_markdown() {
        let s = "hello".to_string();
        assert_eq!(truncate_markdown(s.clone()), s);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tool-web --lib markdown::`
Expected: FAIL — `cannot find function is_html_content_type` / `MAX_MARKDOWN_LENGTH`.

- [ ] **Step 3: Implement the functions**

At the TOP of `markdown.rs` (above the test module), add:

```rust
use crate::web_fetch::WEBFETCH_TRUNCATION_SUFFIX;

/// Markdown is truncated to this many bytes before the secondary model, to avoid
/// "Prompt is too long" errors. claude-code `utils.ts` `MAX_MARKDOWN_LENGTH`.
pub const MAX_MARKDOWN_LENGTH: usize = 100_000;

/// True when the `Content-Type` header denotes HTML (case-insensitive prefix
/// match on `text/html`). Non-HTML bodies are used as-is (no conversion).
#[must_use]
pub fn is_html_content_type(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("text/html")
}

/// Truncate `markdown` to `MAX_MARKDOWN_LENGTH` bytes (char-boundary safe),
/// appending [`WEBFETCH_TRUNCATION_SUFFIX`] when truncated. Mirrors the
/// `markdownContent.length > MAX_MARKDOWN_LENGTH` slice in `utils.ts`.
#[must_use]
pub fn truncate_markdown(markdown: String) -> String {
    if markdown.len() <= MAX_MARKDOWN_LENGTH {
        return markdown;
    }
    let mut cut = MAX_MARKDOWN_LENGTH;
    while cut > 0 && !markdown.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = markdown[..cut].to_string();
    out.push_str(WEBFETCH_TRUNCATION_SUFFIX);
    out
}

/// Convert HTML to markdown via `htmd` (the `turndown` analogue). On conversion
/// error, fall back to the original HTML (claude-code falls back to raw content
/// when turndown throws). Only compiled under `web-markdown`.
#[cfg(feature = "web-markdown")]
#[must_use]
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}
```

Note: confirm the exact `htmd::convert` signature against the version pinned in Task 2; it returns `Result<String, _>`. If the pinned API differs (e.g. `HtmlToMarkdown::new().convert(html)`), adapt this one call.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tool-web --lib markdown::`
Expected: PASS (the `html_to_markdown` fn isn't tested here — it's behind the feature; covered in Task 6's integration test run with `--features web-markdown`).

- [ ] **Step 5: Wire the module + run with the feature**

In `lingxi-code/tools/web/src/lib.rs`, add near the other `mod` lines:

```rust
mod markdown;
```

Run: `cargo test -p tool-web --features web-markdown --lib markdown::`
Expected: PASS, and `html_to_markdown` compiles.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tools/web/src/markdown.rs lingxi-code/tools/web/src/lib.rs
git commit -m "feat(tool-web): markdown module — html detection, htmd conversion, truncation"
```

---

## Task 4: `markdown.rs` — secondary-model prompt (byte-faithful)

**Files:**
- Modify: `lingxi-code/tools/web/src/markdown.rs`

- [ ] **Step 1: Write the failing test**

Add to `markdown.rs`'s `tests` module:

```rust
    #[test]
    fn secondary_prompt_matches_template_strict() {
        let got = make_secondary_model_prompt("MD-HERE", "what is X?", false);
        let expected = "\nWeb page content:\n---\nMD-HERE\n---\n\nwhat is X?\n\n\
Provide a concise response based only on the content above. In your response:\n \
- Enforce a strict 125-character maximum for quotes from any source document. \
Open Source Software is ok as long as we respect the license.\n \
- Use quotation marks for exact language from articles; any language outside of \
the quotation should never be word-for-word the same.\n \
- You are not a lawyer and never comment on the legality of your own prompts and \
responses.\n - Never produce or reproduce exact song lyrics.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn secondary_prompt_preapproved_uses_short_guidelines() {
        let got = make_secondary_model_prompt("MD", "q", true);
        assert!(got.contains(
            "Provide a concise response based on the content above. Include relevant \
details, code examples, and documentation excerpts as needed."
        ));
        assert!(got.starts_with("\nWeb page content:\n---\nMD\n---\n\nq\n\n"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tool-web --lib markdown::secondary`
Expected: FAIL — `cannot find function make_secondary_model_prompt`.

- [ ] **Step 3: Implement the prompt builder**

Add above the test module in `markdown.rs` (port of `prompt.ts` `makeSecondaryModelPrompt`):

```rust
/// Guidelines appended for a NON-preapproved domain (the strict default).
/// Byte-faithful to `prompt.ts`.
const GUIDELINES_STRICT: &str = "Provide a concise response based only on the content above. In your response:\n \
- Enforce a strict 125-character maximum for quotes from any source document. \
Open Source Software is ok as long as we respect the license.\n \
- Use quotation marks for exact language from articles; any language outside of \
the quotation should never be word-for-word the same.\n \
- You are not a lawyer and never comment on the legality of your own prompts and \
responses.\n - Never produce or reproduce exact song lyrics.";

/// Guidelines appended for a preapproved domain. Byte-faithful to `prompt.ts`.
const GUIDELINES_PREAPPROVED: &str = "Provide a concise response based on the content above. Include relevant \
details, code examples, and documentation excerpts as needed.";

/// Build the secondary-model prompt. Byte-faithful to `prompt.ts`
/// `makeSecondaryModelPrompt` (note the leading/trailing newlines from the TS
/// template literal).
#[must_use]
pub fn make_secondary_model_prompt(
    markdown_content: &str,
    prompt: &str,
    is_preapproved_domain: bool,
) -> String {
    let guidelines = if is_preapproved_domain {
        GUIDELINES_PREAPPROVED
    } else {
        GUIDELINES_STRICT
    };
    format!("\nWeb page content:\n---\n{markdown_content}\n---\n\n{prompt}\n\n{guidelines}\n")
}

/// Whether `host` is on the WebFetch preapproved-domain allowlist. Ported as a
/// conservative default (`false` ⇒ strict guidelines) — porting the full
/// `preapproved.ts` list is a follow-up; the strict path is the safe default.
#[must_use]
pub fn is_preapproved_domain(_host: &str) -> bool {
    false
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tool-web --lib markdown::secondary`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tools/web/src/markdown.rs
git commit -m "feat(tool-web): port makeSecondaryModelPrompt + guideline strings"
```

---

## Task 5: `WebFetchTool` — side-query builder

**Files:**
- Modify: `lingxi-code/tools/web/src/web_fetch.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add to `web_fetch.rs`'s `tests` module:

```rust
    #[test]
    fn with_side_query_sets_the_client() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        assert!(tool.side_query.is_none(), "default has no side-query client");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tool-web --lib with_side_query_sets_the_client`
Expected: FAIL — `no field side_query on WebFetchTool`.

- [ ] **Step 3: Add the field + builder**

In `web_fetch.rs`, change the struct and `impl`:

```rust
pub struct WebFetchTool {
    ctx: BuiltinToolContext,
    /// Optional small-fast side-query client for the apply step. Wired only at
    /// the desktop composition root (None on mobile/minimal — see plan).
    side_query: Option<std::sync::Arc<dyn sidequery::SideQueryClient>>,
}

impl WebFetchTool {
    /// Construct a new tool (no apply step until [`Self::with_side_query`]).
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx, side_query: None }
    }

    /// Attach the side-query client that powers the secondary-model apply step.
    #[must_use]
    pub fn with_side_query(
        mut self,
        client: std::sync::Arc<dyn sidequery::SideQueryClient>,
    ) -> Self {
        self.side_query = Some(client);
        self
    }
```

(Leave the existing `user_agent`/`emit_*` methods unchanged inside the same `impl`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tool-web --lib with_side_query_sets_the_client`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tools/web/src/web_fetch.rs
git commit -m "feat(tool-web): WebFetchTool::with_side_query builder seam"
```

---

## Task 6: `call()` — convert to markdown, cache it, run the apply step

**Files:**
- Modify: `lingxi-code/tools/web/src/web_fetch.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test (mock side-query asserts the apply)**

Add to `web_fetch.rs`'s `tests` module:

```rust
    use sidequery::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};

    struct CapturingSideQuery {
        captured: std::sync::Mutex<Option<String>>,
        reply: String,
    }
    #[async_trait]
    impl SideQueryClient for CapturingSideQuery {
        async fn query(
            &self,
            request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            // `text_content()` concatenates the message's Text blocks (protocol).
            let user_text = request.messages.last().map(|m| m.text_content());
            *self.captured.lock().unwrap() = user_text;
            Ok(SideQueryResponse {
                text: Some(self.reply.clone()),
                structured: None,
                tool_calls: vec![],
                // cost::Usage derives Default; inferred from the field type so the
                // test needs no direct `cost` dependency.
                usage: Default::default(),
                stop_reason: Some("end_turn".into()),
            })
        }
    }

    #[tokio::test]
    async fn apply_step_runs_model_over_markdown() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: "<h1>Title</h1><p>Body text</p>".into(),
        }));
        let capture = std::sync::Arc::new(CapturingSideQuery {
            captured: std::sync::Mutex::new(None),
            reply: "MODEL SUMMARY".into(),
        });
        let tool = WebFetchTool::new(ctx).with_side_query(capture.clone());
        let res = tool
            .call(
                json!({ "url": "https://apply.example/x", "prompt": "summarize" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        // The tool returns the MODEL's answer, not the raw HTML/markdown.
        assert_eq!(res.data["content"], "MODEL SUMMARY");
        // The model saw markdown (h1 became `# Title`, not `<h1>`), plus the prompt.
        let seen = capture.captured.lock().unwrap().clone().unwrap();
        assert!(seen.contains("# Title"), "model prompt should carry markdown: {seen}");
        assert!(seen.contains("summarize"));
        assert!(!seen.contains("<h1>"), "HTML must be converted, not raw");
    }

    #[tokio::test]
    async fn no_side_query_returns_markdown_unchanged_behavior() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: "<h1>Hi</h1>".into(),
        }));
        // No .with_side_query → apply step is skipped; content is the markdown.
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://nomarkdown.example/x", "prompt": "q" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["content"], "# Hi");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tool-web --features web-markdown --lib apply_step_runs_model_over_markdown`
Expected: FAIL — content is the raw HTML, no apply step exists yet.

- [ ] **Step 3: Add the apply helper + markdown conversion to `call()`**

Add this method inside `impl WebFetchTool` (near `user_agent`):

```rust
    /// Resolve the small-fast model id for the apply step. Anthropic-family
    /// default ⇒ Haiku; otherwise fall back to the configured default model.
    /// (Fuller provider-aware resolver is a follow-up — see the design spec.)
    fn apply_model(&self) -> String {
        if self.ctx.default_model.contains("claude") {
            "claude-haiku-4-5".to_string()
        } else {
            self.ctx.default_model.clone()
        }
    }

    /// Run the secondary-model apply step over `markdown` with `prompt`. Returns
    /// the model's text (or the byte-faithful fallback `"No response from model"`).
    #[cfg(feature = "web-markdown")]
    async fn apply_prompt(
        &self,
        client: &std::sync::Arc<dyn sidequery::SideQueryClient>,
        host: &str,
        markdown: &str,
        prompt: &str,
    ) -> String {
        use protocol::{ConversationMessage, MessageId};
        use sidequery::{QuerySource, SideQueryRequest};
        let truncated = crate::markdown::truncate_markdown(markdown.to_string());
        let model_prompt = crate::markdown::make_secondary_model_prompt(
            &truncated,
            prompt,
            crate::markdown::is_preapproved_domain(host),
        );
        let req = SideQueryRequest {
            model: self.apply_model(),
            system_prompt: None,
            messages: vec![ConversationMessage::user(MessageId::new(), model_prompt)],
            tools: vec![],
            tool_choice: None,
            output_format: None,
            max_tokens: 1024,
            max_retries: 1,
            temperature: None,
            thinking_budget: None,
            stop_sequences: vec![],
            query_source: QuerySource::WebFetchApply,
            skip_system_prompt_prefix: true,
        };
        match client.query(req).await {
            Ok(resp) => resp.text.unwrap_or_else(|| "No response from model".to_string()),
            Err(_) => "No response from model".to_string(),
        }
    }
```

Now change the success arm of `call()`. Replace the existing `Ok(resp) => { ... }` block (`web_fetch.rs:496-539`) with the version below — the changes are: (1) convert HTML→markdown into `content`, (2) cache the markdown, (3) run the apply step when a prompt + client are present.

```rust
            Ok(resp) => {
                let status = resp.status;
                let body_bytes = resp.body.len();
                let content_type = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map_or_else(String::new, |(_, v)| v.clone());
                let (raw_body, truncated) = truncate_body(resp.body);

                // HTML→markdown (claude-code converts HTML; non-HTML is used as-is).
                // Behind `web-markdown`; with the feature off, content is the raw body
                // (byte-identical to the pre-conversion behavior).
                #[cfg(feature = "web-markdown")]
                let content = if crate::markdown::is_html_content_type(&content_type) {
                    crate::markdown::html_to_markdown(&raw_body)
                } else {
                    raw_body
                };
                #[cfg(not(feature = "web-markdown"))]
                let content = raw_body;

                // Cache the (markdown) content under the ORIGINAL url (utils.ts:505-517).
                crate::cache::cache_set(
                    parsed_input.url.clone(),
                    crate::cache::CachedFetch {
                        content: content.clone(),
                        status,
                        content_type,
                        bytes: body_bytes,
                        persisted_path: None,
                    },
                );
                self.emit_completed(&invocation_id, status, body_bytes as u64, truncated, elapsed_ms)
                    .await;

                // Apply step: process the markdown with the caller's prompt via the
                // small-fast model. Only when BOTH a prompt and a client are present;
                // otherwise return the markdown (graceful, mobile/feature-off path).
                let out_content = self
                    .maybe_apply(&host, &content, parsed_input.prompt.as_deref())
                    .await
                    .unwrap_or(content);

                Ok(ToolCallResult {
                    data: json!({
                        "url": parsed_input.url,
                        "status": status,
                        "content": out_content,
                        "truncated": truncated,
                        "bytes": body_bytes,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
```

Add the `maybe_apply` shim inside `impl WebFetchTool` (it isolates the `#[cfg]` so `call()` stays clean and compiles in both feature states):

```rust
    /// Returns `Some(model_output)` when the apply step ran, else `None`.
    async fn maybe_apply(
        &self,
        host: &str,
        content: &str,
        prompt: Option<&str>,
    ) -> Option<String> {
        #[cfg(feature = "web-markdown")]
        {
            if let (Some(client), Some(p)) = (self.side_query.as_ref(), prompt) {
                return Some(self.apply_prompt(client, host, content, p).await);
            }
        }
        #[cfg(not(feature = "web-markdown"))]
        {
            let _ = (host, content, prompt);
        }
        None
    }
```

- [ ] **Step 4: Run both tests to verify they pass**

Run: `cargo test -p tool-web --features web-markdown --lib apply_step_runs_model_over_markdown no_side_query_returns_markdown_unchanged_behavior`
Expected: PASS (both).

- [ ] **Step 5: Verify the feature-off build is unchanged**

Run: `cargo test -p tool-web --lib` (no `--features`)
Expected: PASS — the existing happy-path test (`happy_path_emits_completed`) still sees `content == "hello world"` (non-HTML body, no conversion, no apply).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/tools/web/src/web_fetch.rs
git commit -m "feat(tool-web): WebFetch HTML->markdown + secondary-model apply step"
```

---

## Task 7: Thread the side-query client through `register_all`

**Files:**
- Modify: `lingxi-code/tools/web/src/lib.rs`

The current function (`tools/web/src/lib.rs:25-29`) is:

```rust
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(WebFetchTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(WebSearchTool::new(ctx)));
}
```

- [ ] **Step 1: Add the side-query parameter**

Replace the function with:

```rust
/// Register the web fetch + search tools against `reg`.
///
/// `side_query`: the small-fast client powering WebFetch's apply step. `None`
/// (mobile/minimal, or the offline registry-snapshot path) ⇒ WebFetch returns
/// markdown without the secondary model.
pub fn register_all(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    side_query: Option<std::sync::Arc<dyn sidequery::SideQueryClient>>,
) {
    use std::sync::Arc;
    let web_fetch = match side_query {
        Some(client) => WebFetchTool::new(ctx.clone()).with_side_query(client),
        None => WebFetchTool::new(ctx.clone()),
    };
    reg.register_builtin(Arc::new(web_fetch));
    reg.register_builtin(Arc::new(WebSearchTool::new(ctx)));
}
```

(`mod markdown;` was already added in Task 3 Step 5.)

- [ ] **Step 2: Build the crate**

Run: `cargo build -p tool-web`
Expected: PASS — tool-web compiles. (The callers in engine-desktop/engine-mobile now have arity errors; fixed in Task 8.)

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/tools/web/src/lib.rs
git commit -m "feat(tool-web): register_all threads an optional side-query client to WebFetch"
```

---

## Task 8: Wire the desktop root (and pass `None` from mobile/snapshot)

`tool_web::register_all` is called from inside `register_desktop_tools` (`engine-desktop/lib.rs:377`), which already threads wired optionals (`cron_auth`, `skill_loader`, `cwd_changed_firer`). We add `web_side_query` the same way. `register_desktop_tools` has two call sites: the offline-snapshot path (`desktop_tool_registry`, `:342`, passes `None`s) and `build()` (`:1551`, which has the real client built at `:1284`). engine-mobile calls `tool_web::register_all` directly (`:105`).

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs`
- Modify: `lingxi-code/apps/engine-mobile/src/lib.rs`

- [ ] **Step 1: Add `web_side_query` to `register_desktop_tools` (`:359`)**

Add a final parameter to the signature:

```rust
pub fn register_desktop_tools(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    coordinator: Option<CoordinatorWiring>,
    cron_auth: Option<Arc<dyn tool_cron::ClaudeAiAuthProvider>>,
    skill_loader: Option<Arc<dyn tool_skill::skill::SkillLoader>>,
    cwd_changed_firer: hooks::OptionalCwdChangedFirer,
    web_side_query: Option<Arc<dyn sidequery::SideQueryClient>>,
) {
```

And update the WebFetch registration line (`:377`) from `tool_web::register_all(reg, ctx.clone());` to:

```rust
    tool_web::register_all(reg, ctx.clone(), web_side_query);
```

- [ ] **Step 2: Snapshot-path call site (`desktop_tool_registry`, `:342`) passes `None`**

Change `:342` from:

```rust
    register_desktop_tools(&mut reg, ctx, coordinator, cron_auth, None, None);
```

to (one more trailing `None` for `web_side_query`):

```rust
    register_desktop_tools(&mut reg, ctx, coordinator, cron_auth, None, None, None);
```

- [ ] **Step 3: `build()` passes the real client (`:1284`/`:1551`)**

`side_query_client` is built at `:1284` and currently MOVED into the forked runner at `:1288`. Clone it there so it survives to the registration call. Change `:1288` from:

```rust
            .with_side_query_client(side_query_client, orch_cfg.model.clone()),
```

to:

```rust
            .with_side_query_client(side_query_client.clone(), orch_cfg.model.clone()),
```

Then at the `register_desktop_tools(...)` call in `build()` (`:1551`), add the trailing argument. The call becomes:

```rust
    register_desktop_tools(
        &mut reg,
        ctx,
        coordinator,
        cron_auth,
        skill_loader,
        cwd_changed_firer,
        Some(side_query_client.clone()),
    );
```

(Match the argument expressions already present at `:1551`; only the trailing `Some(side_query_client.clone())` is new. `:1284` precedes `:1551`, so the client is in scope; if a future reorder breaks that, move the `:1284` `let` above the registration — it depends only on `cfg.api_key`/`cfg.api_base`/`http`.)

- [ ] **Step 4: engine-mobile passes `None` (`:105`)**

In `lingxi-code/apps/engine-mobile/src/lib.rs:105`, change `tool_web::register_all(reg, ctx.clone());` to:

```rust
    tool_web::register_all(reg, ctx.clone(), None);
```

- [ ] **Step 5: Build both engines**

Run: `cargo build -p engine-desktop && cargo build -p engine-mobile`
Expected: both PASS.

- [ ] **Step 6: Confirm mobile does NOT pull `htmd`**

Run: `cargo tree -p engine-mobile 2>/dev/null | grep -c htmd`
Expected: `0`.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/apps/engine-desktop/src/lib.rs lingxi-code/apps/engine-mobile/src/lib.rs
git commit -m "feat(engine): wire WebFetch side-query (desktop Some, mobile/snapshot None)"
```

---

## Task 9: Full gate

**Files:** none (verification only).

- [ ] **Step 1: Struct-trap (whole-workspace compile)**

Run: `cargo test --workspace --no-run`
Expected: builds with no errors.

- [ ] **Step 2: Targeted tests (both feature states)**

Run: `cargo test -p tool-web && cargo test -p tool-web --features web-markdown && cargo test -p sidequery`
Expected: all pass.

- [ ] **Step 3: Clippy (both feature states)**

Run: `cargo clippy -p tool-web --all-targets --no-deps -- -D warnings && cargo clippy -p tool-web --all-targets --features web-markdown --no-deps -- -D warnings && cargo clippy -p sidequery --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 4: Confirm existing WebFetch parity fixtures unchanged**

Run: `cargo test -p test-harness web` (and any `parity_web_tools` test)
Expected: PASS — the feature-off path is byte-identical, so locked web fixtures stay green.

- [ ] **Step 5: Final commit (if any fixups)**

```bash
git add -A
git commit -m "test(tool-web): gate WebFetch markdown+apply across feature states"
```

---

## Notes & follow-ups (out of scope for this plan)

- **Provider-aware model resolution:** `apply_model()` is a minimal Anthropic→Haiku resolver. The fuller resolver (explicit WebFetch override → router alias/profile → Haiku-only-when-Anthropic → main-loop fallback) is a follow-up.
- **Preapproved-domain list:** `is_preapproved_domain` returns `false` (strict guidelines). Porting `claude-code/src/tools/WebFetchTool/preapproved.ts` is a follow-up; the strict path is the safe default.
- **htmd vs html2md:** if `htmd` output is unacceptable on representative pages, swap to `html2md` (same `web-markdown` feature, same `html_to_markdown` seam).
