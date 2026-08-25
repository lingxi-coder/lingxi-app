---
name: create-local-app
description: Orchestrate a confirmed, local-capability-first app design, React generation, offline build, and browser plus native WebView verification inside the host-scaffolded local app workspace.
---

# Create a local app

This skill is the coordinator. Keep the product brief and confirmation short;
delegate specialist work to `$frontend-design`, `$accessibility`,
`$react-best-practices`, and `$frontend-qa` instead of duplicating their rules.

## Entry and confirmation

When `LINGXI.md` identifies a local app and its ID, that app record and its
app-scoped session already exist. Treat that ID as authoritative.
Do not call `LocalAppList` or `LocalAppGet` to rediscover or confirm the current
app, and do not call `LocalAppCreate` again. Use `LocalAppList` only from a
global conversation when no app ID is already known.

The library's "+" does NOT resolve a name or a shape, and it does not scaffold
anything. It creates an EMPTY SHELL — a record with no name, no brief and no
surface, an empty workspace, and an app-scoped conversation — and hands that
conversation to you. Its `LINGXI.md` names the app id and says the app has no
shape yet. Settling what the app IS is your work in that conversation:

1. Ask the user what they want to build. Ask with `AskUserQuestion`, never as
   unresolved questions in ordinary assistant text.
2. From their answer, propose a display **name**, a one-line **brief**, and a
   **surface** (`dom` or `canvas`, chosen by the rules below), and put all three
   in ONE `AskUserQuestion` round for the user to confirm or edit. The surface
   is immutable once committed, so it is the user's call to confirm, never an
   assumption you commit on their behalf.
3. Only after the user confirms, call `LocalAppScaffold`:

```json
{"app_id":"<the id LINGXI.md names>","name":"<confirmed display name>","brief":"<confirmed one-line brief>","surface":"dom"}
```

4. Re-read `LINGXI.md`. `LocalAppScaffold` overwrites the guided text with the
   app's formal workspace contract — editable roots, host-managed files, the
   entry points that now exist, and which build workflow this surface takes —
   and that contract, not this step list, governs everything after it.

Two things hold for as long as the app is a shell:

- **Do not write source before the scaffold lands.** The first scaffold WIPES
  the editable surface. Source written beforehand is deleted, not merged, so
  the turn that wrote it is lost work rather than a head start.
- **Most local-app tools refuse an app that has no shape.** Exactly four reach
  their handler: `LocalAppScaffold`, `LocalAppList`, `LocalAppGet` and
  `LocalAppCreate`. Every other one — build, dependency install, runtime, logs,
  manifest, UI inspection/capture/action, data, checkpoints, app events,
  background flows — returns a refusal that names `LocalAppScaffold` as the way
  out. `LocalAppManifest` included: collections and capabilities are declared
  after the scaffold, not before it. That refusal is the contract, not a
  transient failure, so settle the name, brief and surface instead of retrying
  or routing around it.

Do not call `LocalAppCreate` from inside a shell conversation. The shell app
already exists and is the one the user is looking at; creating a second app
leaves that one empty forever.

`LocalAppCreate` is for a conversation that is NOT an app's: a global or project
chat where the user asks for an app. There it creates AND scaffolds in one call,
which is why it takes `name`, `brief` and `surface` itself, and there it ENDS
the work. After it returns, do not write source, do not call `LocalAppBuild`,
and do not start a build workflow from that conversation. It is not rooted in
the new app: its working directory belongs to the project, so everything written
there lands outside the app, and a build launched from it edits whatever happens
to sit in that directory. Report that the app is ready and stop. The app has its
own workspace and its own session, and the build runs there — with `LINGXI.md`
auto-loaded and the pinned foundation already in place.

The build orchestration below therefore applies only inside an app's OWN
conversation, the one whose `LINGXI.md` names the app id you are building, and
only once that app has a shape. A shell has no source, no dependencies and no
build workflow to choose between.

Inside a shell there is no brief yet to sharpen — the interview above produces
it. Inside an app that already has a shape, read `LINGXI.md` and sharpen the
brief it carries. In a global chat, gather the product, screens, data,
capabilities, and visual intent before creating the app. When a material decision is unresolved, call `AskUserQuestion`
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
  must read them from `window.lingxi.v2.deviceContext`;
