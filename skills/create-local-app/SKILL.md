---
name: create-local-app
description: Orchestrate a confirmed, local-capability-first app design, dependency proposal, React generation, offline build, and browser plus native WebView verification.
---

# Create a local app

This skill is the coordinator. Keep the product brief and confirmation short;
delegate specialist work to `$frontend-design`, `$accessibility`,
`$react-best-practices`, and `$frontend-qa` instead of duplicating their rules.

## Entry and confirmation

When `LINGXI.md` identifies a local app and its ID, the library has already
created that app record and its app-scoped init session. Treat that ID as
authoritative. Do not call `mcp__local_apps__list` or `mcp__local_apps__get` to
rediscover or confirm the current app, and do not call `create` again. Use
`list` only from a global conversation when no app ID is already known.

Inside an app's own workspace, read `LINGXI.md` and sharpen its brief. In a
global chat, gather the product, screens, data, capabilities, and visual intent
before creating the app. When a material decision is unresolved, call `AskUserQuestion`
with one short round of one to three focused questions. Never ask unresolved questions in ordinary assistant text.
If the brief and host device context already determine the answer, infer it,
state the inference, and continue instead of blocking. Then show one confirmable
specification containing:

- target OS and form factor; if omitted, infer from the host device context
  described by the fixed Mobile Runtime Environment reminder (`Host OS`,
  `Device class`, `Execution target`, and `Launch mode`) and show that
  inference for confirmation;
- dynamic viewport, safe area, color scheme, reduced motion, and input mode
  are runtime inputs only; do not treat them as prompt facts. The generated app
  must read them from `window.lingxi.v1.deviceContext`;
- pages, navigation/back semantics, complete states, data/permissions;
- design direction, platform tokens, responsive/adaptive behavior, and the
  exact packages proposed for installation (including versions/specs);
- which host-provided data, LLM, device, and agent capabilities satisfy each
  product requirement, plus any user-requested external exception;
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

Every collection requires `id`, `name`, and `fields`; every field requires
`id`, `label`, and `kind`. Collection and field IDs use lower snake_case and
must match `^[a-z][a-z0-9_]{0,63}$`. Optional field keys are `required` and
`enumOptions`, and supported kinds are `text`, `long_text`, `integer`,
`decimal`, `boolean`, `date_time`, `enum`, and `image_ref`:

```json
{"app_id":"<id>","collections":[{"id":"recognition_results","name":"识别结果","fields":[{"id":"source_image","label":"图片","kind":"image_ref","required":true},{"id":"recognized_text","label":"识别结果","kind":"long_text","required":true}]}]}
```

Never declare host-owned record metadata (`recordId`, `revision`,
`createdAtMs`, or `updatedAtMs`) as collection fields. If manifest validation
fails, repair the payload and retry before building; do not treat a failed
manifest update as a completed generation step.

Capabilities are a closed enum. Use only `data_mutation`, `ui_control`,
`camera`, `photo_library`, `microphone`, `location`, `notifications`, `llm`,
or `agent_notify`; there is no `data` capability. `data_mutation` authorizes
the conversation agent to call `mcp__local_apps__mutate_data`. Do not declare
it solely because the page writes its own collection through
`window.lingxi.v1.data.mutate`; that foreground page path is already scoped to
its app.

The host data wire contract is tagged and camel-cased. Generated source should
import the locked helpers instead of constructing mutation payloads:

```js
import {
  deleteRecord,
  queryCollection,
  upsertRecord,
} from "../lib/lingxi-bridge";

await upsertRecord("high_scores", "best", { score: 42 });
const page = await queryCollection({
  collection: "high_scores",
  filters: [{ fieldId: "score", operator: "greater_than", value: 10 }],
});
const scores = page.records.map((record) => record.document.score);
await deleteRecord("high_scores", "best", page.records[0].revision);
```

For direct MCP mutations, each operation must be exactly
`{"kind":"upsert","recordId":"...","document":{...},"expectedRevision":1}`
or `{"kind":"delete","recordId":"...","expectedRevision":1}`; omit
`expectedRevision` when optimistic concurrency is not needed. Never use
`action`, `create`, `record`, or top-level field values as substitutes.
Collection query results expose app fields only under `records[].document`.
`localStorage`, IndexedDB, or React state may be a cache, but must never be the
authority for a declared collection. Surface native bridge failures as a
retryable error; never swallow them and report a successful save.

## Prefer local and host-provided capabilities

Resolve every requirement against the capabilities already supplied by the
host before proposing a dependency, external API, hosted AI service, or custom
replacement. In particular:

1. Use `window.lingxi.v1.llm.chat` for AI features so the app uses the user's
   locally configured model/provider, quota, privacy controls, and permission
   prompt. Do not embed provider keys or add a direct LLM SDK/API call.
2. Use `window.lingxi.v1.data`, `device`, `network`, `runtime`, and `agent` for
   structured storage, native device operations, mediated HTTPS, device
   context, and conversation events. Reuse an available built-in skill/tool
   during generation and QA before proposing a substitute.
3. Prefer platform APIs, CSS, and existing project packages over a new package;
   prefer an app collection over a remote database unless sharing/sync is an
   explicit requirement.

