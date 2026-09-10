# mcp / plugin re-audit — 2.1.251 → 2.1.267

The mcp/plugin subsystem was last aligned against **2.1.251** (2026-08-28…31),
sixteen releases back — the largest baseline gap in the tree. This is the
re-audit.

**Oracles, both verified.** New: `~/.local/share/claude/versions/2.1.267`,
sha256 `a681f3008f0050029aeebcab3af51bb6a55ddeb625a3af3141a4416d43cd2558`.
Old: 2.1.251, refetched with
`npm pack @anthropic-ai/claude-code-darwin-arm64@2.1.251`, sha256
`625869b01e0050f260b2980fac248fd9cef9e462612bded4ec9d3d49ff8969a5` — which
**matches the value `docs/REMAINING.md` recorded**, so the old side is the same
binary the 251 audit used. Tree at `13d73b2a8`.

---

## Method, and its self-check

Naive cross-version string diff over the two binaries yielded **198**
mcp/plugin candidates. Raw-substring re-verification of each candidate's
longest placeholder-free fragment against 2.1.251's **bytes** dropped 45 of
them — a **23% false-positive rate**, in line with the 36% measured on a
263→267 sample. 153 survived as genuinely new.

Port presence was then checked against a 16 MB concatenation of the port's
mcp/plugin-adjacent Rust sources, trying both the raw and Rust-escaped
spellings. That returned **0 of 153 present** — a 100% miss is the signature of
a broken comparison, so before believing it I probed the haystack with four
identifiers known to be in the port: `requiresUserInteraction` (16),
`resource_metadata` (55), `allowManagedMcpServersOnly` (17),
`skipMcpDiscovery` (8). The haystack is sound; the miss is real, and expected
for a port pinned at 251.

🚨 **153 is an upper bound for scoping, not a backlog.** Classified:

| bucket | count | disposition |
|---|---|---|
| MCP connection / transport / auth | 45 | **the actionable bucket** — see below |
| marketplace / plugin CLI copy | 21 | mostly copy, low risk |
| SDK / connector / claude.ai | 12 | accepted divergence |
| function-hook runtime | 6 | classified, not to build |
| plugin surface modules / JSX | 6 | new in 267, classified, not to build |

---

## MCP-01 — no legacy HTTP+SSE fallback on a rejected `initialize` — ✅ REAL

Upstream dials streamable HTTP, and when the `initialize` POST is rejected it
**re-dials the server over legacy HTTP+SSE** (`src_187489883.js` @63137,
mirrored at `src_187282055.js` @63512):

```js
try { await se(L, P, Xl()) }
catch(A){
  if (I===void 0 || P instanceof vmt && P.protocolVersion!==void 0
      || !en(A) || !H("tengu_mcp_legacy_sse_fallback",!0)) throw A;
  J(e, `initialize POST rejected (…); trying legacy HTTP+SSE`);
  let k = A.code === 405;
  await P.close().catch(()=>{});
  … // re-dial, passing { postMethodNotAllowed: k, dialSignal, dialConnected }
}
```

The predicate is precise, and identical in both chunks:

```js
function Hs(e){
  if(!(e instanceof o_) || (e.status!==400 && e.status!==404 && e.status!==405)) return !1;
  let n = e.data.text;
  if(typeof n !== "string") return !0;
  return !js(n);              // …and the body is NOT a JSON-RPC message
}
```

So the fallback fires on a **400/404/405 whose body does not parse as JSON-RPC**
(`js` tolerates SSE framing by reading the `data:` line). A server that returned
a *valid JSON-RPC error* over the rejected POST is a real protocol failure and
must NOT be re-dialled — getting that half wrong would convert protocol errors
into silent transport churn.

**Port:** absent. `McpClient::initialize` (`mcp/src/client.rs:754`) maps any
error straight to `McpClientError::Rpc` with no fallback; `legacy_sse`,
`postMethodNotAllowed` and any 400/404/405 branch are 0 hits in `mcp/`.
`tengu_mcp_legacy_sse_fallback` is 0 hits (oracle default **true**, so this is
live upstream behaviour, not a dormant flag).