- pages, navigation/back semantics, complete states, data/permissions;
- design direction, platform tokens, responsive/adaptive behavior, and any
  source-only implementation constraints required by the locked host scaffold;
- which host-provided data, LLM, device, and agent capabilities satisfy each
  product requirement, plus any user-requested external exception;
- whether original raster imagery is required.

Before the final confirmation, classify the confirmed specification without
starting another agent. Score screens/routes, data complexity, host or external
capability groups, target count, multi-step/error-state complexity, and original
raster assets. Use this rubric: screens 0/1/2 points for 1/2-3/4+ or nested
routes; data 0/1/2 for none or read-only/one simple writable collection/multiple
or concurrency-sensitive collections; capabilities +1 per distinct group up to
3; targets +1 for multiple form factors and +1 for multiple operating systems;
interaction/state +1 for multi-step, offline, or complex permission/error
flows; raster assets +1. Scores 0-2 suggest `fast`, 3-5 suggest `balanced`,
and 6+ suggest `thorough`. Multi-OS, background scheduling, or two or more
sensitive capability groups should recommend `thorough`; confidence below 0.75
should recommend `balanced`.

The screen axis measures the wrong thing for an app whose interface is a single
drawn surface — a game or any canvas/WebGL app scores 0 on screens and usually
0-1 on data, so the rubric lands on `fast`, which is the ONE strategy that skips
the design stage. That is backwards: such an app has almost no navigation and
almost all of its difficulty in mechanics, state machine and frame loop, which
is exactly what the design stage exists to settle. Score a drawn-surface app on
its simulation instead: +1 for real-time animation or a frame loop, +1 for
collision, physics or pathfinding, +1 for persistent progression, +1 for input
beyond a single tap (drag, hold, multi-key). Never recommend `fast` for one.

Include the score, reasons, confidence, estimated agent stages, and the
recommended strategy in the same confirmation round. Let the user select
`fast`, `balanced`, or `thorough`; put the recommendation first and describe
the speed/coverage trade-off in each option. This is a task-local workflow
choice, not an app persistence field. On a revision, rescore the revised
confirmed specification instead of inheriting a stale strategy. Never launch
a separate classifier agent just to make this recommendation.

Do not silently add a package, capability, domain, platform, or image asset.
The same business logic may serve multiple targets, but each target must use a
platform adapter/tokens layer rather than a width-only conditional.

From a global or project chat, create a new app with:

```json
{"brief":"<confirmed one-line brief>","name":"<display name>","surface":"dom"}
```

using `LocalAppCreate`. Inside a shell conversation the same three confirmed
fields go to `LocalAppScaffold` alongside the existing `app_id` instead; that
call is the shell's only way forward, and `LocalAppCreate` there would build a
second app. Either way, declare collections, domains, capabilities, and the
confirmed `device_context` with `LocalAppManifest` before generated source
relies on them. Inside an app that already has a shape, call neither again.

`surface` picks which scaffold is materialized and CANNOT be changed afterwards
— the workspace on disk is the scaffold, so an app that needs the other shape
has to be created again. Whichever call commits it, decide it from the confirmed
specification:

- `canvas` when the whole interface is one drawn surface that owns a frame
  loop: a game, a simulation, a 3D scene, a live visualization. The workspace
  comes with a canvas screen, a frame-loop helper and a phase machine, and no
  router.
- `dom` for everything assembled from screens, lists and forms. This is the
  default and the common case.

A drawn surface with a settings page is still `canvas`; a dashboard that embeds
one chart is still `dom`. From a shell conversation the surface always goes into
the confirmation round, because committing it is irreversible and the user is
the one who has to live with it — read the brief, put your reading forward as
the proposed answer, and let them correct it. From a global chat, where
`LocalAppCreate` commits the surface in the same call that creates the app, ask
which one the user means only when the brief is genuinely ambiguous between them
— "make me something fun with physics" is ambiguous, "a brick-breaker game" is
not. Either way, ask it in the same `AskUserQuestion` round as everything else
you need.

