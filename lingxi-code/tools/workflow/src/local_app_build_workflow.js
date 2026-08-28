export const meta = {
  name: 'local-app-build',
  description: 'Adaptively design, generate, build, and verify a confirmed local app.',
  phases: [
    { title: 'Design' },
    { title: 'Generate & Build' },
    { title: 'Verify' },
  ],
};

const CONTRACT = [
  'Workspace contract (violations break the app):',
  '- Edit ONLY files under app/, src/, components/, lib/, styles/, public/ in the current workspace.',
  '- The host has already scaffolded the workspace, checked in the locked package manifests, and pinned the root build contract before this workflow starts. Do not run npm, npx, node, Vite scaffolds, Shell-driven package installs, or any alternate project generator in any phase.',
  '- Root infrastructure is locked: do not edit package.json, pnpm-lock.yaml, pnpm-workspace.yaml, index.html, vite.config.*, jsconfig.json, host metadata under .lingxi/, lib/device-context.js, lib/lingxi-bridge.js, lib/lingxi-provider.jsx, lib/platform-adapter.js, or styles/foundation.css.',
  '- Generate only editable source and assets, starting from the entry point the scaffold already put there: app/screens/home-screen.jsx for this routed app. Read what is on disk rather than assuming, and do not import or reference canvas-only entry points such as app/screens/game-screen.jsx from this workflow.',
  '- The UI kit is Ionic. Import components from the @ionic/react barrel and NEVER from @ionic/core/components: the per-component entry points dynamically import one another and the pinned iife build rejects that outright. There is no Tailwind and no shadcn/ui — use Ionic CSS variables (--ion-color-primary, --ion-background-color, --ion-color-step-*), Ionic utility classes (ion-padding, ion-margin, ion-text-center, ion-justify-content-*, ion-hide-*), and app/globals.css for anything else.',
  '- The platform look is already chosen: the checked-in provider calls setupIonicReact with the host OS, so components render iOS or Material chrome on their own. Do not branch on the user agent and do not hard-code the metrics of one platform.',
  '- A routed app puts routes inside IonRouterOutlet using react-router 6 Routes/Route, and every routed screen renders IonPage as its ROOT element — without that the outlet has nothing to animate and the platform back gesture never attaches. Navigate with routerLink rather than an onClick handler.',
  '- The host draws NO chrome around a running app: no title bar, no back button, no navigation of any kind. Whatever bars this app renders are the only bars the user gets. Every routed screen therefore renders its own IonHeader/IonToolbar with an IonTitle, and every screen that can be pushed onto also renders its own IonBackButton inside IonButtons slot="start" with a defaultHref, because nothing outside the page offers a way back. The web view keeps the TOP safe-area inset, so a header already sits below the status bar: do not add status-bar padding of your own on top of it.',
  // The keep-clear square below is DERIVED from the two clients, not guessed.
  // Collapsed footprint, measured out from the safe-area corner as
  // (leading across) x (bottom up):
  //   Android -- LocalAppsScreen.kt RunPill: call-site `.padding(16.dp)` at
  //     `Modifier.align(Alignment.BottomStart)`, Row `padding(horizontal = 4.dp)`,
  //     one `IconButton` at `RunPillTapTarget` = 48.dp.
  //     16 + 4 + 48 + 4 = 72dp across, 16 + 48 = 64dp up.
  //   iOS -- LocalAppDetailView.swift LocalAppRunControl: call-site `.padding(16)`,
  //     HStack `.padding(.horizontal, 4)`, one button at `tapTarget` = 44.
  //     16 + 4 + 44 + 4 = 68pt across, 16 + 44 = 60pt up.
  // Android is the larger client on both axes, so take its 72 x 64 and round up
  // to 80, published as one square (80 across, 80 up). The previous 64 x 64 was
  // SHORT of the leading extent on BOTH clients (68 and 72), i.e. it told
  // generated apps that a strip the host control actually covers was theirs.
  //
  // EXPANDED the control is three buttons wide and the same height:
  //   Android 16 + 4 + 48 + 4 + 48 + 4 + 48 + 4 = 176dp across, 64dp up.
  //   iOS     16 + 4 + 44 + 2 + 44 + 2 + 44 + 4 = 160pt across, 60pt up.
  // The published square covers the COLLAPSED footprint only. Expansion is
  // user-initiated and collapses again on the next tap, so reserving ~180pt of
  // every generated app's bottom edge permanently would sterilize a whole strip
  // for a state that is transient by construction. The contract line still warns
  // about that strip so nothing time-critical lands there.
  '- The host floats ONE control over the running page in the BOTTOM-LEADING corner: a 44-48pt target, inset 16pt from the safe area, painted ON TOP of the page. Anything the app puts under it is covered and untappable. Keep that corner clear, the leading 80pt by the bottom 80pt measured from the safe area (foundation.css exposes --safe-area-bottom and --safe-area-left). Tapping the control expands it along the bottom edge to about 180pt for as long as it is held open, so keep anything time-critical off that strip too; only the square has to be reserved permanently. IonFab defaults to the bottom-END corner, which is clear: leave it there and never place one at horizontal="start" with vertical="bottom". An IonTabBar spans the FULL width of an IonPage, so its leading-most tab lands under the control: prefer navigation in the header (IonSegment on an IonToolbar), or, when a bottom tab bar really is the right shape, reserve the leading edge of the bar with padding-inline-start so that no tab sits in that square.',
  '- @ionic/react-router exports EXACTLY three router components — IonReactRouter, IonReactHashRouter, IonReactMemoryRouter — and no hooks. Every Ionic hook, useIonRouter included, comes from the @ionic/react barrel. Importing a hook from @ionic/react-router fails the build with MISSING_EXPORT, and it fails while rendering chunks rather than while transforming, so the message names the package and not your screen.',
  '- Keep Vite\'s official dist/ output default. Do not redirect build.outDir. The host mounts this workspace as the sole writable build root (guest-visible as the project path), excludes prior .lingxi-build-state/build-output/dist/, runs with --outDir dist --emptyOutDir, and serves only the atomically promoted build/store/dist/.',
  '- The page reaches host data/network/device ONLY through window.lingxi.v2 and the checked-in bridge adapter.',
  '- Use the checked-in bridge helpers for host AI: requestLlmChat for complete responses, streamLlmChat with onLlmStreamFrame for ordered streaming frames, and never call a provider SDK directly.',
  '- Use the checked-in native helpers getClipboardText, setClipboardText, shareContent, synthesizeSpeech, readFile, writeFile, getDeviceStatus, triggerHaptics, openDeepLink, listCalendarEvents, searchContacts, and getMedia for host capabilities; do not call platform SDKs from the page.',
  '- Manifest capabilities are exact enums: data_mutation, ui_control, camera, photo_library, microphone, location, notifications, clipboard, share, text_to_speech, files_read, files_write, device_status, haptics, deep_link, calendar, contacts, media, llm, agent_notify, background_schedule. data_mutation is for conversation-agent LocalAppMutateData calls; page-owned window.lingxi.v2.data writes do not request it solely for storage. background_schedule is required when the app registers a system background flow. WebAssembly and Web Workers are available to every local app and need no capability at all: the served policy already allows wasm compilation and blob: workers, so never invent one to ask for them.',
  '- For page storage, import queryCollection, upsertRecord, and deleteRecord from the locked bridge. Read app fields from records[].document. Never invent action/create/record mutation shapes.',
  '- localStorage must never be authoritative for a declared collection and must not hide a failed native write. Do not swallow bridge errors; surface a recoverable UI error and keep failed state retryable.',
  '- Dependencies are fixed by the host-owned template and lockfile set. The host may prepare the workspace dependencies when needed, but this workflow may not add, remove, install, reconcile, or re-lock packages itself.',
  '- Whole drawn surfaces route to the dedicated canvas workflows, not this routed DOM one. When a routed screen still embeds a <canvas> or WebGL region for a chart, simulation, or custom visualization, own exactly one requestAnimationFrame loop inside an effect and cancel it in that effect’s cleanup. Size the drawing buffer to the device pixel ratio, resize it when the viewport or layout changes, clamp the first delta after a resume/background return, and keep per-frame state in refs rather than the store — pushing positions through React re-renders every frame. Do not import the canvas-only lib/frame-loop.js helper here; that profile-managed helper is scaffolded only for whole-surface Canvas2D/Three apps routed elsewhere. 2D needs no dependency. For 3D, three@0.185.1 is in the locked set: import it directly and drive the scene updates and WebGL draw calls from that owned loop. Nothing outside the locked set can be installed, so do not design around a game engine, a physics library, or a WebGL wrapper that is not there.',
  '- Source versioning uses ordinary workspace Git history. Use the existing Git capability when available; do not add a second version store or command surface.',
  '- Data collections, network domains, capabilities, target OS, and form factor must be confirmed before source generation. The mobile host replaces expected_writable_collections with every id in the materialized manifest; every declared collection therefore needs a real UI write path, or remove it from the manifest before building.',
].join('\n');

