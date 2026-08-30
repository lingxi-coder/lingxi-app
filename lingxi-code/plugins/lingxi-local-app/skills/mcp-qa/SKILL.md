---
name: mcp-qa
description: Evaluate a proposed app MCP tool's discovery, schema, permission, side effects, result shape, and app isolation against real Host evidence — never a passing prose claim.
---

# QA a proposed app MCP tool

Own turning a proposed (or already-active) per-App MCP tool into a
pass/fail verdict across six dimensions — discovery, schema, permission,
side effects, result shape, and isolation — each backed by an actual Host
call, log, or digest, never asserted from reading the proposal text or this
design doc. Fixing a dimension that fails is `$mcp-tool-design`'s or
`$mcp-flow-binding`'s job; this skill only evaluates and reports.

## There is no Host evidence for this yet — say so plainly

This is the honest state of the repo today, not a gap to route around: the
per-App MCP authoring workflow, the logical per-App server, the catalog
store, and every digest §12/§15/§16 describe are all design-only (see
`$expose-as-mcp` for the full non-existence list). Because none of them
exist, there is currently **no generated per-App tool to call**, so there
is nothing for a QA pass to point at. When asked to QA a per-App MCP tool
today, the correct answer is that the mechanism this skill depends on
doesn't exist yet — not a fabricated "passed" and not a workaround that
tests something else and calls it equivalent.

## The six dimensions, and what would ground each (§12.2, §13, §14, §19.8)

- **Discovery** — the tool appears in `tools/list` for the app's own
  logical server, at the surface its last-listed `tool_surface_sha256`
  names; a connection whose listed surface is stale is rejected until
  `tools/list` refreshes (§13.5); the tool is **absent** from every other
  app's connection (App A/B isolation).
- **Schema** — `name` matches `^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$`
  (1–64 chars, no doubled/edge `_`); `title` ≤100 chars; `description`
  ≤1000; input/output schema ≤32 KiB each, depth ≤8, closed object root, no
  remote `$ref` (§13.2); a call with input that fails the schema is
  rejected before execution (§13.5's `validate inputSchema` step); when an
  `outputSchema` is declared, the result's `structuredContent` is bound and
  validated against it (§13.1).
- **Permission** — the two independent pipelines from §14.1 both apply, and
  neither ever loosens the other: the rule-walk result from
  `PermissionRuleSource::McpServerPolicy` (BUILT —
  `permission/src/rule.rs:190,208,240`, wired into the deny→ask→allow walk
  at `permission/src/policy.rs:53`, already real for any MCP server today)
  is still clamped by the Host-derived `McpPermissionCeiling`
  (Allow/Ask/Deny, design-only, §14.1); `requiresUserInteraction=true`
  forces Ask regardless of ceiling; a call over ceiling fails with
  `permission_ceiling`, never a silent downgrade.
- **Side effects** — a mutating tool's derived `destructiveHint` /
  `idempotentHint` matches what its bound Flow steps actually do (§13.3's
  derivation table; conservative defaults `readOnlyHint=false,
  destructiveHint=true, idempotentHint=false, openWorldHint=true` apply
  whenever the Host can't prove otherwise); the mutation actually executes
  through the Host Flow engine — never App-provided JS, shell, a native
  module, or a WebView callback (§14.4).
- **Result shape** — `CallToolResult` carries `content` /
  `structuredContent` / `isError` / `_meta` byte for byte (§13.1). This
  part already has a real wire type: `McpToolResultDto`
  (`traits/src/mcp.rs:285-302`, BUILT — `content`, `is_error`, `meta`,
  `structured_content` all present today), though nothing populates it from
  a generated per-App catalog yet. List/search results are bounded with
  `cursor`/`has_more`; an error names a specific, actionable recovery.
- **Isolation** — the connection scope carries only `app_id` +
  `listed_tool_surface_sha256` (§14.3); a tool's own input schema never
  accepts `app_id` as a field; a call can never reach a different app's
  Flow or data; the concurrency/rate boundaries (4 in-flight/App, 8/
  conversation, 60 read calls/App/min, 10 mutations/tool/min, 5-minute
  timeout, §14.4) are actually exercised at their edges — pass **and**
  fail cases — not just documented as numbers.

## What already exists that a future QA pass will actually ride on

So the next author doesn't reinvent these: the generic `tools/list` /
`tools/call` round trip (`mcp/src/client.rs:500,571` — BUILT, already used
for any configured MCP server, just not yet pointed at a per-App logical
server, because none exists); the `McpToolResultDto` result shape (BUILT,
above); the `McpServerPolicy` permission rule source (BUILT, above).
Everything specific to a *generated* catalog — the logical server itself,
the three-layer digest (`approval_contract_sha256` / `tool_surface_sha256`
/ `catalog_sha256`, §16.1), the rate-limit/timeout enforcement, the
isolation boundary — is §12/§14/§15 and is design-only.

## Reading and reporting a result

When the mechanism does exist and a check runs, report per dimension: which
of the six passed or failed, and quote the specific Host evidence — the
`tools/list` entry, the rejected call's error code, the digest that didn't
match — not a paraphrase. A dimension with no evidence to cite is
"unverified," never silently folded into an overall "passed."

## Boundaries

- Never write "QA passed," a checklist of checkmarks, or a summary verdict
  from reading the proposal, the design doc, or this skill file — every
  dimension needs an actual Host call, result, or digest to point at.
- When the underlying mechanism doesn't exist yet — true for the whole
  per-App MCP subsystem today — say that plainly instead of fabricating a
  pass or testing something adjacent and calling it equivalent.
- Never fix a failing dimension yourself. A bad schema goes back to
  `$mcp-tool-design`; a bad binding goes back to `$mcp-flow-binding`; this
  skill's job ends at reporting what actually happened.
- Never treat a `mcp_authoring_required` or `needs_input` outcome from
  `$expose-as-mcp`'s workflow as something for this skill to resolve —
  those are pre-QA states; QA only evaluates a proposal that already
  cleared the tool-quality gate.