**Impact:** an MCP server that speaks only HTTP+SSE fails to connect here and
connects upstream. The substrate exists — `McpTransportSpec::Sse` is already a
supported *configured* transport — so this is wiring a fallback, not building a
transport.

## MCP-02 — a project `.mcp.json` can smuggle an unresolved `${…}` into a command — ✅ REAL, security-relevant

`src_175313539.js` @26576, in the project-scope arm of the approval builder:

```js
let a = {...n, command:u(n.command), url:u(n.url),
         args:n.args?.map((b)=>Te(b,e)), env:ct(n.env,e), headers:ct(n.headers,e)};
if (r === "project") {
  if ([a.url, a.command, ...a.args??[]].some((S)=> S!==void 0 && S.includes("${")))
    return { refused: "Its url, command or args reference an environment variable; on connect Claude Code would expand it into a repo-authored command or url. Add it manually with `claude mcp add` if you trust this repo." };
  …
}
```

Two details carry the whole property. The test runs on the **expanded** value
`a`, not the raw config — so a `${VAR}` that resolved is fine, and only one that
did NOT resolve (unset variable, literal `${…}` surviving) is refused. And it is
gated on `r === "project"`: the same shape from a user-scoped config is allowed,
because the user authored it. The threat is a repo-authored `.mcp.json`.

**Port:** absent. `mcp/src/server_gate.rs` approves/rejects project entries **by
name** (`approved_project_servers` / `rejected_project_servers`) with no
content test. `env_expansion.rs:133`'s `contains("${")` is an early-out inside
the expander, and `discovery_cache.rs:749`'s `has_placeholder` is cache-safety —
neither refuses anything.

**This is the highest-value implementable item in this audit.**

## MCP-03 — `managedMcpServers` is not read — ⚠️ REAL, needs a scope decision

2.1.267 accepts managed MCP servers under a `managedMcpServers` **settings key**,
with its own shape validation:

> `"managedMcpServers" must be an object keyed by server name (the .mcp.json
> mcpServers shape; Claude Desktop's array form of its same-named key is not
> accepted here: use the server name as the key and "type" instead of
> "transport"). No managed MCP servers are installed from it until it is fixed.`

**Port:** `managedMcpServers` is 0 hits. The port reads managed MCP policy from a
**separate file** instead (`enterprise_policy.rs:46` `managed_mcp_config_path()`,
with `allowedMcpServers` / `allowManagedMcpServersOnly`). So this is not a
missing validator — it is a different intake shape, and adopting the key means
deciding whether the two coexist. Do not bolt the error string onto the existing
reader; it validates a key this port does not consume.

## MCP-04 — three further new gates, all classified

| gate | oracle | port | disposition |
|---|---|---|---|
| `tengu_mcp_legacy_sse_fallback` | 3 | 0 | MCP-01 above |
| `tengu_sdk_mcp_manifests` | 2 | 0 | SDK control-protocol surface |
| `tengu_sdk_workspace_trust` | 2 | 0 | SDK `initialize: workspaceTrust` |
| `tengu_plugins_sync_mcp_relay_wait` | 2 | 0 | telemetry only (reconcile/connect budgets) |

## CLOSED since the 251 audit

`tengu_surface_failed_mcp_servers` — the 251 report recorded the port defaulting
this flag **false** against an oracle default of **true**, i.e. failed MCP
servers were never surfaced in the ToolSearch note. `tools/meta/src/tool_search.rs`
now reads `telemetry::flag_bool("tengu_surface_failed_mcp_servers", true)`.
Strike it from the backlog.

## Still open from the 251 audit, unchanged

Both remain 0 hits and both are still blocked for the reasons recorded then:
the startup `N MCP servers need authentication` warning (no runtime producer was
ever recovered from the binary — a changelog sentence is not a producer), and
managed-settings MCP startup-mode approval exemptions (owned by the
managed-settings approval subsystem, not this crate).

---

## Recommended order

1. **MCP-02** — bounded, security-relevant, exact spec above, and the approval
   path already exists to hang it on.
2. **MCP-01** — real interop loss, substrate present, but touches the dial path
   and needs both halves of the predicate to avoid turning protocol errors into
   transport churn.
3. **MCP-03** — needs a scope decision before any code.
