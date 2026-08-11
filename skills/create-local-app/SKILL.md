---
name: create-local-app
description: Orchestrate a confirmed local-app design, dependency proposal, React generation, offline build, and browser plus native WebView verification.
---

# Create a local app

This skill is the coordinator. Keep the product brief and confirmation short;
delegate specialist work to `$frontend-design`, `$accessibility`,
`$react-best-practices`, and `$frontend-qa` instead of duplicating their rules.

## Entry and confirmation

Inside an app's own workspace, read `LINGXI.md` and sharpen its brief. In a
global chat, gather the product, screens, data, capabilities, and visual intent
before creating the app. Ask focused questions in rounds, then show one
confirmable specification containing:

- target OS and form factor; if omitted, infer from the host device context
  (`os`, `formFactor`, `viewport`, `safeArea`, `colorScheme`, `reducedMotion`,
  and `inputMode`) and show that inference for confirmation;
- pages, navigation/back semantics, complete states, data/permissions;
- design direction, platform tokens, responsive/adaptive behavior, and the
  exact packages proposed for installation (including versions/specs);
- whether original raster imagery is required.

Do not silently add a package, capability, domain, platform, or image asset.
The same business logic may serve multiple targets, but each target must use a
platform adapter/tokens layer rather than a width-only conditional.

Create a new app with:

```json
{"brief":"<confirmed one-line brief>","name":"<display name>"}
```

using `mcp__local_apps__create`. Declare collections, domains, capabilities,
and the confirmed `device_context` with `mcp__local_apps__update_manifest`
before generated source relies on them. Inside an existing app, do not call
`create` again.

## Image assets (conditional)

Only when the brief needs an original photo, illustration, texture, hero,
background, or other bitmap, detect whether the built-in ImageGen skill/tool is
available. If available and configured, generate the asset into `public/` and
record prompt, source, and use in the spec. If unavailable, ask once whether
to guide installation/configuration or skip it; on skip, use CSS, gradients,
user assets, or an honest placeholder and do not ask again in this task. A
built-in path does not need an API key; a CLI/API fallback may require
`OPENAI_API_KEY`. After the user chooses setup, follow the environment's
install path, reload skills (`/reload-skills` or its equivalent), re-detect,
and enable immediately if ready. Lucide/SVG is sufficient for ordinary icons.

## Build orchestration

Call the `local-app-build` workflow once with the confirmed spec:

```json
{"name":"local-app-build","args":{"app_id":"<id>","spec":"<confirmed spec>"}}
```

The workflow is deterministic and owns these phases in order:

1. **Design** — invoke `$frontend-design`, produce/confirm platform and form
   factor, page structure, tokens, adapters, interactions, and asset decision.
   For a new app with no `package.json`, use the existing Mobile Linux
   `Shell`/Bash tool to run the official CLI in a newly created empty staging
   source root: `npm create vite@latest . -- --template react --no-interactive`.
   Use `--template react-ts` only when the confirmed specification explicitly
   requires TypeScript; the exact TypeScript command is
   `npm create vite@latest . -- --template react-ts --no-interactive`; do not
   make template choice interactive. The app
   workspace already contains host `.lingxi/` metadata, so never target that
   non-empty directory directly. Copy the completed staging contents into the
   still-empty source root only after checking that no user source exists.
   If registry/network access is unavailable, copy the repository-verified
   `.lingxi/vite-fallback/` into the source root as an explicit
   `offline-fallback`, record the reason, and report that the official CLI did
   not run. Do not use that fallback for unrelated CLI failures or add a Vite
   wrapper/scaffold API.
2. **Dependencies** — compare the confirmed package list with the app's
   package manifest using the existing `Shell` tool in the app workspace. Show
   the exact displayed specs, then run `npm install -- <specs>` or
   `npm uninstall -- <specs>` through Shell; its existing network/command
   approval and command logs cover network access and lifecycle scripts. After
   the official scaffold, run ordinary `npm install` in that same workspace
   when `node_modules` is absent or incomplete. After restore, run `npm ci`
   through the same Shell when the lockfile and installed tree differ. Optional
   `tailwindcss`/`@tailwindcss/vite`, `motion`, and `lucide-react` packages are
   valid proposals, but only install the exact specs shown and confirmed.
3. **Generate** — invoke `$accessibility` and `$react-best-practices`; inject
   the LingXi bridge, `deviceContext`, source policy/manifest integration,
   platform adapter, complete React source, and all required states under the
   editable roots (`src/` is the normal Vite source root; `app/` remains the
   explicit offline fallback root).
4. **Build** — call `mcp__local_apps__build`; the host runs the production
   `vite build`/`npm run build` equivalent offline with the fixed runtime and
   the app's projected, read-only dependencies. `npx vite` is for a Shell
   preview/development session only and never enables network in production
   verification.
5. **Verify** — invoke `$frontend-qa`. Use Browser when available for preview,
   screenshots, console, navigation, and core interactions, then use native
   WebView inspect/act/log tools for bridge, data, system back, and device
   context. Cover the phone/tablet matrix and desktop when supported.

On a verification finding, repair, rebuild, and re-verify at most twice. If
issues remain, return them with evidence and the reduced verification level;
never claim full Browser or native QA that was not run.

## Workspace and dependency boundary

Edit generated source only under `app/`, `src/`, `components/`, `lib/`,
`styles/`, and `public/`. The normal project workflow owns `package.json`,
lockfile, and `node_modules`; use the existing Shell tool in the app workspace
for `npm install`, `npm uninstall`, `npm ci`, `npm run build`, or `npx vite`,
never a new wrapper or MCP tool. The official Vite `index.html` and
`vite.config.*` remain host-controlled; `src/main.*` may be minimally adapted
to import the checked-in bridge/deviceContext/platform adapter. The build is
offline and only reads the installed app-local dependencies.
Use only `window.lingxi.v1` for host data, network, device, and agent events.
The build is offline and the bridge/device context is untrusted input: validate
it at the adapter boundary.

Source versioning uses the existing Git/Bash capability and its normal
workspace approval and command logs. Checkpoints are ordinary workspace Git
history plus the package-lock digest; do not introduce a second version store
or a checkpoint-specific command surface.

## Existing app operations

Use `mcp__local_apps__build`, `mcp__local_apps__manage_runtime`,
`mcp__local_apps__read_logs`, structured `mcp__local_apps__inspect_ui` /
`mcp__local_apps__act_on_ui`, `mcp__local_apps__query_data` /
`mcp__local_apps__mutate_data`, `mcp__local_apps__restore_checkpoint`, and
app-event tools as needed. Restore
checkpoints only after the user's explicit choice; a restore
must compare package-lock digests and show the difference before any `npm ci`
through the existing Shell tool.
Create a checkpoint only after the user approves the working preview.