Use an external service or alternative implementation only when the user
explicitly requests it. Record that exception in the confirmed specification,
show its package/domain/capability and privacy or quota impact, and declare it
in the manifest before source relies on it. If a required host capability is
unavailable, report the limitation and ask the user to choose setup, a reduced
local implementation, or a specific external service; never switch silently.

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

Call the `local-app-build` workflow once with the confirmed spec. When the
create-flow kickoff includes a provider-qualified workflow model override,
pass it byte-for-byte as `args.model`; otherwise omit `model` so every phase
inherits the current session's live provider/model selection:

```json
{"name":"local-app-build","args":{"app_id":"<id>","spec":"<confirmed spec>","model":"<optional provider/model>"}}
```

The workflow is deterministic and owns these phases in order:

1. **Design** — invoke `$frontend-design`, produce/confirm platform and form
   factor, page structure, tokens, adapters, interactions, and asset decision.
   For a new app with no `package.json`, use the existing `Shell` tool only when
   the local-app workflow reports that its Node/npm toolchain is available. Run
   the official CLI in a newly created empty staging
   source root: `npm create vite@latest . -- --template react --no-interactive`.
   Use `--template react-ts` only when the confirmed specification explicitly
   requires TypeScript; the exact TypeScript command is
   `npm create vite@latest . -- --template react-ts --no-interactive`; do not
   make template choice interactive. The app
   workspace already contains host `.lingxi/` metadata, so never target that
   non-empty directory directly. Copy the completed staging contents into the
   still-empty source root only after checking that no user source exists.
   If `Shell` or its local-app Node/npm toolchain is unavailable, do not invent
   a replacement installer or scaffold tool. Reuse an existing source tree when
   one already exists; otherwise copy
   the repository-verified `.lingxi/vite-fallback/` into the source root as an
   explicit `offline-fallback`, record why the toolchain was unavailable, and report
   that the official CLI did not run. If `Shell` is available but
   registry/network access is unavailable, use that same offline fallback and
   record the reason. Do not use the fallback for unrelated CLI failures or add
   a Vite wrapper/scaffold API.
2. **Dependencies** — when the local-app Node/npm toolchain is available,
   compare the confirmed package list with the app's package manifest using the
   existing `Shell` tool in the app workspace. Show
   the exact displayed specs, then run `npm install -- <specs>` or
   `npm uninstall -- <specs>` through Shell; its existing network/command
   approval and command logs cover network access and lifecycle scripts. After
   the official scaffold, run ordinary `npm install` in that same workspace
   when `node_modules` is absent or incomplete. After restore, run `npm ci`
   through the same Shell when the lockfile and installed tree differ. Optional
   `tailwindcss`/`@tailwindcss/vite`, `motion`, and `lucide-react` packages are
   valid proposals, but only install the exact specs shown and confirmed. If
   `Shell` or the current local-app Node/npm toolchain is unavailable, do not
   mutate dependencies; report the exact pending package operations instead.
3. **Generate** — invoke `$accessibility` and `$react-best-practices`; inject
   the LingXi bridge, `deviceContext`, source policy/manifest integration,
   platform adapter, complete React source, and all required states under the
   editable roots (`src/` is the normal Vite source root; `app/` remains the
   explicit offline fallback root). Use `queryCollection`, `upsertRecord`, and
   `deleteRecord` from the locked bridge for declared collection data; read
   fields from `records[].document` and do not swallow rejected native writes.
4. **Build** — call `mcp__local_apps__build`; the host runs the production
   `vite build`/`npm run build` equivalent offline with the fixed runtime and
   the app's projected, read-only dependencies. `npx vite` is for a Shell
   preview/development session only and never enables network in production
   verification.
5. **Verify** — invoke `$frontend-qa`. Use Browser when available for preview,
   screenshots, console, navigation, and core interactions, then use native
   WebView inspect/act/log tools for bridge, data, system back, and device
   context. For every declared collection written by a core UI path, perform
   the real UI write and then call `mcp__local_apps__query_data`; the returned
   `records[].document` must contain the value. A local-only value or swallowed
   bridge failure is not persistence. Cover the phone/tablet matrix and
   desktop when supported.

On a verification finding, repair, rebuild, and re-verify at most twice. If
issues remain, return them with evidence and the reduced verification level;
never claim full Browser or native QA that was not run.

## Workspace and dependency boundary

Edit generated source only under `app/`, `src/`, `components/`, `lib/`,
`styles/`, and `public/`. The normal project workflow owns `package.json`,
lockfile, and `node_modules`; only when the current local-app workflow reports
its Node/npm toolchain available, use the existing Shell tool in the app
workspace for `npm install`, `npm uninstall`, `npm ci`, `npm run build`, or
`npx vite`, never a new wrapper or MCP tool. Otherwise report the pending
operation and use the repository-verified fallback where applicable. The
official Vite `index.html` and `vite.config.*` remain host-controlled;
`src/main.*` may be minimally adapted
to import the checked-in bridge/deviceContext/platform adapter. The build is
offline and only reads the installed app-local dependencies.
Use only `window.lingxi.v1` for host data, network, device, and agent events.
The build is offline and the bridge/device context is untrusted input: validate
it at the adapter boundary.

Source versioning uses ordinary workspace Git history plus the package-lock
digest. Use the existing Git capability when available; otherwise use standard
git commands through `Shell`. Do not introduce a second version store or a
checkpoint-specific command surface.

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