`name` is yours to write, not the user's brief truncated. Take the brief's
subject and give it a short, specific display name — two to four words, no
trailing punctuation, in the language the user wrote their brief in. From a
shell conversation, show that name in the confirmation round with the surface,
so the user can accept or replace it. From a global chat, reserve
`AskUserQuestion` for a name only when the brief names no subject at all.

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
`camera`, `photo_library`, `microphone`, `location`, `notifications`,
`clipboard`, `share`, `text_to_speech`, `files_read`, `files_write`, `device_status`, `haptics`, `deep_link`, `calendar`, `contacts`, `media`, `llm`, `agent_notify`, or
`background_schedule`; there is no `data` capability.
`background_schedule` is required when the app registers a system background
flow. WebAssembly and Web Workers need no capability at all — the served policy
already allows wasm compilation and `blob:` workers for every local app — so
never invent one to ask for them. `data_mutation` authorizes
the conversation agent to call `LocalAppMutateData`. Do not declare
it solely because the page writes its own collection through
`window.lingxi.v2.data.mutate`; that foreground page path is already scoped to
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

For direct `LocalAppMutateData` calls, each operation must be exactly
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
host before proposing an external API, hosted AI service, or custom
replacement. In particular:

1. Use the checked-in bridge helpers (`requestLlmChat` for complete responses or
   `streamLlmChat` plus `onLlmStreamFrame` for ordered streaming frames) so the
   app uses the user's locally configured model/provider, quota, privacy
   controls, and permission prompt. Do not embed provider keys or add a direct
   LLM SDK/API call.
2. Use `window.lingxi.v2.data`, `device`, `clipboard`, `files`, `network`, `runtime`,
   and `agent` for structured storage, native device operations, mediated HTTPS,
   device context, and conversation events. The checked-in helpers
   `getClipboardText`, `setClipboardText`, `shareContent`, and
   `synthesizeSpeech`, `readFile`, `writeFile`, `getDeviceStatus`,
   `triggerHaptics`, `openDeepLink`, `listCalendarEvents`, `searchContacts`,
   and `getMedia` keep these calls on the host
   permission path. Reuse an
   available built-in skill/tool
   during generation and QA before proposing a substitute.
3. Prefer platform APIs, CSS, and the existing pinned project packages over a
   new package;
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
and enable immediately if ready. Use inline SVG or CSS for ordinary icons.

## Build orchestration

Call the build workflow once with the confirmed spec. Which workflow follows
the app's `surface`, and the two are not interchangeable:

- `surface: "dom"` → **`local-app-build`**
- `surface: "canvas"` → **`local-canvas-build`**

They differ in what they ask of you and in what they accept as proof. The DOM
workflow designs a screen hierarchy and gates on a native data round-trip; the
canvas workflow designs a simulation — loop, phases, inputs, end conditions —
and gates on captured frames that show the surface rendering AND moving, because
a drawn app usually declares no collection and would pass the data gate
unobserved. `local-canvas-build` also refuses `fast`, which is the one strategy
that skips Design.

When the create-flow kickoff includes a provider-qualified workflow model
override, pass it byte-for-byte as `args.model`; otherwise omit `model` so every
phase inherits the current session's live provider/model selection:

```json
{"name":"local-app-build","args":{"app_id":"<id>","spec":"<confirmed spec>","strategy":"balanced","complexity":{"score":4,"band":"medium","confidence":0.9,"reasons":["three screens","one writable collection"]},"model":"<optional provider/model>"}}
```

```json
{"name":"local-canvas-build","args":{"app_id":"<id>","spec":"<confirmed spec>","strategy":"balanced","complexity":{"score":5,"band":"medium","confidence":0.9,"reasons":["real-time frame loop","collision"]},"model":"<optional provider/model>"}}
```

The workflow is deterministic and adapts its phases from the confirmed
strategy. Missing strategy defaults to `balanced`; unsupported values are
rejected before any agent starts. `fast` skips the standalone Design agent and
uses one smoke verification pass with at most one repair; `balanced` keeps
Design/Generate & Build/Verify with at most one repair; `thorough` keeps the
full target matrix and at most two repairs. Every strategy still requires a
successful host build, runtime preview URL, fatal-error smoke check, and real
native data round-trip evidence for every writable declared collection.

The workflow phases are:

