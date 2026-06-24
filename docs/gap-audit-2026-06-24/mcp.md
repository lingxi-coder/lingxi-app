# MCP Parity Gap Audit vs v2.1.186
Date: 2026-06-24
Confirmed gaps: 8 | Uncertain: 3

---

## Confirmed Gaps

| # | Area | Item | Oracle (evidence) | LingXi (file:line) | Severity | Note |
|---|------|------|-------------------|--------------------|----------|------|
| 1 | Initialize / clientInfo | Missing `description` field in `clientInfo` wire payload | Binary at 84000384: `description:"Anthropic's agentic coding tool"` in `new Client({name,title,version,**description**,websiteUrl},{capabilities:{...}})`. TS source `client.ts:985-991` confirms both initialize blocks (lines 985 and 3280) send it. | `lingxi-code/mcp/src/identity.rs` — `ClientInfo` struct has only `name`, `title`, `version`, `website_url`; no `description` field. | P1 | The MCP server's `initialize` response may depend on clientInfo fields to select behavior. Missing `description` = wire divergence on every `initialize` handshake. |
| 2 | Tool name normalization | `McpClient::list_tools` only normalizes the server segment, not the tool name | TS `mcpStringUtils.ts:51`: `buildMcpToolName = getMcpPrefix(server) + normalizeNameForMCP(toolName)` — normalizes BOTH segments. Binary at 64316205 emits `mcp__<norm-server>__<norm-tool>`. | `lingxi-code/mcp/src/client.rs:257-261`: FQN built as `format!("mcp__{}__{}",normalize(server), t.name)` — `t.name` is raw, not normalized. | P1 | Partially mitigated: `registry.rs:444-448` overwrites the FQN in the registry's `connect()` path (normalizes both segments there). BUT `McpClient::servers_with_tools()` at line 1120 calls `client.list_tools()` directly — tools from that path have un-normalized tool names in their `full_name`. Any other direct `list_tools()` call will also produce wrong FQNs for tools with invalid-character names (e.g. `my.tool` → `mcp__server__my.tool` instead of `mcp__server__my_tool`). |
| 3 | MCP config / transport | Missing `claudeai-proxy` and `sdk` transport types | Binary: `claudeai-proxy` at offset 74175408, 81811504; `sdk` type at 194710219, 196781049. TS `client.ts:868,866,1762`, `cli/handlers/mcp.tsx:177-179`, `claudeai.ts:114` — `type: 'claudeai-proxy'` is used for claude.ai hosted MCP servers; `type: 'sdk'` for agent-SDK-embedded servers. | `lingxi-code/mcp/src/json_config.rs:79-83,181-215`: `McpJsonEntry.transport_type` field is parsed but only `"sse"` and `"http"` are recognized. `claudeai-proxy` falls through to the default `Http` branch; `sdk` transport is not parsed at all. `McpTransportSpec` enum (traits) has no `ClaudeAiProxy` or `Sdk` variant. | P1 | `claudeai-proxy` affects claude.ai MCP servers (OAuth path + proxy URL handling). `sdk` transport affects agent SDK embedded servers (`CLAUDE_AGENT_SDK_MCP_NO_PREFIX` env gate). Both are real production transport paths. |
| 4 | Security / Unicode sanitization | MCP tool/prompt responses not Unicode-sanitized | TS `client.ts:1758`: `recursivelySanitizeUnicode(result.tools)` before processing tools; line 2051: `recursivelySanitizeUnicode(result.prompts)`. Sanitization removes zero-width chars, format controls, private-use areas (ASCII Smuggling / Hidden Prompt Injection defense — HackerOne #3086545). | `lingxi-code/mcp/src/client.rs:241-268` (`list_tools`), `:467-480` (`list_prompts`): no Unicode sanitization applied to tool/prompt data from MCP servers. `recursivelySanitizeUnicode` equivalent not found anywhere in lingxi-code. | P0 | Security gap. Malicious MCP servers can inject invisible Unicode (Tag characters, zero-width) into tool names/descriptions that are invisible to users but executed by the model. The binary explicitly mitigates this after a HackerOne report. |
| 5 | `${{tool._meta}}` / searchHint forwarding | `_meta.anthropic/searchHint` and `_meta.anthropic/alwaysLoad` parsed but NOT forwarded to `McpToolDto` | TS `client.ts:1777-1783`: `searchHint` from `tool._meta['anthropic/searchHint']` and `alwaysLoad` from `tool._meta['anthropic/alwaysLoad']` set on each registered Tool object and used for deferred-tool search ranking. | `lingxi-code/mcp/src/client.rs:866-886` (`ToolMeta`): `search_hint` / `always_load` parsed from the wire but tagged `#[allow(dead_code)]`. `McpToolDto` (traits `mcp.rs:178-189`) has NO `search_hint`/`always_load` fields. The data is read off the wire and discarded. | P1 | Tool retrieval ranking is broken for MCP tools that advertise `anthropic/searchHint` — they won't be found by ToolSearch when the hint would have matched, and `alwaysLoad` tools won't be force-included. |
| 6 | MCP prompts slash command name | Prompt slash command name only normalizes server, not prompt name | TS `client.ts:2058`: `name: 'mcp__' + normalizeNameForMCP(client.name) + '__' + prompt.name` — prompt name is raw (no normalization). This is DIFFERENT from tools where `buildMcpToolName` normalizes both. This is the correct TS behavior. | `lingxi-code/mcp/src/client.rs:476-479`: `McpPromptDto { name: p.name, ... }` — only the raw prompt name from the server is stored; NO `mcp__<server>__` prefix is prepended at all at this layer. The slash command FQN construction is deferred to the composition root, but `McpPromptDto` carries no normalized prefix. | P1 | The prompt DTO name is just the raw server-provided prompt name (e.g. `"search"`). The full slash-command name `mcp__<server>__search` is supposed to be built at the slash-command registration site. Need to verify whether the composition root actually builds and registers the prefixed slash command names at all. If it does not, `/mcp__github__search` won't be available as a slash command. |
| 7 | CLI `/mcp list` empty-server message | Different text for empty server list in CLI output | Binary at 117090800: `"No MCP servers configured. Use \`claude mcp add\` to add a server."` (confirmed by TS `cli/handlers/mcp.tsx:151`). | `lingxi-code/commands/core/src/mcp.rs:37`: `render_list("MCP servers", rows, "No MCP servers configured")` — no period, no "Use `claude mcp add`" suffix. | P2 | Cosmetic/string divergence in `claude mcp list` CLI output. |
| 8 | CLAUDE_AGENT_SDK_MCP_NO_PREFIX gate | `sdk` transport type + env gate to skip `mcp__` prefix entirely | Binary at 67524672: `CLAUDE_AGENT_SDK_MCP_NO_PREFIX`. TS `client.ts:1762-1770`: `const skipPrefix = client.config.type === 'sdk' && isEnvTruthy(process.env.CLAUDE_AGENT_SDK_MCP_NO_PREFIX)` — when set, tool `name` sent to model is the raw `tool.name` (not FQN), allowing MCP tools to override builtins by name. | No `sdk` transport type in LingXi `McpTransportSpec`; `CLAUDE_AGENT_SDK_MCP_NO_PREFIX` env var not checked anywhere. | P1 | Affects Agent SDK embedded use cases where MCP tools intentionally shadow builtins. |

---

## Uncertain / Needs Deeper Look

| # | Area | Item | Oracle hint | LingXi | Note |
|---|------|------|-------------|--------|------|
| U1 | MCP prompts slash-command registration | Whether the composition root builds `mcp__<server>__<prompt>` slash commands from `McpPromptDto.name` | TS `client.ts:2058` builds `mcp__${normalizeServer}__${raw_prompt_name}` and registers it as a `Command`. The binary processes `prompts/list_changed` notifications to refresh. | Registry (`registry.rs:425,461`) stores raw `McpPromptDto`s. The composition root or slash-command dispatcher would need to prepend `mcp__<server>__`. Search for that site was inconclusive in this session. | If the composition root doesn't prepend, MCP prompt slash commands are missing entirely (P0). If it does, gap #6 closes. |
| U2 | tools/list pagination / nextCursor loop | Binary warning `"still returning nextCursor after N pages; stopping"` (offset 83743089) indicates a guarded cursor-loop for `tools/list` | TS `client.ts:1753` shows a single `client.request({method:'tools/list'})` call — the MCP SDK `Client.request` may transparently handle pagination internally. Binary warning may originate in the SDK, not the app. | `lingxi-code/mcp/src/client.rs:241-247`: single `tools/list` call, no cursor loop. | Low impact if SDK handles pagination transparently. Only a gap if the SDK does NOT auto-paginate and the app must loop manually. Needs SDK source inspection. |
| U3 | `websiteUrl` value correctness | Binary at 200376067 shows `websiteUrl:a9e` (variable `a9e` = `PRODUCT_URL` = `'https://claude.com/claude-code'`, per `constants/product.ts:1`). | `lingxi-code/mcp/src/identity.rs:16`: `pub const MCP_WEBSITE_URL: &str = "https://claude.com/claude-code"` — matches. | Confirmed match — no gap. Listed here because the variable indirection in the binary requires verification. |

---

## Summary of Evidence

### Binary extraction methodology
- String literals extracted with `grep -aboF 'string' /path/to/binary`
- Contexts extracted with `tail -c +OFFSET binary | head -c N | strings`
- TS source files used as cross-reference hints only; binary is canonical

### Key binary oracle facts
1. `initialize` clientInfo wire: `{name:"claude-code", title:"Claude Code", version:"...", description:"Anthropic's agentic coding tool", websiteUrl:"https://claude.com/claude-code"}` (offset 84000384)
2. `protocolVersion`: `"2025-11-25"` (offsets 64070816, 194094053) — LingXi matches
3. `capabilities`: `{"roots":{}, "elicitation":{}}` — LingXi matches
4. Tool FQN: `mcp__${normalizeServer}__${normalizeTool}` via `buildMcpToolName` (mcpStringUtils.ts:51) — LingXi registry corrects server segment and tool segment but McpClient::list_tools leaves tool raw
5. `claudeai-proxy` transport (offsets 74175408, 81811504) — LingXi missing
6. `sdk` transport + `CLAUDE_AGENT_SDK_MCP_NO_PREFIX` (offset 67524672) — LingXi missing
7. Unicode sanitization on tool/prompt data from MCP servers (TS client.ts:1758,2051) — LingXi missing
8. `recursivelySanitizeUnicode` protects against ASCII Smuggling (HackerOne #3086545)
9. `anthropic/searchHint` + `anthropic/alwaysLoad` in `tool._meta` forwarded to registered Tool object — LingXi parses but discards
10. CLI empty-server text: `"No MCP servers configured. Use \`claude mcp add\` to add a server."` — LingXi uses shorter string without suffix

### LingXi correctly implements
- Protocol version `2025-11-25`
- `clientInfo.name="claude-code"`, `.title="Claude Code"`, `.websiteUrl="https://claude.com/claude-code"`
- `capabilities: {roots:{}, elicitation:{}}`
- `roots/list` → `{"roots":[{"uri":"file://<cwd>"}]}`
- `elicitation/create` default → `{"action":"cancel"}`
- `claudeai.ai ` prefix collapse-and-trim in `normalizeNameForMCP`
- `mcp__<server>__<tool>` FQN (at registry connect site, both segments normalized)
- Tool description truncation at 2048 chars with `… [truncated]` suffix
- `tools/list`, `prompts/list`, `resources/list`, `resources/read` RPC methods
- `_meta.claudecode/toolUseId` in tools/call request
- `MCP_TOOL_TIMEOUT` env-var-gated timeout with `~27.8h` default
- `ListMcpResourcesTool`, `ReadMcpResourceTool`, `MCP`, `McpAuth` builtin tool names
- `tools/list`, `prompts/list`, `resources/list`, `resources/read`, `completion/complete` method strings present
