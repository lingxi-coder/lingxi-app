---
name: create-local-app
description: Orchestrate confirmed local-capability-first app design, React generation, offline build, and Browser/native WebView verification inside the host-scaffolded workspace.
---

# Create a local app

This skill is the coordinator. Keep the product brief and confirmation short;
delegate specialist work to `$frontend-design`, `$accessibility`,
`$react-best-practices`, and `$frontend-qa` instead of duplicating their rules.
The build workflow routes DOM implementation through `$ionic-react-local-app`
and routes a drawn surface through the persisted runtime-profile-matched
specialist: `$canvas-2d-local-app`, `$threejs-local-app`,
`$phaser-2d-local-app`, or `$babylon-3d-local-app`.

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

1. Open in ORDINARY ASSISTANT TEXT: ask what they want to build, in one short
   open question, and wait for their answer. Do NOT use `AskUserQuestion` here.
   That tool renders a picker, and on this turn you know nothing about the app,
   so every option you could put in it is a guess at the user's own idea —
   handing them a menu of your guesses replaces the description you actually
   need. Their words in their own phrasing are the input to everything below,
   and no option list can collect them. This one opening turn is the single
   deliberate exception to the rule further down about unresolved questions in
   ordinary text; from step 2 onward that rule holds without exception.
2. Read what they wrote and settle whatever it already settles. Infer every
   point the description and the host device context determine, state the
   inference, and move on — do not ask back something they have already told
   you. Ask at most ONE clarification `AskUserQuestion` round: omit the round
   if none are material; otherwise ask 1-3 focused questions, never padding a
   quota, then use the answers in step 3.
3. Call `LocalAppRuntimeProfiles`, then propose a display **name**, a one-line
   **brief**, and one available **runtime profile**. Show its derived surface,
   core engine, revision, and recommendation reason in the same final
   confirmation. Profile family is immutable once committed, so it is the
   user's call to confirm.
4. After the conversational confirmation, call the unified
   `lingxi-local-app:local-app-build` create branch for this shell app. That
   Host-owned path re-reads the template catalog, stages the create candidate,
   shows one trusted native create confirmation, and only then calls
   `LocalAppScaffold`. Pass the display **name** and **brief** the user just
   confirmed in step 3 verbatim: they are staged with the create candidate, they
   are what the native create confirmation sheet renders, and they are what the
   Host commits onto the record — the shell app created in step 1 is still the
   empty `untitled` placeholder, so a create launched without them cannot show
   or commit the confirmed wording. App creation never runs MCP authoring: the
   app-owned MCP remains unconfigured and disabled until the user explicitly
   starts MCP setup from that app's settings. Never call a
   standalone runtime-profile selector or pass a model-authored
   surface/profile override. Do not supply `args.runtime_profile` as an authority; the host reads the materialized manifest and overwrites it:

```json
{"name":"lingxi-local-app:local-app-build","args":{"operation":"create","app_id":"<the id LINGXI.md names>","name":"<the display name confirmed in step 3>","brief":"<the one-line brief confirmed in step 3>","spec":"<confirmed product + UI + data + runtime intent>","quality_level":"balanced"}}
```

5. Re-read `LINGXI.md`. `LocalAppScaffold` overwrites the guided text with the
   app's formal workspace contract — editable roots, host-managed files, the
   entry points that now exist, and which build workflow the persisted profile takes —
   and that contract, not this step list, governs everything after it.

### Ambiguity routing

After the opening description, classify only unresolved decisions that could
materially change the result. Group related decisions so the single
clarification `AskUserQuestion` round contains 1-3 real questions; omit the
round if no material decision remains:

- **Product / process** — resolve the primary job, audience, success condition,
  or the required flow when the brief leaves more than one plausible product.
- **Target platform** — resolve OS or form factor only when the host context and
  brief do not determine it; otherwise state the inferred target for the final
  confirmation.
- **Data / capabilities** — resolve writable collections, permissions, host
  capabilities, or an explicitly requested external service when alternatives
  would change the implementation or privacy boundary.
- **UI / visual style** — ask only when the visual choice would significantly
  change the result and cannot be inferred from the brief. Otherwise propose a
  concrete design direction, tokens, and platform treatment in the final
  confirmation so the user can edit that proposal.
- **Interaction / accessibility** — resolve a material input, navigation,
  assistive-technology, reduced-motion, or non-pointer requirement that is not
  already implied by the product and target.