const DESIGN_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    targets: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          os: { type: 'string' },
          form_factor: { type: 'string' },
          presentation: { type: 'string' },
          navigation: { type: 'string' },
        },
        required: ['os', 'form_factor', 'presentation', 'navigation'],
      },
    },
    information_architecture: {
      type: 'object',
      properties: {
        screens: { type: 'array', items: { type: 'string' } },
        states: {
          type: 'object',
          properties: {
            loading: { type: 'string' },
            empty: { type: 'string' },
            error: { type: 'string' },
            success: { type: 'string' },
            permission: { type: 'string' },
          },
          required: ['loading', 'empty', 'error', 'success', 'permission'],
        },
      },
      required: ['screens', 'states'],
    },
    visual_system: {
      type: 'object',
      properties: {
        design_direction: { type: 'string' },
        tokens: {
          type: 'object',
          properties: {
            color: { type: 'array', items: { type: 'string' } },
            typography: { type: 'array', items: { type: 'string' } },
            spacing: { type: 'array', items: { type: 'string' } },
            radius: { type: 'array', items: { type: 'string' } },
            elevation: { type: 'array', items: { type: 'string' } },
            motion: { type: 'array', items: { type: 'string' } },
            safe_area: { type: 'array', items: { type: 'string' } },
          },
          required: ['color', 'typography', 'spacing', 'radius', 'elevation', 'motion', 'safe_area'],
        },
      },
      required: ['design_direction', 'tokens'],
    },
    interaction_model: {
      type: 'object',
      properties: {
        pointer_touch: { type: 'array', items: { type: 'string' } },
        keyboard_mouse: { type: 'array', items: { type: 'string' } },
        back: { type: 'array', items: { type: 'string' } },
        reduced_motion: { type: 'array', items: { type: 'string' } },
      },
      required: ['pointer_touch', 'keyboard_mouse', 'back', 'reduced_motion'],
    },
    adapter_boundary: {
      type: 'object',
      properties: {
        shared_logic: { type: 'array', items: { type: 'string' } },
        platform_specific_shell: { type: 'array', items: { type: 'string' } },
      },
      required: ['shared_logic', 'platform_specific_shell'],
    },
    acceptance_checks: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
  required: [
    'targets',
    'information_architecture',
    'visual_system',
    'interaction_model',
    'adapter_boundary',
    'acceptance_checks',
    'summary',
  ],
};

