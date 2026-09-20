---
name: create-local-app
description: Orchestrate confirmed local-capability-first app design, React generation, offline build, and native WebView verification (Browser if available) in the host-scaffolded workspace.
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

If a shell workspace contract handed off to this skill, carry forward only the
answers explicitly present in the current conversation. Do not assume that
name, targets, runtime family, MCP intent, or visual decisions were already
confirmed, and never re-ask a decision the user has actually answered. Fill
only the remaining material gaps before the final AuthoringSpec confirmation.

1. If no product request or usable brief is present, open in ORDINARY ASSISTANT
   TEXT with one short open question asking what they want to build, then wait
   for the answer. Do NOT use `AskUserQuestion` for that opening: a picker
   would guess the user's idea. If the current conversation or Host handoff
   already contains a product request, skip the question and use that brief
   verbatim; never re-ask an answered decision.
2. Read what they wrote and settle whatever it already settles. Infer every
   point the description and host context determine, state the inference, and
   move on. Ask only material unresolved questions; there is no question-count
   quota and no re-asking of an answered decision. Continue until the confirmed
   AuthoringSpec is complete, then use the answers in step 3.
2a. After targets have been inferred, normalize each target OS for routing with
   `String(target.os ?? "").trim().toLowerCase()` (so `ios` and `ipados` are
   matched case-insensitively). For create or UI-impacting design/generate/update
   work, if any confirmed target is iOS or iPadOS, load the
   `lingxi-local-app:apple-design` skill exactly once before proposing the UI or
   presenting the final AuthoringSpec, including for `fast`. Route from the
   confirmed `targets[]`; never substitute the launching device or ask another
   platform questionnaire. Apple Design is the default for those targets only,
   and an explicit brand or reference remains authoritative. This conditional
   load is workflow work, not an extra confirmation. Do not load it for
   verify-only or code-only work.
3. Call `LocalAppRuntimeProfiles` to make a technical recommendation, then
   propose a display **name**, one-line **brief**, targets, and the complete
   AuthoringSpec. Include a user-facing one-line UI summary naming the
   structure/navigation, light/dark/system theme and accent, style/density, and
   phone/tablet or Canvas treatment. Show the profile's derived surface, core
   engine, revision, and recommendation reason in the same final confirmation.
   Do not turn a technical profile recommendation into a separate mandatory
   questionnaire; the Host-verified selector commits it only after the
   confirmed spec.
4. Keep MCP authoring separate from creation, but preserve an explicit
   create-time MCP intent when the user names business capabilities they want
   the LLM to expose later. Explain the distinction between app-exposure
   capabilities and external integrations, read the verified catalog's
   `mcpSuggestions`, and ask only if that intent is materially unresolved. If
   asked, record `{"status":"declined"}` or
   `{"status":"requested","capabilities":[...]}`; if not asked, omit
   `mcp_intent` so Host records `never_asked`. This does not configure or
   publish MCP during creation.
5. After the conversational confirmation, call the unified
   `lingxi-local-app:local-app-build` create branch through the `Workflow`
   tool for this shell app. That
   Host-owned path re-reads the template catalog, stages the create candidate,
   shows one trusted native create confirmation, and only then calls
   `LocalAppScaffold`. Pass the complete Host-bound AuthoringSpec, including
   product, targets, UI structure/theme/style, design, and acceptance checks.
   Pass the display **name** and **brief** the user just
   confirmed in step 3 verbatim: they are staged with the create candidate, they
   are what the native create confirmation sheet renders, and they are what the
   Host commits onto the record — the shell app created in step 1 is still the
   empty `untitled` placeholder, so a create launched without them cannot show
   or commit the confirmed wording. App creation itself never runs MCP
   authoring: the app-owned MCP remains unconfigured and disabled until the
   user explicitly starts MCP setup from that app's settings. Pass the exact
   `mcp_intent` outcome when one was collected; never call a
   standalone runtime-profile selector or pass a model-authored
   surface/profile override. Do not supply `args.runtime_profile` as an authority; on a create launch the host strips any caller-supplied `runtime_profile` at the launch boundary and injects no profile at all — the profile is fixed later in the run by the Host-verified template selection:

```
Workflow({"name":"lingxi-local-app:local-app-build","args":{"operation":"create","app_id":"app_123","name":"Errand List","brief":"Track errands with a quick add flow","authoring_spec":{"product":{"goal":"Track errands","tasks":["add an errand"],"external_integrations":[]},"targets":[{"id":"phone","os":"ios","form_factor":"iphone"}],"ui":{"structure":["list","editor"],"theme":{"mode":"system","accent":"blue"},"style":{"direction":"clear","density":"comfortable"},"references":[]},"design":{"presentations":[{"target_id":"phone","presentation":"single-column list","navigation":"push editor; back returns to list"}],"tokens":{"spacing":"8px"},"states":{"loading":"skeleton","empty":"prompt to add","error":"retry","success":"show list","permission":"explain if needed"},"inputs":{"pointer_touch":["tap"],"keyboard_mouse":["tab"],"back":"pop editor","reduced_motion":"respect device"},"canvas":null},"acceptance_checks":[{"id":"add-item","target_ids":["phone"],"required":true,"preconditions":[],"steps":["open editor","save errand"],"expected":"errand appears in list","evidence":["inspect","ui_action"]}]},"quality_level":"balanced","mcp_intent":{"status":"requested","capabilities":["search saved errands"]}}})
```

6. Re-read `LINGXI.md`. `LocalAppScaffold` overwrites the guided text with the
   app's formal workspace contract — editable roots, host-managed files, the
   entry points that now exist, and which build workflow the persisted profile takes —
   and that contract, not this step list, governs everything after it.

### Ambiguity routing

After the opening description, classify only unresolved decisions that could
materially change the result. Group related decisions into focused
`AskUserQuestion` rounds; ask as many rounds as material decisions require,
without padding a quota, and never repeat an answered question:

- **Product / process** — resolve the primary job, audience, success condition,
  or the required flow when the brief leaves more than one plausible product.
- **Target platform** — resolve OS or form factor only when the host context and
  brief do not determine it; otherwise state the inferred target for the final
  confirmation.
- **Data / capabilities** — resolve writable collections, permissions, host
  capabilities or an explicitly requested external integration when alternatives
  would change the implementation or privacy boundary. Keep Host app-exposure
  capabilities, product external integrations, and optional post-create MCP
  exposure intent as separate decisions.
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

The build orchestration below applies only inside an app's OWN conversation,
the one whose `LINGXI.md` names the app id you are building. Its create branch
accepts that app while it is still an empty shell; update and verify require the
Host-persisted shape. A shell has no source or dependencies until the approved
scaffold lands.

Inside a shell, preserve any usable brief already present in the conversation
or Host handoff and ask for one only when it is absent. Inside an app that
already has a shape, read `LINGXI.md` and its persisted
AuthoringSpec, then sharpen only the requested change. In a global chat, gather the product, screens, data,
capabilities, and visual intent before creating the app. When a material decision is unresolved, call `AskUserQuestion` for that decision; do not impose a question quota and never re-ask an answered decision. Never ask unresolved questions in ordinary assistant text.
An existing shaped app retains its persisted design and explicit brand or
reference choices. Apply the conditional Apple guidance only to a confirmed
UI-impacting change and only within its affected iOS/iPadOS targets; do not
migrate an existing design during verify-only or code-only work.
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
- which Host app-exposure capabilities (data, LLM, device, and agent) satisfy
  each product requirement, separately from any user-requested external
  integration. Host capabilities are manifest permissions. An external
  integration may be required to build the app and must be confirmed as a
  product dependency; it is not an MCP exposure intent by implication.
- whether original raster imagery is required.
- for each acceptance check, set `motion_required: true` only when dynamic
  motion across frames is part of the expected behavior; such a check must
  request `capture` evidence and use a Canvas profile. Omit it or keep it false
  for static UI and reduced-motion checks; identical captured frames remain
  valid when motion is not required.

Before this final confirmation, the coordinator must prepare any required
iOS/iPadOS checks in `acceptance_checks` from the confirmed targets and scope.
For DOM targets these include the existing safe-area reservation, 44 CSS-pixel
control targets, system-font/Ionic navigation, Dynamic Type tolerance, and reduced
motion behavior as applicable; keep them as the existing flat check schema and
do not invent `motion_required` for DOM. The frontend designer reads this
active contract and cannot add acceptance checks after confirmation.

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
reason, then apply it yourself through the two Host operations that exist for
exactly this. Call `LocalAppConfirmDependencyChange` with `app_id` and the
`changes` array: it validates the request against the app's current dependency
baseline and mints a short-lived one-shot receipt. Then call
`LocalAppUpdateDependencies` with `app_id` and that `receipt_id`. Both are
model-callable under their builtin `LocalApp*` names only — each has a row in
the builtin tool table. The `mcp__local_apps__*` spelling of these two was
deliberately retired and returns `ToolNotFound`, so never reach for it. Any
add or update raises the native dependency-review confirmation to the user
before resolution and fails with `user denied dependency changes` if they
decline; a pure remove needs no prompt. The receipt is consumed on use and
goes stale if the baseline moves underneath it, so a `dependencies_dirty`
error means reconfirm and call again — never route
around it. `LocalAppUpdateDependencies` resolves the lockfile in staging,
publishes a dependency snapshot, runs the offline production build and the
profile launch smoke, then commits package/lock, `node_modules` and the build
together, restoring the previous dependency and build state on any failure.
Never edit package/lock or run npm, npx, Yarn, or pnpm directly to work around
this path — that is exactly the drift the Host's dependency checks exist to
catch. React, Ionic, Vite, renderer engines, and other Catalog core packages
are refused by `LocalAppConfirmDependencyChange` itself and can change only
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
Inside an app that already has a shape, do not create another shell or repeat
the opening interview; use its persisted Host contract as the starting point.

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