- **Canvas mechanics / runtime profile** — first choose `canvas` when the whole
  app is a drawn surface. Prefer `canvas-2d-local-app` for an ordinary 2D app
  or game, and `threejs-local-app` for an explicitly described Three.js scene.
  Use `phaser-2d-local-app` or `babylon-3d-local-app` only when the product
  clearly calls for those runtimes and the host/runtime contract already offers
  them; do not infer availability from package wishes. Ask which runtime family
  only when the brief is genuinely ambiguous among valid options. The later
  canvas design and build steps must preserve the persisted runtime profile.

The final confirmation always includes the inferred or proposed answers,
whether or not a clarification round was needed. Visual platform presentation
remains separate from this technical runtime profile.

Two things hold for as long as the app is a shell:

- **Do not write source before the scaffold lands.** The first scaffold WIPES
  the editable surface. Source written beforehand is deleted, not merged, so
  the turn that wrote it is lost work rather than a head start.
- **Most local-app tools refuse an app that has no shape.** The shell may list
  and inspect records, read `LocalAppRuntimeProfiles` and
  `LocalAppTemplateCatalog`, and then hand the confirmed spec to
  `lingxi-local-app:local-app-build`. Every other one — direct build,
  dependency install, runtime, logs,
  manifest, UI inspection/capture/action, data, checkpoints, app events,
  background flows — returns a refusal that names `LocalAppScaffold` as the way
  out. `LocalAppManifest` included: collections and capabilities are declared
  after the scaffold, not before it. That refusal is the contract, not a
  transient failure, so settle the name, brief and profile instead of retrying
  or routing around it.

Do not call `LocalAppCreate` from inside a shell conversation. The shell app
already exists and is the one the user is looking at; creating a second app
leaves that one empty forever.

`LocalAppCreate` is for a conversation that is NOT an app's: a global or project
chat where the user asks for an app. It creates the same EMPTY SHELL and app
session as the library entry; it does not persist a runtime profile. Continue
the interview and confirmation in that app session, where `LINGXI.md` is
auto-loaded. Do not write app source or launch a build from the global/project
working directory.

The build orchestration below therefore applies only inside an app's OWN
conversation, the one whose `LINGXI.md` names the app id you are building, and
only once that app has a shape. A shell has no source, no dependencies and no
build workflow to choose between.

Inside a shell there is no brief yet to sharpen — the interview above produces
it. Inside an app that already has a shape, read `LINGXI.md` and sharpen the
brief it carries. In a global chat, gather the product, screens, data,
capabilities, and visual intent before creating the app. When a material decision is unresolved, call `AskUserQuestion`
with one short round of 1-3 focused questions; omit the round when there are
none. Never ask unresolved questions in ordinary assistant text.
The one exception is the opening turn of a shell conversation (step 1 above),
where you have no options to offer and need the user's own description; that
exception covers that turn only, and does not extend to any later question in
the same conversation.
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
- the host draws no title or navigation chrome while the app is running, so
  the app owns every visible title/back affordance; reserve the bottom-leading
  80-by-80 CSS-pixel safe-area corner for the host's floating run control;
- pages, navigation/back semantics, complete states, data/permissions;
- for a `canvas` surface, the confirmed runtime profile (`canvas_2d`,
  `three_3d`, `phaser_2d`, or `babylon_3d`) and the mechanics, phase model,
  and input paths that make that choice appropriate;
- design direction, platform tokens, responsive/adaptive behavior, and any
  source-only implementation constraints required by the locked host scaffold;
- which host-provided data, LLM, device, and agent capabilities satisfy each
  product requirement, plus any user-requested external exception;
- whether original raster imagery is required.

Before the final confirmation, classify the confirmed specification without
starting another agent. Score screens/routes, data complexity, host or external
capability groups, target count, multi-step/error-state complexity, and original
raster assets. For a `dom` surface, use this rubric: screens 0/1/2 points for
1/2-3/4+ or nested routes; data 0/1/2 for none or read-only/one simple writable
collection/multiple or concurrency-sensitive collections; capabilities +1 per
distinct group up to 3; targets +1 for multiple form factors and +1 for
multiple operating systems; interaction/state +1 for multi-step, offline, or
complex permission/error flows; raster assets +1. Scores 0-2 suggest `fast`,
3-5 suggest `balanced`, and 6+ suggest `thorough`. Multi-OS, background
scheduling, or two or more sensitive capability groups should recommend
`thorough`; confidence below 0.75 should recommend `balanced`.