const VERIFICATION_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    // Unresolved DEFECTS only. The script treats a non-empty array as blocking:
    // it spends a repair round on it and then fails the build quoting it. That
    // is the right direction for a gate, but the field carried no definition
    // for a long time and "findings" reads to an agent as "what I found" — a
    // real run against the snake game came back `ok:true` with every gate green
    // and ten sentences of praise here, which dispatched a repair agent whose
    // prompt was `Repair the findings` followed by a list of things that worked.
    // The fixtures never caught it because every one of them writes `[]`, so
    // they encode the intended meaning instead of testing that it survives
    // contact with an agent. The definition now lives in the verify prompt too.
    findings: {
      type: 'array',
      items: { type: 'string' },
      description:
        'Unresolved defects that still need a source change. Empty when verification found nothing wrong — ' +
        'observations belong in checked_matrix, conclusions in summary.',
    },
    checked_matrix: { type: 'array', items: { type: 'string' } },
    browser_available: { type: 'boolean' },
    webview_checked: { type: 'boolean' },
    degraded_verification: { type: 'boolean' },
    data_roundtrip: {
      type: 'object',
      properties: {
        status: { enum: ['passed', 'not_applicable', 'failed'] },
        collections: { type: 'array', items: { type: 'string' } },
        evidence: { type: 'string' },
      },
      required: ['status', 'collections', 'evidence'],
    },
    // `data_roundtrip` is the only gate with teeth, and an app with no writable
    // collection answers it `not_applicable` — which passes. For a form-shaped
    // app the DOM snapshot still carries evidence, so that is survivable. For an
    // app that DRAWS its interface it is not: `LocalAppInspectUi` returns an
    // empty element list, `data_roundtrip` is `not_applicable`, and the stage
    // completes having observed literally nothing.
    //
    // `canvas_surfaces` is what closes it, and it keys on the PRESENCE OF A
    // CANVAS rather than on the DOM being empty. That distinction is the whole
    // gate: the first version keyed on `dom_elements_seen === 0`, which a real
    // generated game walks straight past — the snake game this was tested
    // against draws its board to a canvas but still ships a score bar, a pause
    // button and a restart button, so its element list is not empty and the
    // rule never fired. A canvas anywhere on the page means part of the app
    // cannot be seen through the DOM, however much chrome surrounds it.
    //
    // Every field here is still the AGENT's transcription — the workflow VM has
    // no tool-calling primitive of its own, so it cannot read `canvasCount` off
    // the live page. What the gate buys is that the transcription comes from
    // three places that can DISAGREE (a status claim, a frame count, a canvas
    // count), and the disagreements are the shapes that mean "nothing was
    // verified". A verifier that never looked and reports
    // `{status:'not_applicable', canvas_surfaces:0}` is still indistinguishable
    // from an honest DOM app; closing that needs a host-side ledger of
    // LocalAppCaptureUi calls, which does not exist yet.
    render_check: {
      type: 'object',
      properties: {
        status: { enum: ['passed', 'not_applicable', 'failed'] },
        /** `canvasCount` from the LAST LocalAppInspectUi. Non-zero means the DOM path cannot see part of this app. */
        canvas_surfaces: { type: 'integer', minimum: 0 },
        /** Frames actually returned by LocalAppCaptureUi. */
        frames_captured: { type: 'integer', minimum: 0 },
        /** Pointer/key actions actually driven, e.g. ["pointer 120,300 tap", "key ArrowLeft down"]. */
        interactions_driven: { type: 'array', items: { type: 'string' } },
        evidence: { type: 'string' },
      },
      required: [
        'status',
        'canvas_surfaces',
        'frames_captured',
        'interactions_driven',
        'evidence',
      ],
    },
    summary: { type: 'string' },
  },
  required: [
    'ok',
    'findings',
    'checked_matrix',
    'browser_available',
    'webview_checked',
    'degraded_verification',
    'data_roundtrip',
    'render_check',
    'summary',
  ],
};

