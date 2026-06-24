# Shell & Web Tool Parity Gap Audit

**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
**LingXi shell tools:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/tools/shell/src/`
**LingXi web tools:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/tools/web/src/`

**All binary evidence independently verified by direct string extraction.**

**Total confirmed gaps: 8**
(P0: 6 schema-breaking, P1: 1 description/guidance, P2: 1 design divergence)

> Note: gaps originally numbered 7 and 8 in the sub-agent draft were non-gaps (gap 7 was a documentation note within a real gap; gap 8 was the REPL — renumbered below).

---

## Confirmed Gaps

| # | Tool | Dimension | Oracle (binary evidence) | LingXi (file:line) | Severity | Note |
|---|------|-----------|--------------------------|-------------------|----------|------|
| 1 | Bash | `timeout` field: JSON schema type | Binary @200146856: `function sB(e=A.number()){return A.preprocess(...,e)}` — wraps `A.number()` which emits `{"type":"number"}`. Binary schema at @203768858: `timeout:sB(A.number().optional())` | `bash.rs:839` → `"type": "integer"` | **P0** | Zod `.number()` emits `type:number`, not `integer`. Model-sent float timeout (e.g. 0.5) validates in binary, fails in LingXi. |
| 2 | Bash | `timeout` field: spurious `minimum` constraint | Binary @203768858: `timeout:sB(A.number().optional())` — no `.min()`, no `.nonnegative()`. Advisory only: `describe("Optional timeout in milliseconds (max ${xmt()})")` | `bash.rs:840` → `"minimum": 1` | **P0** | Invented constraint absent from binary schema. |
| 3 | PowerShell | `timeout` field name | Binary @203601400: `A.strictObject({command:...,timeout:sB(A.number().optional())...})` — field named **`timeout`** | `powershell.rs:98` → field named **`timeout_ms`** | **P0** | Wrong field name. Binary uses `timeout` (matching Bash). Any model-generated payload will use `timeout`. |
| 4 | PowerShell | `timeout` type + spurious constraints | Binary @203601400: `timeout:sB(A.number().optional())` → `type:number`, no `minimum`, no `maximum` | `powershell.rs:98` → `"type":"integer","minimum":1,"maximum":600000` | **P0** | Three divergences on one field. |
| 5 | PowerShell | `dangerouslyDisableSandbox` field absent | Binary @203601400: `dangerouslyDisableSandbox:kI(A.boolean().optional()).describe("Set this to true to dangerously override sandbox mode and run commands without sandboxing.")` | `powershell.rs:93–103` — field completely absent | **P0** | Schema field missing entirely. |
| 6 | PowerShell | `additionalProperties` not false | Binary @203601400: `A.strictObject({...})` emits `additionalProperties:false`. PowerShell schema uses `A.strictObject` identically to Bash. | `powershell.rs:93–103` — no `"additionalProperties"` key; uses plain `json!({})` | **P0** | Unknown keys pass through LingXi; binary rejects them. |
| 7 | PowerShell | Per-field `description` strings absent | Binary @203601400: `command.describe("The PowerShell command to execute")`, `description.describe("Clear, concise description of what this command does in active voice.")`, `run_in_background.describe("Set to true to run this command in the background.")`, `dangerouslyDisableSandbox.describe("Set this to true to dangerously override sandbox mode and run commands without sandboxing.")` | `powershell.rs:97–101` — all four fields have no `"description"` key | **P1** | Missing field descriptions degrade model guidance quality. |
| 8 | REPL | Tool semantics: language-runner vs transparent wrapper | Binary `"REPL"` @198059604: `isTransparentWrapper(){return!0}` — a JS VM wrapping Bash/Read/Edit/etc; only exposed for `USER_TYPE=ant` internal builds | `repl.rs`: `{language: enum["python","node","ruby"], code: string}` direct language runner | **P2** | LingXi implements the older claude-code REPL design. Binary's REPL is ant-internal only; external users never see it. Functionally equivalent for external parity but is an architectural divergence. |