The screen axis measures the wrong thing for an app whose interface is a single
drawn surface — a game or any canvas/WebGL app scores 0 on screens and usually
0-1 on data, which would put it in the DOM-only `fast` band that skips the
design stage. That is backwards: such an app has almost no navigation and
almost all of its difficulty in mechanics, state machine and frame loop, which
is exactly what the design stage exists to settle. For a `canvas` surface,
replace the screen axis with its simulation instead: +1 for real-time animation
or a frame loop, +1 for collision, physics or pathfinding, +1 for persistent
progression, and +1 for input beyond a single tap (drag, hold, multi-key).
Clamp any base recommendation to the canvas strategies: a `fast` band becomes
`balanced`, while `balanced` and `thorough` remain as scored (subject to the
stronger multi-target/capability overrides above). Never advertise or pass
`fast` for a canvas surface.

Include the score, reasons, confidence, estimated agent stages, and the
recommended strategy in the same confirmation round. For a `dom` surface, let
the user select `fast`, `balanced`, or `thorough`; for a `canvas` surface, offer
only `balanced` or `thorough` because the drawn-surface workflow requires its
simulation Design stage and never accepts `fast`. Put the recommendation first
and describe the speed/coverage trade-off in each option shown. This is a
task-local workflow choice, not an app persistence field. On a revision, rescore
the revised confirmed specification instead of inheriting a stale strategy.
Never launch a separate classifier agent just to make this recommendation.

Do not silently add a package, capability, domain, platform, or image asset.
The same business logic may serve multiple targets, but each target must use a
platform adapter/tokens layer rather than a width-only conditional.

For a non-core npm-registry package, propose an add/update/remove with a
reason. Add/update is designed to go through `LocalAppConfirmDependencyChange`
and its native one-shot receipt, then `LocalAppUpdateDependencies` — but as
shipped, neither Host operation is on the model-callable tool surface (absent
from the builtin `LocalApp*` table, and the MCP transport refuses their static
spelling by name). There is currently no agent-invocable way to add, update,
or remove a non-core dependency: propose it and its reason as a finding for
the workflow/user to resolve outside this agent turn. Never edit package/lock
or run npm, npx, Yarn, or pnpm directly to work around the gap — that is
exactly the drift the Host's dependency checks exist to catch. React, Ionic,
Vite, renderer engines, and other Catalog core packages can change only
through a same-family Runtime Profile migration, which is a separate,
unrelated path.

From a global or project chat, create a new shell with:

```json
{"brief":"<initial user brief>","name":"<optional provisional name>"}
```

using `LocalAppCreate`, then continue in the app-scoped session it hands you:
the create flow above (step 1 onward) governs from there, including the
unified `lingxi-local-app:local-app-build` create branch that stages the
candidate, raises the native create confirmation, and only then calls
`LocalAppScaffold` — do not call `LocalAppScaffold` directly from this app
session, and do not call `LocalAppCreate` again once inside it. Then declare
collections, domains, and capabilities with `LocalAppManifest`
before generated source
relies on them. Derive `expected_writable_collections` from the confirmed core
UI paths that write those manifest collections. On mobile, the host reads the
materialized manifest and overwrites the workflow's list with every declared
collection id, so the caller's list is advisory only; every declared collection
must have a real UI write path, or be removed from the manifest before building.
Inside an app that already has a shape, call neither again.

The persisted Runtime Profile picks the scaffold and derives its surface.
Profile family CANNOT be changed afterwards — the workspace on disk is that
profile's scaffold, so an app that needs another family must be created again.
Choose it from the confirmed specification and the host catalog:

- `canvas` when the whole interface is one drawn surface that owns a frame
  loop: a game, a simulation, a 3D scene, a live visualization. The workspace
  comes with a canvas screen, a frame-loop helper and a phase machine, and no
  router.
- `dom` for everything assembled from screens, lists and forms. This is the
  default and the common case.

A drawn surface with a settings page is still a Canvas-family profile; a
dashboard that embeds one chart is still `react_dom`. The runtime profile
always appears in the final confirmation because committing it is irreversible.
Read the brief, put your recommendation forward, and let the user correct it;
only ask an earlier clarification when valid catalog profiles remain materially
ambiguous.

`name` is yours to write, not the user's brief truncated. Take the brief's
subject and give it a short, specific display name — two to four words, no
trailing punctuation, in the language the user wrote their brief in. From a
shell conversation, show that name in the confirmation round with the runtime profile,
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

Call one namespaced build workflow with the confirmed spec. Host enriches the
launch with the verified catalog (Create) or persisted profile/snapshot
(Update/Verify); caller input never selects a renderer or profile:

