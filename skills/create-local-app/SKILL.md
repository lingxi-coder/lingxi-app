---
name: create-local-app
description: Create, revise, preview, and operate LingXi local applications through the built-in local_apps MCP provider and the host design workflow. Use when a user asks to build a local app, dashboard, tracker, showcase, form utility, mini app, or to modify, inspect, run, restore, or debug an existing LingXi app.
---

# Create Local App

Use the host-owned designer and generator. Never create an app by writing an
arbitrary project directly into the mobile filesystem.

Treat the Rust `AppService` as authoritative. Follow only this persisted state
flow: `designer opened -> design confirmed -> scaffold -> generate -> validate
-> build -> preview -> user approved`.

## Create an app

1. Call `mcp__local_apps__list` and obtain the current template catalog. Do not
   hard-code template questions or choices.
2. Match the request to a template. If multiple templates fit, explain the
   smallest meaningful distinction and let the user choose in the designer.
3. Call `mcp__local_apps__create` with the selected template and the known
   intent. Treat success as “designer opened,” not “app generated.”
4. Guide the user through the host's five steps: basics, structure, data,
   appearance, then permissions and confirmation.
5. Use `mcp__local_apps__propose_design` when a concrete suggestion would help.
   Present its field-level diff. Never apply or dismiss a suggestion on the
   user's behalf.
6. Wait for explicit design confirmation. Do not use UI automation to click the
   confirmation action.
7. Follow generation progress with `mcp__local_apps__get`; use
   `mcp__local_apps__read_logs` when a job fails. Retry only the failed revision.
8. Open the preview after the generator reaches preview-ready. Ask the user to
   approve it or provide feedback; do not approve your own output.

## Modify or operate an app

- Resolve the app with `mcp__local_apps__list`, then load its details before
  changing data, runtime, code intent, or UI.
- Describe the intended revision and return to the designer for material schema,
  capability, or dependency changes.
- Use only structured collection requests for data. Never issue SQL.
- Inspect UI before acting. Use only structured click, fill, select, toggle,
  scroll, navigate, back, or reload actions. Never execute JavaScript.
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
`mcp__local_apps__get`, `mcp__local_apps__create`,
`mcp__local_apps__propose_design`, `mcp__local_apps__manage_runtime`,
`mcp__local_apps__query_data`, `mcp__local_apps__mutate_data`,
`mcp__local_apps__inspect_ui`, `mcp__local_apps__act_on_ui`,
`mcp__local_apps__read_logs`, `mcp__local_apps__list_checkpoints`, and
`mcp__local_apps__restore_checkpoint`. If it is unavailable, report that the
mobile host did not register it. Do not fall back to shell, remote MCP,
`.mcp.json`, direct filesystem mutation, or a development server.

The fixed scaffold is `next-static-v1`. Generated source may change only
`app/`, `components/`, `lib/`, `styles/`, and `public/`. Reject path traversal,
symbolic links, package installation, dependency edits, API routes, Server
Actions, external scripts, `eval`, arbitrary JavaScript UI actions, and direct
network calls. Use `window.lingxi.v1` for data, permitted network requests, and
runtime information.

Both store and full builds must remain static-export compatible. Full mode runs
the same source through the pinned production Next server; it does not grant
server-only application APIs.

Never automate final design confirmation, suggestion disposition, preview
approval, destructive schema migration, checkpoint restore, first data/UI
control permission, or first access to an external HTTPS domain.