Keep separate lists in the confirmed AuthoringSpec. `manifest capabilities`
are closed Host app-exposure permissions used by the generated app itself.
`product.external_integrations` names remote services the app may need to
fulfil its product goal; confirm them, their domain/capability, and privacy or
quota impact before creation. MCP intent names business actions the user may
later expose to an LLM. Never silently turn one list into the other.

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
background, or other bitmap, detect whether an ImageGen skill/tool is
available. If available and configured, generate the asset into `public/` and
record prompt, source, and use in the spec. If unavailable, ask once whether
to guide installation/configuration or skip it; on skip, use CSS, gradients,
user assets, or an honest placeholder and do not ask again in this task. A
built-in path does not need an API key; a CLI/API fallback may require
`OPENAI_API_KEY`. After the user chooses setup, follow the environment's
install path, reload skills (`/reload-skills` or its equivalent), re-detect,
and enable immediately if ready. Use inline SVG or CSS for ordinary icons.

## Build orchestration

Call one namespaced build workflow through the `Workflow` tool, with the confirmed spec. Host enriches the
launch with the verified catalog (Create) or persisted profile/snapshot
(Update/Verify); caller input never selects a renderer or profile:

```
Workflow({"name":"lingxi-local-app:local-app-build","args":{"operation":"create","app_id":"app_123","name":"Errand List","brief":"Track errands with a quick add flow","authoring_spec":{"product":{"goal":"Track errands","tasks":["add an errand"],"external_integrations":[]},"targets":[{"id":"phone","os":"ios","form_factor":"iphone"}],"ui":{"structure":["list","editor"],"theme":{"mode":"system","accent":"blue"},"style":{"direction":"clear","density":"comfortable"},"references":[]},"design":{"presentations":[{"target_id":"phone","presentation":"single-column list","navigation":"push editor; back returns to list"}],"tokens":{"spacing":"8px"},"states":{"loading":"skeleton","empty":"prompt to add","error":"retry","success":"show list","permission":"explain if needed"},"inputs":{"pointer_touch":["tap"],"keyboard_mouse":["tab"],"back":"pop editor","reduced_motion":"respect device"},"canvas":null},"acceptance_checks":[{"id":"add-item","target_ids":["phone"],"required":true,"preconditions":[],"steps":["open editor","save errand"],"expected":"errand appears in list","evidence":["inspect","ui_action"]}]},"quality_level":"balanced"}})
```

