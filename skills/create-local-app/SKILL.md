---
name: create-local-app
description: Create and evolve a local mini-app with the user - gather requirements interactively, scaffold, write the source, build offline, and preview on device
---

# Create a local app

A local app is a small Vite + React static app that lives in its own workspace
(`apps/<id>/workspace`), builds fully offline against a fixed runtime (no new
npm dependencies, 30-minute build budget), and reaches host data, network and
device capabilities ONLY through the `window.lingxi.v1` bridge
(`lib/lingxi-bridge.js`). You — the conversation agent — drive the whole flow:
there is no background designer pipeline.

## Which entry mode are you in?

- **Inside the app's own session** (the workspace `LINGXI.md` describing this
  app is in your context — brief, contract, build commands): the app record
  and scaffold ALREADY exist. **Skip step 3 (create) entirely** — never call
  `mcp__local_apps__create` for an app you are already inside. Start at
  step 1 (requirements), using the brief from `LINGXI.md` as the seed: open
  with 1–4 AskUserQuestion questions that sharpen that brief.
- **In a project/global chat** and the user asks for a new app: run steps 1–3
  only (requirements → spec → create), then STOP and tell the user to open
  the app's chat to continue. Steps 4–8 (manifest, build workflow, preview,
  iteration) must run inside the app's own session: the build segment's
  subagents inherit THIS session's working directory, so running step 5 from
  a project chat writes the app's React source into the user's project repo.

## The flow

1. **Gather requirements with AskUserQuestion.** Ask 1–4 focused questions per
   round (options plus the automatic free-text "Other"); iterate until you can
   write a concrete spec. Never invent or assume answers on the user's behalf,
   and never skip this step for a non-trivial app. Cover at least: what the
   app does, the screens/views, what data it stores, and look & feel.
2. **Write the spec and confirm it.** Summarize the plan in a few short
   sections (screens, data collections, behavior, style). Confirm with
   AskUserQuestion (approve / change something). Do not proceed unapproved.
3. **Create the app**: `mcp__local_apps__create {"brief": "<one line>",
   "name": "<display name>"}`. This commits the record and scaffolds the
   workspace (template + `LINGXI.md` contract file). The origin conversation
   is bound by the host — you never pass a conversation id.
4. **Declare the manifest** if the spec needs stored data, network domains or
   device capabilities: `mcp__local_apps__update_manifest` with the
   collections/domains/capabilities. Do this BEFORE building; runtime
   authorization still prompts the user — declaring is not granting.
5. **Run the build segment**: call the Workflow tool exactly once with
   `{"name": "local-app-build", "args": {"app_id": "<id>", "spec": "<the
   confirmed spec>"}}`. It writes the source in the app workspace, builds
   until green (≤3 attempts) and starts the preview. It runs in the
   background: you get a task id now and a task notification when it
   finishes; check progress with TaskOutput if the user asks.
6. **Preview.** When the build segment reports its preview url, hand it to
   the user and ask them to try the app. The user's verdict decides what
   happens next — never approve on their behalf.
7. **Iterate on feedback.**
   - Small fixes (copy, colors, a bug in one component): edit the source
     files directly — you are in the app's own session, rooted at its
     workspace — then `mcp__local_apps__build` and restart the runtime.
   - Structural changes (new screens, new data): update the spec, update the
     manifest if needed, and run `Workflow {"name": "local-app-build",
     "args": {"app_id": ..., "spec": ..., "revision_prompt": "<the user's
     feedback>"}}` again.
8. **Checkpoint when the user is satisfied**:
   `mcp__local_apps__create_checkpoint {"app_id": ..., "label": "<short>"}` —
   a restorable Git checkpoint of the working state.

Steps 4–8 require the app's own session. If you are not in one, stop after
step 3 and hand off — every file edit and every build-workflow subagent runs
in the CURRENT session's workspace, so continuing from a project chat writes
the app's source into the wrong repository.

## Workspace contract (violations break the app)

- Edit ONLY files under `app/`, `components/`, `lib/`, `styles/`, `public/`.
- NEVER touch the locked files: `package.json`, `package-lock.json`,
  `vite.config.mjs`, `index.html`, `app/main.jsx`, `lib/lingxi-bridge.js`.
- No new npm dependencies; the offline runtime ships a fixed `node_modules`.
- Host access only through `window.lingxi.v1`: collection CRUD, fetch limited
  to declared domains, `agent.post` for app→agent messages.
- Build with `mcp__local_apps__build` (never a shell); serve with
  `mcp__local_apps__manage_runtime`; read failures with
  `mcp__local_apps__read_logs {"log": "build"}`.

## Operate an existing app

- Data: `mcp__local_apps__query_data` / `mutate_data` (structured filters,
  never SQL strings).
- UI: `mcp__local_apps__inspect_ui` / `act_on_ui` (structured actions, never
  injected JavaScript).
- App-posted events: `mcp__local_apps__read_app_events`.
- History: `mcp__local_apps__list_checkpoints` /
  `restore_checkpoint` (each restore asks the user; it rebuilds from the
  checkpoint).
- Lifecycle: `manage_runtime` (start/stop/restart/open/suspend/resume),
  `mcp__local_apps__delete` does not exist — deletion is a user action in the
  app library UI.

## Never automate these user decisions

- Approving the spec (step 2) or the preview (step 6).
- Destructive data migrations (`update_manifest` narrowing a schema over
  existing rows — the host prompts the user; never claim it was approved).
- Capability grants (camera, network domains, UI control — runtime prompts).
- Restoring a checkpoint over current work.