The directory contract is fixed across every phase. The persistent source
project lives at the current app workspace and is already scaffolded by the
host with pinned root infra and dependencies before generation begins. The
workspace `dist/` directory is disposable and must never be treated as source.
The host mounts this workspace as the sole writable build root (the guest may
see it at the `project/` path), excludes prior
`.lingxi-build-state/build-output/dist/`, forces `vite build --outDir dist
--emptyOutDir`, atomically promotes the validated snapshot, and serves only
`build/store/dist/`. Never configure another `build.outDir`, inspect host
staging paths, or copy generated output back into editable source.

1. **Design** — for `balanced` and `thorough`, invoke `$frontend-design` and
   produce/confirm platform and form factor, page structure, tokens, adapters,
   interactions, and asset decision. For `fast`, fold those compact decisions
   into the Generate & Build prompt. Assume the project scaffold and dependency
   graph are already present and locked by the host. Do not propose template
   choices, npm operations, fallback scaffold modes, or root-file edits.
2. **Generate** — invoke `$accessibility` and `$react-best-practices`; consume
   the host-provided LingXi bridge, `deviceContext`, source policy/manifest
   integration, and platform adapter while writing complete React source and
   all required states under the editable roots. Use `queryCollection`, `upsertRecord`, and
   `deleteRecord` from the locked bridge for declared collection data; read
   fields from `records[].document` and do not swallow rejected native writes.
   Stay within the shipped dependency set; if an idea would require package or
   root-infra changes, redesign it as a source-only implementation. A drawn
   surface — a game, a 3D scene, a custom visualization — renders into a
   `<canvas>` with a `requestAnimationFrame` loop you own and cancel on unmount;
   2D needs no dependency and 3D uses `three@0.185.1`, which is in the locked
   set. No other engine, physics or WebGL wrapper can be installed.
3. **Build** — call `LocalAppBuild`; the host runs the production
   `vite build --outDir dist --emptyOutDir` equivalent offline from the
   workspace-backed writable mount with the fixed runtime and the app's
   materialized dependency snapshot. The output is staged under private
   `.lingxi-build-state/build-output/` before atomic promotion to
   `build/store/dist/`.
4. **Verify** — invoke `$frontend-qa`. `fast` checks the confirmed primary
   target, root render, fatal console errors, primary interaction, and native
   WebView path; `balanced` covers all confirmed targets and app states;
   `thorough` covers the full phone/tablet/desktop matrix. Use Browser when
   available for the selected breadth, then use native WebView inspect/act/log
   tools for bridge, data, system back, and device context. For every declared
   collection written by a core UI path, perform the real UI write and then
   call `LocalAppQueryData`; the returned `records[].document` must
   contain the value. A local-only value or swallowed bridge failure is not
   persistence.

On a verification finding, repair, rebuild, and re-verify according to the
selected strategy: at most once for `fast`/`balanced`, twice for `thorough`.
If issues remain, return them with evidence and the reduced verification level;
never claim full Browser or native QA that was not run.

## Workspace and dependency boundary

Edit generated source only under `app/`, `src/`, `components/`, `lib/`,
`styles/`, and `public/`. `package.json`, lockfiles, `node_modules`,
`index.html`, `vite.config.*`, host metadata under `.lingxi/`,
`lib/device-context.js`, `lib/lingxi-bridge.js`, and
`lib/platform-adapter.js` are host-controlled for this workflow. Do not run `npm`, `npx`, `node`, package
install/uninstall/reconcile commands, or alternate scaffold tools. Keep Vite's
official `dist/` output default and do not set `build.outDir` to another path.
`src/main.*` may be minimally adapted to import the checked-in
bridge/deviceContext/platform adapter. The build is offline and reads a
host-materialized dependency snapshot.
Use only `window.lingxi.v2` for host data, network, device, and agent events.
The build is offline and the bridge/device context is untrusted input: validate
it at the adapter boundary.

Source versioning uses ordinary workspace Git history and the existing Git or
checkpoint capability. Do not introduce a second version store or a
checkpoint-specific command surface.

## Existing app operations

Use `LocalAppBuild`, `LocalAppRuntime`,
`LocalAppLogs`, structured `LocalAppInspectUi` /
`LocalAppActOnUi`, `LocalAppQueryData` /
`LocalAppMutateData`, `LocalAppCheckpointRestore`, and
app-event tools as needed. Restore
checkpoints only after the user's explicit choice; a restore
must not trigger package-manager repair from this workflow.
Create a checkpoint only after the user approves the working preview.