`name` and `brief` carry the user-confirmed create wording; every
identity field stays Host-derived. External integrations are recorded in the
AuthoringSpec as product requirements. They are not MCP exposure intent and
creation does not configure or expose them.
For update use `operation: "update"` and pass the user-confirmed change as
`revision_prompt` (a plain string carrying what the user asked to change —
`authoring_spec` is the full confirmed object for create/update (verify uses the
Host effective contract); for a verification-only run use
`operation: "verify"`. Both fail closed unless Host can read the persisted
Runtime Profile and dependency snapshot. `quality_level` is the only quality
selector. `fast` is rejected by Host staging for any non-`react_dom` family
after the Host resolves the selection; the workflow never treats a
template-id spelling as a runtime-family contract. Repair
budgets are fast=1, balanced=1, thorough=2. Host derives writable collection
ids and all template/profile identity; do not pass `renderer`,
`runtime_profile`, template paths, or expected collections.

A create run can come back in several ways that are not source defects. If the
workflow returns `{"ok":false,"status":"create_declined",...}`, the user
declined the native create confirmation — that is their answer, not a host
failure: re-confirm the name/brief/AuthoringSpec (offering to change them) and relaunch
`operation: "create"` once the user is ready to try again. A structured
`verification_failed` result preserves the existing app, preview, Host receipt,
and findings. An infrastructure failure is reported without source repair. If
the workflow throws after scaffold succeeded because build tooling failed, the
app record and workspace still exist under `app_id`; continue from that same
app only after the failure is understood.

The directory contract is fixed across every phase. The persistent source
project lives at the current app workspace and is already scaffolded by the
host with pinned root infra and dependencies before generation begins. The
workspace `dist/` directory is disposable and must never be treated as source.
The host mounts this workspace as the sole writable build root (the guest may
see it at the `project/` path), excludes prior
`.lingxi-build-state/build-output/dist/`, forces
`vite build --outDir .lingxi-build-state/build-output/dist --emptyOutDir`
with `--config vite.config.mjs` pinned — the private staging path, never the
workspace's own `dist/` — then atomically promotes the validated snapshot and
serves only `build/store/dist/`. Never configure another `build.outDir`,
inspect host staging paths, or copy generated output back into editable
source.

1. **Design** — for `dom`, the designer uses the frontend-design platform/output
   router and accessibility guidance for `balanced` and `thorough`, while `fast`
   folds compact decisions into Generate & Build. When the coordinator loaded
   Apple Design, apply it to iOS/iPadOS output only, adapting to the Host/Ionic
   shell; do not copy it into Android or desktop targets. For `canvas`, Apple
   guidance applies to the HUD/menu overlay and its safe-area treatment, never
   to the drawn scene. Use the designer for both accepted strategies; its
   confirmation never includes `fast`. Produce/confirm platform and form
   factor, page structure, tokens, adapters, interactions, and asset decision.
   Keep the existing string token/state schema and do not add APIs. On Create,
   this is a structured design return before scaffold; do not write files. Do
   not propose npm operations, fallback scaffold modes, or root-file edits.
2. **Generate** — after the preparer receipt and scaffold, the builder consumes
   its generic device/React guidance and invokes exactly one Host-profile
   renderer guide. Consume the host-provided LingXi bridge, `deviceContext`,
   source policy/manifest
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
   `vite build --outDir .lingxi-build-state/build-output/dist --emptyOutDir
   --config vite.config.mjs` equivalent offline from the workspace-backed
   writable mount with the fixed runtime and the app's materialized dependency
   snapshot. The output therefore lands under private
   `.lingxi-build-state/build-output/` — not in the workspace `dist/` — before
   atomic promotion to `build/store/dist/`. The build also fails closed on
   fresh blocking LSP diagnostics in App-managed JS before Vite ever starts,
   so repair LSP errors first.
4. **Verify** — invoke `$frontend-qa`. For `dom`, `fast` checks the confirmed
   primary target, root render, fatal console errors, primary interaction, and
   native WebView path; `balanced` expands to the confirmed targets and app
   states that the Host authorizes; `thorough` expands the same active contract
   with the broader checks the Host verification scope authorizes. For `canvas`,
   `balanced` covers the confirmed simulation, captured render, motion, and
   input paths; `thorough` adds declared states and any additional targets the
   Host scope authorizes. A quality level never expands the Host's current-device
   `verification_scope`: preserve declared targets and report unverified ones,
   and never call a partial run a full-matrix pass. Use Browser when available
   for the selected breadth, then use native WebView inspect/act/log tools for
   bridge, data, system back, and device context. Use only actual Host in-scope
   evidence for Apple checks; screenshots or synthetic pointers cannot establish
   physical smoothness. For
   every declared collection listed in `expected_writable_collections`, perform
   the real UI write and then call `LocalAppQueryData`; the returned
   `records[].document` must contain the value. On mobile, the host derives the
   list from every collection in the materialized manifest and overwrites any
   caller-provided list before Verify, so a non-empty list cannot be reported as
   `not_applicable`. A declared collection without a UI write path must be
   removed from the manifest and rebuilt. A local-only value or swallowed bridge
   failure is not persistence.

On a completed Host QA candidate with a source finding, repair, rebuild, and
re-verify according to the selected surface strategy: for `dom`, at most once
for `fast`/`balanced` and twice for `thorough`; for `canvas`, at most once for
`balanced` and twice for `thorough`. One bounded evidence resample is separate
from those budgets. Never send an evidence-resample request or infrastructure
failure to source repair.
If issues remain, return them with evidence and the reduced verification level;
never claim full Browser or native QA that was not run.

## Workspace and dependency boundary

Edit generated source only under `app/`, `src/`, `components/`, `lib/`,
`styles/`, and `public/`. `package.json`, lockfiles, `node_modules`,
`index.html`, `vite.config.*`, host metadata under `.lingxi/`,
`lib/device-context.js`, `lib/lingxi-bridge.js`, and
`lib/platform-adapter.js` are host-controlled for this workflow. Do not run `npm`, `npx`, `node`, package
install/uninstall/reconcile commands, or alternate scaffold tools. Nothing in
the host denies these for you: the workspace permission lease is a filesystem
boundary only and deliberately declines to lease-authorize package managers,
interpreters and network commands, routing them to the ordinary Shell approval
path instead — so a plain approval prompt appearing for `npm install` is not
the host permitting it, and this ban is yours to keep. Never set
`build.outDir` yourself either: the host passes `--outDir` on the command line
and pins `--config vite.config.mjs`, so a config-level output path is both
host-managed and overridden.
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
