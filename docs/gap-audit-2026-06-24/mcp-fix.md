# MCP Parity Gap Fix Report
Date: 2026-06-24

## Summary

8 confirmed gaps from `/tmp/gap-audit/mcp.md` addressed. All binary facts verified before writing code. Full workspace build clean; 226 mcp+tool-mcp tests pass.

---

## Fixes Applied

### [P0] Unicode sanitization (HackerOne #3086545)
**Files:** `mcp/src/client.rs`, `mcp/Cargo.toml`

- Added `partially_sanitize_unicode(input: &str) -> String` — 1:1 port of TS `partiallySanitizeUnicode` from `utils/sanitization.ts`:
  - NFC normalization
  - Removes `Cf` (format controls), `Co` (private-use), `Cn` (noncharacters) via `char_is_dangerous()`
  - Explicit fallback ranges: U+200B–U+200F, U+202A–U+202E, U+2066–U+2069, U+FEFF, U+E000–U+F8FF
  - Iterates up to 10 times until stable
- Added `recursively_sanitize_unicode(value: serde_json::Value) -> serde_json::Value` — walks the JSON tree sanitizing all string keys and values
- Applied to `list_tools` (mirrors `client.ts:1758`: `recursivelySanitizeUnicode(result.tools)`) and `list_prompts` (mirrors `client.ts:2051`: `recursivelySanitizeUnicode(result.prompts)`) — both now deserialize from a sanitized `serde_json::Value` rather than directly
- Added `unicode-normalization = "0.1"` to `mcp/Cargo.toml`
- 7 new unit tests in `client::sanitization_tests` covering tag chars, zero-width, BOM, private-use, normal-text preservation, nested JSON, and primitives

### [P1] `clientInfo.description` missing from `initialize` handshake
**Files:** `mcp/src/identity.rs`, `mcp/src/lib.rs`

- Added `CLIENT_DESCRIPTION: &str = "Anthropic's agentic coding tool"` constant (binary-confirmed at offset 84000384)
- Added `description: &'static str` field to `ClientInfo` struct
- Updated `Default::default()`, `CLIENT_INFO` const, and lib.rs re-export
- Updated 3 existing identity tests to assert the description field

### [P1] Transport types `claudeai-proxy` + `sdk`
**File:** `mcp/src/json_config.rs`

- Added `Some("claudeai-proxy")` arm → `McpTransportSpec::Http` (proxy URL handling deferred to platform layer; binary-confirmed at offsets 74175408, 81811504)
- Added `Some("sdk")` arm → `McpTransportSpec::SdkControl { control_channel_id: url }` (binary-confirmed at offsets 194710219, 196781049)

### [P1] `CLAUDE_AGENT_SDK_MCP_NO_PREFIX` env gate
**File:** `mcp/src/client.rs`

- Added `is_env_truthy(name: &str) -> bool` private helper (matches `envUtils.ts:32-37` semantics)
- Added `McpClient::skip_mcp_prefix(&self) -> bool` method checking the env var
- `list_tools` now checks the gate and emits raw `t.name` as `full_name` when truthy (mirrors `client.ts:1762-1770`)

### [P1] Tool name normalization (both segments)
**File:** `mcp/src/client.rs`

- `list_tools` now normalizes BOTH server AND tool segments: `mcp__{normalize(server)}__{normalize(tool)}` — matches `buildMcpToolName` = `getMcpPrefix(server) + normalizeNameForMCP(toolName)` (`mcpStringUtils.ts:51`)
- Previously the tool segment was left raw; the registry's `connect()` site already normalized both, but `McpClient::list_tools()` called directly (e.g. `servers_with_tools()`) produced un-normalized tool names

### [P1] `searchHint` / `alwaysLoad` forwarded to `McpToolDto`
**Files:** `platform-api/src/mcp.rs`, `mcp/src/client.rs`, `platforms/posix/src/mcp.rs`, `mcp/src/registry.rs`, `test-harness/src/mocks/mock_mcp.rs`

- Added `search_hint: Option<String>` and `always_load: Option<bool>` fields to `McpToolDto` (both `#[serde(default, skip_serializing_if = "Option::is_none")]`)
- `list_tools` forwards `t.meta.search_hint` and `t.meta.always_load` — the `ToolMeta` fields parsed from `_meta` are no longer dead code
- Removed `#[allow(dead_code)]` from `RawTool.meta`
- Updated all 4 `McpToolDto { ... }` struct literal sites to include the new fields (`None` for callers that don't have meta)

### [P2] Empty-server CLI message
**File:** `commands/core/src/mcp.rs`

- Changed `render_list("MCP servers", rows, "No MCP servers configured")` to `"No MCP servers configured. Use \`claude mcp add\` to add a server."` — byte-exact match with binary offset 117090800
- Updated the `empty_list` test to assert the new full message

---

## Unresolved / Deferred

- **U1 (Prompt FQN composition root):** The `McpPromptDto` still carries raw server-provided prompt name without `mcp__<server>__` prefix. The gap audit notes this is built at the slash-command registration site. Investigation was inconclusive as to whether the composition root prepends the FQN. Deferred — not in the confirmed gap list for this session.
- **U2 (tools/list pagination cursor loop):** Single `tools/list` call only; MCP SDK may handle pagination transparently. Low impact.

---

## Build + Test Status

```
cargo build --workspace   → clean (0 errors, warnings only in unrelated crates)
cargo test -p mcp -p tool-mcp → 226 tests, 0 failures
cargo test -p command-core -- mcp → 1 test, 0 failures (empty-message test)
```
