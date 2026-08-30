---
name: expose-as-mcp
description: Tell apart a first per-App MCP authoring pass from an update or a standalone revise, and start the local-app-mcp-authoring workflow — never designs, approves, or promotes a tool.
---

# Expose a local app as MCP tools

Own exactly two things: classify which of three authoring situations a
request is, and start the `local-app-mcp-authoring` workflow with the two
inputs the design allows for it. Everything the workflow does after
that — turning a workflow into a tool definition, binding it to a Flow,
evaluating whether it actually works, registering or promoting anything —
belongs to `$mcp-tool-design`, `$mcp-flow-binding`, `$mcp-qa`, or to
Host-owned validation this skill never performs itself.

## This subsystem does not exist in code yet

Per-App MCP authoring is §12 of the plugin's frozen design
(`docs/local-apps/LOCAL-APP-PLUGIN-DESIGN-V2.md`), scheduled for Phase 6
("Per-App MCP authoring 与 persistence") and Phase 7 ("Logical server、
registry 与 listChanged"). As of this writing there is no
`agents/mcp-designer.md`, no `workflows/local-app-mcp-authoring.js`, and no
`AppMcpProposal` / `McpToolDefinitionDto` / `McpPermissionCeiling` type
anywhere in the plugin package or the Rust host. Everything below describes
the contract exactly as §12 specifies it, not code you can inspect today.
Naming the workflow through the `Workflow` tool right now will fail because
the workflow script doesn't exist; run this the moment it lands, and never
report having run it before then.

## The three situations

The manifest field that would settle this — schema v3's
`active_mcp_catalog: Option<AppMcpCatalogRef>` (§16.1) — doesn't exist
either: today's `AppManifest` (`local-apps/src/manifest.rs:354`) has no MCP
catalog field at all, only `runtime_profile` / `dependency_snapshot` /
`surface`. Once schema v3 lands, classify by what that field holds; until
then, treat this as the intended logic to implement against, not something
to query today:

- **Initial** — the app has no active catalog yet (`active_mcp_catalog` is
  `None`). The proposal folds into the single unified create-confirmation
  receipt (§17.3) — there is no separate MCP approval sheet at create time.
- **Update** — the app already has an active catalog, and the request came
  from changing the app itself (source, data schema, Flow, dependencies).
  If the resulting `approval_contract_sha256` (§12.2 — full tool surface,
  semantic Flow references, permission ceiling) is unchanged from the
  active catalog, no new MCP approval is created at all; if it changed, the
  Native proposal diff (§17.4a) must be shown before anything promotes.
- **Standalone revise** — the app already has an active catalog, and the
  user asked to add, change, or remove tools without otherwise touching the
  app. Same proposal-diff / approval-receipt path as update (§12.2), run on
  its own rather than folded into an app-update transaction.

## Starting the workflow

```text
{"name": "local-app-mcp-authoring", "args": {"app_id": "<id>", "user_goal": "<text>"}}
```

through the `Workflow` tool — the same generic invocation `$local-app-test`
already uses for `local-app-use-test`. Per §12.2 the workflow accepts
**only** `app_id` and `user_goal`; the Host derives everything else
(evidence, active build/catalog identity, capability graph) on its own.
Never pass raw tool definitions, a server name, annotations, permission
rules, a Flow ID, a workspace path, or a catalog/proposal digest — §12.2
names all six as explicitly rejected inputs, not merely unnecessary ones.

## Reading what comes back

The workflow can return `needs_input` (relay the focused questions to the
user and resume — don't guess an answer for them) or
`mcp_authoring_required` (§1.7 / §12.3 — no proposed tool cleared the
quality gate; the candidate/staging work is preserved, nothing is
published, and the user can keep clarifying what they want the LLM able to
do). Neither is a failure of this skill; both are the workflow doing its
job and handing control back.

## Boundaries

- Never construct an `AppMcpProposal`, decide a tool's schema, or judge
  whether a name or description is meaningful — that's `$mcp-tool-design`'s
  job, run inside the workflow, not this skill's.
- Never bind a tool's input or output to a Flow step — that's
  `$mcp-flow-binding`'s job, also run inside the workflow.
- Never declare a proposal QA-passed, register a logical server, or promote
  a catalog yourself. Even once the Host-owned pieces exist, only the
  workflow's own atomic promote (§12.2, §16.4) does that, gated on Native
  approval and MCP QA — never on this skill's judgment.
- Never invent an `app_id` or a `user_goal` on the user's behalf; if either
  is missing or unclear, ask rather than guessing one to unblock the call.