```json
{"name":"lingxi-local-app:local-app-build","args":{"operation":"create","app_id":"<id>","name":"<confirmed display name>","brief":"<confirmed one-line brief>","spec":"<confirmed spec>","quality_level":"balanced"}}
```

`name` and `brief` are create-only and carry the user-confirmed wording; every
other identity field stays Host-derived.
For update use `operation: "update"` and for a verification-only run use
`operation: "verify"`; both fail closed unless Host can read the persisted
Runtime Profile and dependency snapshot. `quality_level` is the only quality
selector. `fast` is rejected after Host resolves a Canvas family. Repair
budgets are fast=1, balanced=1, thorough=2. Host derives writable collection
ids and all template/profile identity; do not pass `renderer`,
`runtime_profile`, template paths, or expected collections.

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

1. **Design** — for `dom`, invoke `$frontend-design` for `balanced` and
   `thorough`, while `fast` folds compact decisions into Generate & Build. For
   `canvas`, invoke `$frontend-design` for both accepted strategies; its
   confirmation never includes `fast`. Produce/confirm platform and form
   factor, page structure, tokens, adapters, interactions, and asset decision.
   Assume the project scaffold and dependency graph are already present and
   locked by the host. Do not propose template choices, npm operations,
   fallback scaffold modes, or root-file edits.
2. **Generate** — invoke `$accessibility` and `$react-best-practices`; consume
   the host-provided LingXi bridge, `deviceContext`, source policy/manifest
   integration, and platform adapter while writing complete React source and
   all required states under the editable roots. Use `queryCollection`, `upsertRecord`, and
   `deleteRecord` from the locked bridge for declared collection data; read
   fields from `records[].document` and do not swallow rejected native writes.
   Stay within the shipped dependency set; if an idea would require package or
   root-infra changes, redesign it as a source-only implementation. For a
   whole-surface `surface: canvas` app, hand off to the matched runtime
   specialist. For `canvas_2d` or `three_3d`, use the profile-managed
   `createFrameLoop` helper from `lib/frame-loop.js`; never call
   `requestAnimationFrame` directly or hand-write a replacement loop. Phaser
   and Babylon use their profile-managed engine lifecycle adapter instead of a
   second frame loop. For a routed `surface: dom` app that only
   embeds a canvas or WebGL region, own exactly one
   `requestAnimationFrame` loop for that region, cancel it in the effect
   cleanup, make that loop's lifecycle responsible for DPR-aware buffer
   sizing, viewport or layout resize, and clamping or resetting the first
   delta after resume, and keep per-frame state out of React state and Zustand
   stores. `canvas_2d` uses no extra engine and `three_3d` uses
   `three@0.185.1`, which is in the locked set. `phaser_2d` and `babylon_3d`
   are valid only when the persisted runtime profile already names those
   host-managed stacks. No other engine, physics or WebGL wrapper can be
   installed.
3. **Build** — call `LocalAppBuild`; the host runs the production
   `vite build --outDir dist --emptyOutDir` equivalent offline from the
   workspace-backed writable mount with the fixed runtime and the app's
   materialized dependency snapshot. The output is staged under private
   `.lingxi-build-state/build-output/` before atomic promotion to
   `build/store/dist/`.
4. **Verify** — invoke `$frontend-qa`. For `dom`, `fast` checks the confirmed
   primary target, root render, fatal console errors, primary interaction, and
   native WebView path; `balanced` covers all confirmed targets and app states;
   `thorough` covers the full phone/tablet/desktop matrix. For `canvas`,
   `balanced` covers the confirmed simulation, captured render, motion, and
   input paths; `thorough` adds the full target matrix and all declared states.
   Use Browser when available for the selected breadth, then use native WebView
   inspect/act/log tools for bridge, data, system back, and device context. For
   every declared collection listed in `expected_writable_collections`, perform
   the real UI write and then call `LocalAppQueryData`; the returned
   `records[].document` must contain the value. On mobile, the host derives the
   list from every collection in the materialized manifest and overwrites any
   caller-provided list before Verify, so a non-empty list cannot be reported as
   `not_applicable`. A declared collection without a UI write path must be
   removed from the manifest and rebuilt. A local-only value or swallowed bridge
   failure is not persistence.

On a verification finding, repair, rebuild, and re-verify according to the
selected surface strategy: for `dom`, at most once for `fast`/`balanced` and
twice for `thorough`; for `canvas`, at most once for `balanced` and twice for
`thorough`.
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
