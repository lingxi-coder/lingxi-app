---
name: create-local-app
description: Create, revise, preview, and operate LingXi local applications through the built-in local_apps MCP provider and the host design workflow. Use when a user asks to build a local app, dashboard, tracker, showcase, form utility, mini app, or to modify, inspect, run, restore, or debug an existing LingXi app.
---

# Create Local App

Use the host-owned designer and generator. Never create an app by writing an
arbitrary project directly into the mobile filesystem.

Treat the Rust `AppService` as authoritative. Follow only this persisted state
flow: `authoring questionnaire -> collecting spec -> planning -> design
confirmed -> generate -> validate -> preview -> user approved`, with `revise`
looping back from preview or ready into a new generate/validate/preview pass.

## Create an app

1. Call `mcp__local_apps__create` with a `brief` — the user's own one-line
   description, in their words. Do not invent a template, a name, or a
   feature list. Success means "the host is authoring a questionnaire," not
   "an app exists."
2. The host asks the LLM for a questionnaire tailored to that brief and opens
   the designer. Follow progress with `mcp__local_apps__get`.
3. The user answers the questionnaire themselves. Relay what the app will do
   and what is still unanswered; do not fill the answers on their behalf.
4. Use `mcp__local_apps__propose_design` when a concrete suggestion would
   help. Present its field-level diff. Never apply or dismiss a suggestion on
   the user's behalf.
5. After the answers are in, the host derives a plan (data collections,
   capabilities, domains) and opens the design confirmation gate. Explain the
   plan in plain language — especially anything the user left to the model's
   discretion. Wait for explicit confirmation. Never automate that tap.
6. Follow generation with `mcp__local_apps__get`; use
   `mcp__local_apps__read_logs` when a job fails.
7. Open the preview after the generator reaches preview-ready. Ask the user to
   approve it or describe what to change; do not approve your own output.

## Keep improving an app

Generation is not one-shot. When the user describes a change in their own
words, call `mcp__local_apps__revise` with that description as `prompt`. The
app rebuilds and the preview gate re-opens; the user still approves it. There
is no limit on how many times this repeats, and every pass writes a
checkpoint that can be restored.

`revise` only reworks code inside the plan that was already confirmed — it
cannot add a data collection, capability, or external domain the confirmed
plan doesn't already declare. If the user asks for something that needs a
different plan (a new kind of data, a new external dependency), there is no
tool that reopens design on an existing app: return to
`mcp__local_apps__create` for a genuinely different app.

Do not try to change an existing app's brief — that discards every answer the
user gave and is theirs to trigger, not yours.

## Modify or operate an app

- Resolve the app with `mcp__local_apps__list`, then load its details with
  `mcp__local_apps__get` before changing data, runtime, or UI.
- Describe the intended change in the user's own words and call
  `mcp__local_apps__revise`; it works within the app's already-confirmed data
  model and capabilities, not beyond them.
- Use only structured collection requests for data
  (`mcp__local_apps__query_data`, `mcp__local_apps__mutate_data`). Never issue
  SQL.
- Inspect UI with `mcp__local_apps__inspect_ui` before acting. Use only
  structured click, fill, select, toggle, scroll, navigate, back, or reload
  actions via `mcp__local_apps__act_on_ui`. Never execute JavaScript.
- Let the host present first-use capability prompts. Do not weaken, bypass, or
  synthesize user approval.
- Before a checkpoint restore, explain that code will roll back while the app
  database remains unchanged. Require the host confirmation every time.

## Report progress

Report the current workflow state, revision, and next user-visible gate. For a
failure, include the job phase and a concise diagnostic from the build logs.
Do not report an app as created until preview approval has completed.

## Enforce the host contract

Use only the built-in provider: `mcp__local_apps__list`,
`mcp__local_apps__get`, `mcp__local_apps__create`, `mcp__local_apps__revise`,
`mcp__local_apps__propose_design`, `mcp__local_apps__manage_runtime`,
`mcp__local_apps__query_data`, `mcp__local_apps__mutate_data`,
`mcp__local_apps__inspect_ui`, `mcp__local_apps__act_on_ui`,
`mcp__local_apps__read_logs`, `mcp__local_apps__list_checkpoints`, and
`mcp__local_apps__restore_checkpoint`. If it is unavailable, report that the
mobile host did not register it. Do not fall back to shell, remote MCP,
`.mcp.json`, direct filesystem mutation, or a development server.

The fixed scaffold is `next-static-v1`, and it is a hard constraint the
generating model itself works under, not just a review checklist. Generated
source may change only `app/`, `components/`, `lib/`, `styles/`, and
`public/`. Reject path traversal, symbolic links, package installation,
dependency edits, API routes, Server Actions, external scripts, `eval`,
arbitrary JavaScript UI actions, and direct network calls. Use
`window.lingxi.v1` for data, permitted network requests, and runtime
information.

Both store and full builds must remain static-export compatible. Full mode runs
the same source through the pinned production Next server; it does not grant
server-only application APIs.

Never automate final design confirmation, suggestion disposition, preview
approval, destructive schema migration, checkpoint restore, first data/UI
control permission, or first access to an external HTTPS domain.
