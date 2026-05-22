# lingxi-api-client

Anthropic / OpenAI-compatible API client for the LingXi engine. All network I/O routes through `lingxi_traits::HttpTransport`; this crate itself is purely about request shape, SSE parsing, and retry policy. Provides:
- `AnthropicProvider::{new, build_request, build_streaming_request, parse_stream_event}`.
- API-shape DTOs (`MessageRequest`, `MessageResponse`, `StreamEvent`, `ContentDelta`, ...).
- SSE parser (`parse_sse_chunks`).
- `ApiError` — provider-error taxonomy (`PromptTooLong`, `RateLimited`, `Unauthorized`, etc.).

See `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md` §6 (API client layer).