/// Everything that differs between a routed app and a drawn surface.
///
/// The build, repair and gating machinery below this point is shared by both
/// workflows: it is appended from `local_app_workflow_core.js` at compile time,
/// so there is exactly ONE derivation of the repair loop, the preview
/// requirement and the terminal throws. A second full copy of that loop is how
/// two workflows that started identical end up disagreeing about when a build
/// has failed.
const SHAPE = {
  allowedRuntimeProfileFamilies: ['react_dom'],
  designPrompt: ({ appId, confirmedSpec, strategyContext, revision, runtimeProfile }) => [
      'Act as the local app design lead by invoking $frontend-design.',
      `Prepare the implementation design for local app "${appId}" from this confirmed request:`,
      confirmedSpec,
      strategyContext,
      revision ? `Revision feedback:\n${revision}` : '',
      '',
      `The persisted runtime profile is ${runtimeProfile.family} revision ${runtimeProfile.revision} with contract ${runtimeProfile.contract_sha256}. This routed workflow only applies to runtime_profile.family="react_dom"; preserve that family and do not infer another runtime from the brief, imports, or package wishes.`,
      'Return a concrete, machine-readable design brief matching the supplied design schema: every target is an {os, form_factor, presentation, navigation} entry, and the result must include information_architecture with screens plus loading/empty/error/success/permission states, visual_system with design_direction and structured tokens, interaction_model with pointer_touch/keyboard_mouse/back/reduced_motion, adapter_boundary with shared_logic/platform_specific_shell, acceptance_checks, and summary. Explain how each target was confirmed or inferred from the fixed Mobile Runtime Environment reminder (Host OS, Device class, Execution target, Launch mode). If multiple targets are requested, describe distinct platform presentations sharing business logic.',
      'The host draws no title bar and no back button around a running app, and it floats one small control over the BOTTOM-LEADING corner of the page. Design each screen to carry its own header and its own back affordance, and keep the bottom-leading corner free of app furniture: a full-width bottom tab bar puts a tab underneath that control.',
      'Do not treat viewport, safe-area, color-scheme, reduced-motion, or input-mode as prompt facts. Those are dynamic runtime values that the generated app must read from window.lingxi.v2.deviceContext.',
      'Assume the host already scaffolded the workspace and pinned the package/runtime contract. Design against the existing project shape; do not request package, template, scaffold, fallback, or toolchain decisions.',
      'If the confirmed brief needs an original bitmap asset (photo, illustration, texture, hero, or background), conditionally detect ImageGen; when ready, record prompt/source/use and generate under public/. If unavailable, ask once whether to configure it or skip, then continue with CSS, gradients, user assets, or a placeholder without treating ImageGen as a hard dependency. Never use ImageGen for ordinary UI icons.',
      'Do not propose package changes or any root-file edits in the design result. The build root, package graph, and locked infra are host-owned.',
      ],
  generatePrompt: ({ appId, confirmedSpec, strategyContext, design, strategyPolicy, runtimeProfile }) => [
    strategyPolicy.runDesign
      ? 'Generate the complete React implementation. Invoke $ionic-react-local-app for the routed DOM implementation, then invoke $accessibility and $react-best-practices as independent reviewers while writing the source.'
      : 'Generate the complete React implementation from the confirmed spec. Use the compact design decisions in this prompt and invoke $ionic-react-local-app for the routed DOM implementation, then invoke $accessibility and $react-best-practices as independent reviewers while writing the source.',
    `Implement local app "${appId}" from the confirmed request and design:`,
    confirmedSpec,
    strategyContext,
    `Persisted runtime profile: ${runtimeProfile.family} r${runtimeProfile.revision} (${runtimeProfile.contract_sha256}). Preserve it exactly; this workflow is only for react_dom.`,
    JSON.stringify(design || { summary: 'Use the confirmed spec as the compact design brief.' }),
    '',
    CONTRACT,
    '',
    'When the UI persists declared collection data, use the locked upsertRecord/deleteRecord helpers and queryCollection response shape. Read only records[].document for app fields. Do not make localStorage, IndexedDB, or an in-memory cache authoritative over native collection data. Do not swallow bridge errors or convert a rejected native write into UI success.',
    'Use platform tokens and adapters instead of scattered platform conditionals. Include loading, empty, error, success, disabled, offline, permission-denied, and reduced-motion states where relevant. Build actual copy and interaction paths, not a placeholder shell.',
    'Stay inside the host-scaffolded dependency set. If a desired approach would require package or root-file changes, choose a source-only implementation instead of requesting dependency work.',
    `After the source is complete, call LocalAppBuild with {"app_id":"${appId}"}. If it fails, inspect LocalAppLogs with log="build", fix only editable source files, and retry the build once. The host may prepare locked workspace dependencies as part of build recovery; do not run a package manager or request an alternate package flow.`,
    `If the build succeeds, call LocalAppRuntime with {"app_id":"${appId}","action":"start"} and preserve the returned preview URL in preview_url. If the build or runtime step fails, return ok=false and preview_url as an empty string.`,
    // Observed on device: this stage spent its entire 100-turn budget on QA of
    // its own output — 28 LocalAppActOnUi, 16 LocalAppInspectUi, 18 Sleep — and
    // returned max_turns_exhausted, so the Verify stage never ran even once.
    // Verification is a SEPARATE stage with its own fresh budget and its own
    // required evidence; repeating it here does not make the app better, it
    // spends the turns that finishing the source needs.
    'STOP once the build succeeds and the runtime is started. Do not verify your own output here: do not walk the UI with LocalAppInspectUi, do not drive it with LocalAppActOnUi, do not capture frames, and do not Sleep waiting for it to settle. A separate Verify stage does all of that afterwards with a budget of its own. Confirming the build succeeded and the runtime returned a preview URL is where this stage ends.',
    'Return the structured build result only after generation, build, and runtime start have all completed.',
    ],
  verifyPrompt: ({
    appId,
    build,
    strategyContext,
    verificationBreadth,
    design,
    runtimeProfile,
    confirmedSpec,
    expectedWritableCollections,
  }) => [
      'Invoke $frontend-qa for deterministic local-app verification.',
      `Verify local app "${appId}" using the preview from the build result:`,
      JSON.stringify(build),
    strategyContext,
    verificationBreadth,
    'Confirmed product specification (source of truth; do not infer a different product contract):',
    confirmedSpec,
    `Host-materialized expected_writable_collections (authoritative manifest ids): ${JSON.stringify(expectedWritableCollections)}. Verify every id in this list; every declared collection must have a real core UI write path. If an id has no such path, remove that declaration and rebuild rather than treating it as optional. If the list is non-empty, data_roundtrip.status=not_applicable, missing, or any non-passed status is a blocking finding.`,
    `Persisted runtime profile: ${runtimeProfile.family} r${runtimeProfile.revision} (${runtimeProfile.contract_sha256}). Do not route this DOM verification through a canvas runtime or infer another engine from source imports.`,
    JSON.stringify(design || { summary: 'Use the confirmed spec and generated app as the design reference.' }),
      '',
      'Use Browser when the capability exists for the selected verification breadth. A narrow viewport alone does not prove a platform. Then use the real Local App WebView LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs path to verify bridge/data/device context/system back semantics.',
      `For every host-materialized expected_writable_collections id, perform its real core UI write action, then call LocalAppQueryData with {"app_id":"${appId}","collection":"<id>"} and verify the persisted record under records[].document. A declared collection with no UI write path is a defect: remove it from the manifest and rebuild. A localStorage-only value, optimistic UI state, or swallowed bridge rejection is a failed data roundtrip. Return data_roundtrip.status=passed only with this host-query evidence and include every expected id in collections. Return not_applicable only when the host-materialized expected_writable_collections is empty; otherwise failed and set ok=false.`,
      'If Browser is unavailable, use the existing LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs path and report verification as degraded rather than claiming full visual QA.',
      'If this app draws to a canvas or WebGL surface, the DOM path above is BLIND to it: LocalAppInspectUi returns an empty element list whether the app is rendering correctly, rendering nothing, or has crashed, so an empty snapshot is NOT evidence of anything. Call LocalAppCaptureUi to see the actual frame, and drive it with LocalAppActOnUi action "pointer" (value "x,y" or "x,y,phase" in CSS pixels; phase tap|down|move|up) or action "key" (value "<key>" or "<key>,phase"; phase press|down|up). A canvas app verified only through inspect_ui has not been verified.',
      'Also return render_check {status, canvas_surfaces, frames_captured, interactions_driven, evidence}. canvas_surfaces is the canvasCount field from your LAST LocalAppInspectUi, frames_captured is how many frames LocalAppCaptureUi actually returned, and interactions_driven lists the pointer/key actions you actually drove. Set status=passed only when you have looked at a captured frame and confirmed it shows the app rendering; not_applicable ONLY when canvas_surfaces is 0 and the DOM snapshot already carried the evidence; failed when the frame shows a blank, broken or crashed render. If canvas_surfaces is 1 or more you may not answer not_applicable: part of the app draws itself and inspect_ui cannot see it, however many buttons surround it.',
      'findings is the list of UNRESOLVED DEFECTS that still need a source change, each with its evidence and the file to change. It is not a record of what you checked and it is not a summary: every entry you put there sends the app into another repair round and, once the rounds run out, fails the build quoting your own text back. A verification that found nothing wrong returns findings as an empty array no matter how much it verified. What you checked goes in checked_matrix, what you concluded goes in summary, and what a capture showed goes in render_check.evidence.',
      'Return ok, findings, checked matrix, browser_available, webview_checked, degraded_verification, and data_roundtrip {status, collections, evidence}. Do not repair source in this pass.',
      ],
  repairPrompt: ({ appId, verification, renderFindings, strategyContext, confirmedSpec, design, runtimeProfile }) => [
      `Repair the findings from frontend-qa for local app "${appId}".`,
      JSON.stringify(verification),
      // The gate's own findings are DERIVED, not written by the verifier, so
      // they are absent from the JSON above. Without this line a render-gate
      // repair round arrives with `findings: []` and nothing to act on.
      ...(renderFindings.length > 0
        ? [`Additional blocking findings derived by workflow gates: ${renderFindings.join('; ')}`]
        : []),
      strategyContext,
      'Confirmed product specification (source of truth; do not infer a different product contract):',
      confirmedSpec,
      `Persisted runtime profile: ${runtimeProfile.family} r${runtimeProfile.revision} (${runtimeProfile.contract_sha256}). Preserve this routed react_dom shape; do not switch to a canvas runtime or add a runtime-specific dependency path.`,
      'Authoritative design brief (source of truth): preserve its information architecture, visual system, interaction model, adapter boundary, and acceptance checks while repairing the source. This is a routed DOM workflow; preserve its route and component shape.',
      JSON.stringify(design || { summary: 'No standalone design brief was produced; preserve the confirmed request.' }),
      CONTRACT,
      'Fix the smallest source-level cause. Do not install packages, change root infra files, or edit workspace package files, and do not claim verification yet.',
      `After repairing, call LocalAppBuild with {"app_id":"${appId}"}. Let the host prepare locked dependencies if needed. If the build fails, read the build log, fix only editable source files, and retry once.`,
      `If the build succeeds, call LocalAppRuntime with {"app_id":"${appId}","action":"restart"} and preserve its preview URL in preview_url. Otherwise return ok=false and preview_url as an empty string.`,
      'Return the structured build result only after repair, rebuild, and runtime restart have all completed.',
      ],
};