---

## Verified Correct (no gap)

- **Bash `command` field**: type:string, `describe("The command to execute")` — matches binary @203768858 exactly
- **Bash `description` field**: full multi-line describe text including "active voice", "Never use words like complex or risk", examples — matches binary @203768858 exactly
- **Bash `run_in_background`**: type:boolean, `describe("Set to true to run this command in the background.")` — matches binary
- **Bash `dangerouslyDisableSandbox`**: present, type:boolean, describe text matches binary @203768858
- **Bash `required: ["command"]`**: matches binary
- **Bash `additionalProperties: false`**: binary uses `A.strictObject`; LingXi has `"additionalProperties": false` — correct
- **Bash output limits**: `BASH_MAX_OUTPUT_DEFAULT=30_000`, `BASH_MAX_OUTPUT_UPPER_LIMIT=150_000` match binary (`xZr=30000, IZr=150000`)
- **Bash sleep threshold**: `SLEEP_BLOCK_THRESHOLD_SECS=25.0` matches binary (`E$n=25`); leaked TS says 2 but binary has 25
- **Bash sleep message**: "To wait for a condition, use Monitor with an until-loop…" matches binary exactly
- **Bash background task message**: full "Command running in background with ID: {}" message matches binary
- **Bash timeout defaults**: `BASH_DEFAULT_TIMEOUT_MS=120_000`, `BASH_MAX_TIMEOUT_MS=600_000` match binary
- **WebFetch tool name**: "WebFetch" — correct
- **WebFetch input schema**: `required:["url","prompt"]`, both type:string, url format:uri, additionalProperties:false — all match binary
- **WebFetch field descriptions**: "The URL to fetch content from", "The prompt to run on the fetched content" — match binary
- **WebFetch description text**: full multi-line description including 15-min cache note matches binary
- **WebFetch 15-minute cache**: present in both description and concise prompt — correct
- **WebSearch tool name**: "WebSearch" — correct
- **WebSearch input schema**: `required:["query"]`, additionalProperties:false, query minLength:2, allowed_domains/blocked_domains optional arrays — all match binary
- **WebSearch field descriptions**: "The search query to use", "Only include search results from these domains", "Never include search results from these domains" — all match binary
- **WebSearch description/prompt**: VERBOSE and CONCISE variants, em-dash, CRITICAL REQUIREMENT section — all verified
- **WebFetch constants**: `WEBFETCH_MAX_TRANSFER_BYTES=10*1024*1024`, `WEBFETCH_MAX_MARKDOWN_LEN=100_000`, `WEBFETCH_MAX_REDIRECTS=10` match binary

---

## UNCERTAIN

| Item | Reason |
|------|--------|
| REPL `language`/`code` schema exact JSON in binary | Binary REPL is an isTransparentWrapper with dynamic registration; no static language/code schema string in the bundle. LingXi's schema models the older design — cannot byte-verify current binary. |

---

## Remediation

**PowerShell tool** (`tools/shell/src/powershell.rs`) — needs a full schema rewrite:

1. Rename `timeout_ms` → `timeout`
2. Change timeout type from `"integer"` to `"number"` (match `sB(A.number().optional())`)
3. Remove `"minimum": 1` and `"maximum": 600_000` from timeout
4. Add `"dangerouslyDisableSandbox": {"type": "boolean", "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."}`
5. Add `"additionalProperties": false` to the schema root object
6. Add `"description"` strings to all four fields (command, timeout, description, run_in_background)

**Bash tool** (`tools/shell/src/bash.rs`) — two field-level fixes:

1. Change `timeout` field type from `"integer"` to `"number"` (line 839)
2. Remove `"minimum": 1` from the timeout field (line 840)

**WebFetch and WebSearch**: byte-clean, no changes needed.
