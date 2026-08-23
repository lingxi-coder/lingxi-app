export const meta = {
  name: 'local-canvas-build',
  description: 'Adaptively design, generate, build, and verify a confirmed local app whose interface is a drawn surface.',
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
  '- This app is ONE DRAWN SURFACE. Start from app/screens/game-screen.jsx. There is no router and there are no routes: menus, pause, settings and game-over are overlays composed on top of the canvas, not separate pages.',
  '- Own the frame loop through the checked-in helper createFrameLoop in src/game/frame-loop.js. It already sizes the drawing buffer to the device pixel ratio, resizes on rotation and iPad multitasking, clamps the first frame after a resume, and cancels itself. Start it inside an effect and stop it in that effect’s cleanup. Do not re-derive those three; a hand-rolled requestAnimationFrame loop that misses any of them looks correct on a desktop preview and fails on a real device.',
  '- Keep per-frame simulation state in a ref. The store in src/stores/game-store.js is for the phase machine, the score and settings — values that change a few times per session. Pushing positions or velocities through it re-renders React sixty times a second and turns the app into a slideshow.',
  '- 2D needs no dependency. For 3D, three@0.185.1 is in the locked set: import it directly and drive the renderer from your own loop. Nothing outside the locked set can be installed, so do not design around a game engine, a physics library, or a WebGL wrapper that is not there.',
  '- Overlays are Ionic components. Import them from the @ionic/react barrel and NEVER from @ionic/core/components: the per-component entry points dynamically import one another and the pinned iife build rejects that outright. There is no Tailwind — use Ionic CSS variables and app/globals.css.',
  '- Make the surface reachable without a pointer. A drawn app that only responds to drag is unusable with a keyboard and unverifiable through the host automation path, which drives discrete pointer and key actions. Accept keys for every action a pointer can perform.',
  '- Keep Vite’s official dist/ output default. Do not redirect build.outDir. The host mounts this workspace as the sole writable build root (guest-visible as the project path), excludes prior .lingxi-build-state/build-output/dist/, runs with --outDir dist --emptyOutDir, and serves only the atomically promoted build/store/dist/.',
  '- The page reaches host data/network/device ONLY through window.lingxi.v2 and the checked-in bridge adapter.',
  '- Use the checked-in bridge helpers for host AI: requestLlmChat for complete responses, streamLlmChat with onLlmStreamFrame for ordered streaming frames, and never call a provider SDK directly.',
  '- Use the checked-in native helpers getClipboardText, setClipboardText, shareContent, synthesizeSpeech, readFile, writeFile, getDeviceStatus, triggerHaptics, openDeepLink, listCalendarEvents, searchContacts, and getMedia for host capabilities; do not call platform SDKs from the page.',
  '- Manifest capabilities are exact enums: data_mutation, ui_control, camera, photo_library, microphone, location, notifications, clipboard, share, text_to_speech, files_read, files_write, device_status, haptics, deep_link, calendar, contacts, media, llm, agent_notify, background_schedule. data_mutation is for conversation-agent LocalAppMutateData calls; page-owned window.lingxi.v2.data writes do not request it solely for storage. background_schedule is required when the app registers a system background flow. WebAssembly and Web Workers are available to every local app and need no capability at all: the served policy already allows wasm compilation and blob: workers, so never invent one to ask for them.',
  '- For page storage, import queryCollection, upsertRecord, and deleteRecord from the locked bridge. Read app fields from records[].document. Never invent action/create/record mutation shapes.',
  '- localStorage must never be authoritative for a declared collection and must not hide a failed native write. Do not swallow bridge errors; surface a recoverable UI error and keep failed state retryable.',
  '- Dependencies are fixed by the host-owned template and lockfile set. The host may prepare the workspace dependencies when needed, but this workflow may not add, remove, install, reconcile, or re-lock packages itself.',
  '- Source versioning uses ordinary workspace Git history. Use the existing Git capability when available; do not add a second version store or command surface.',
  '- Data collections, network domains, capabilities, target OS, and form factor must be confirmed before source generation.',
].join('\n');

/// A drawn surface has no screen hierarchy to describe, so asking for one
/// produced designs that answered the wrong question. What decides whether this
/// app is any good is the simulation: what updates each frame, what the phases
/// are, what the player can do, and how a run ends.
const DESIGN_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    targets: {
      type: 'array',
      description: 'Confirmed native targets as {os, form_factor} entries.',
      items: {
        type: 'object',
        properties: {
          os: { type: 'string' },
          form_factor: { type: 'string' },
        },
        required: ['os', 'form_factor'],
      },
    },
    loop: {
      type: 'string',
      description: 'What advances every frame, in terms of dt: motion, spawning, collision, scoring. Name what is simulated, not how it is drawn.',
    },
    phases: {
      type: 'array',
      description: 'The phase machine, e.g. menu, playing, paused, over. Each entry names the phase and what leaves it.',
      items: { type: 'string' },
    },
    inputs: {
      type: 'array',
      description: 'Every action the player can take, each with BOTH its pointer form and its key form. An action with no key form cannot be driven by the host automation path and cannot be verified.',
      items: { type: 'string' },
    },
    end_conditions: {
      type: 'string',
      description: 'How a run ends and what carries over between runs.',
    },
    frame_budget: {
      type: 'string',
      description: 'Roughly what has to be drawn per frame and why it fits a phone GPU: entity counts, draw calls, texture sizes.',
    },
    summary: { type: 'string' },
  },
  required: ['targets', 'loop', 'phases', 'inputs', 'end_conditions', 'summary'],
};

/// `render_check` is the HARD gate here, and it deliberately has no
/// `not_applicable`.
///
/// The DOM workflow can fall back on `data_roundtrip` because a form-shaped app
/// writes collections. A drawn app usually declares none, which made that gate
/// answer `not_applicable` — and pass — for the entire class of app it was
/// supposed to guard. The only evidence that a drawn app works is a frame
/// somebody looked at.
const VERIFICATION_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    findings: {
      type: 'array',
      description: 'Unresolved defects that still need a source change. NOT a record of what was checked.',
      items: { type: 'string' },
    },
    checked_matrix: {
      type: 'array',
      description: 'Targets actually exercised.',
      items: { type: 'string' },
    },
    browser_available: { type: 'boolean' },
    webview_checked: { type: 'boolean' },
    degraded_verification: { type: 'boolean' },
    data_roundtrip: {
      type: 'object',
      description: 'Native persistence check. A drawn app that declares no collection answers not_applicable here — which is why it is NOT the gate for this workflow.',
      properties: {
        status: { type: 'string', enum: ['passed', 'not_applicable', 'failed'] },
        collections: { type: 'array', items: { type: 'string' } },
        evidence: { type: 'string' },
      },
      required: ['status', 'collections', 'evidence'],
    },
    render_check: {
      type: 'object',
      description: 'What a captured frame actually showed. There is no not_applicable: this app draws its whole interface.',
      properties: {
        status: { type: 'string', enum: ['passed', 'failed'] },
        canvas_surfaces: { type: 'integer' },
        frames_captured: { type: 'integer' },
        interactions_driven: { type: 'array', items: { type: 'string' } },
        evidence: {
          type: 'string',
          description: 'What you SAW in the captured frames, in concrete visual terms: shapes, colours, positions, and how they changed between frames. Naming the app or restating the spec is not evidence that it rendered.',
        },
      },
      required: ['status', 'canvas_surfaces', 'frames_captured', 'interactions_driven', 'evidence'],
    },
    motion_check: {
      type: 'object',
      description: 'Proof the frame loop is actually running, which a single still frame cannot show.',
      properties: {
        status: { type: 'string', enum: ['passed', 'failed'] },
        frames_compared: { type: 'integer' },
        difference: {
          type: 'string',
          description: 'What changed between two frames captured at different times, and after which input. "Nothing changed" is a FAILED motion check unless the app was deliberately paused.',
        },
      },
      required: ['status', 'frames_compared', 'difference'],
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
    'motion_check',
    'summary',
  ],
};

/// Everything that differs between a routed app and a drawn surface.
///
/// The build, repair and gating machinery is shared: `local_app_workflow_core.js`
/// is appended to this file at compile time, so there is exactly ONE derivation
/// of the repair loop, the preview requirement and the terminal throws.
const SHAPE = {
  // `fast` is the one strategy that skips Design, and for a drawn surface that
  // is backwards: such an app has almost no navigation and almost all of its
  // difficulty in the mechanics and the frame loop, which is precisely what
  // Design settles. The skill already recommends against it; this refuses it.
  allowedStrategies: ['balanced', 'thorough'],

  extraBlockingFindings: (verification) => {
    const motion = verification?.motion_check;
    if (!motion || typeof motion !== 'object') {
      return [
        'the verification reported no motion_check; capture two frames at different times and report {status, frames_compared, difference} so a frozen surface cannot pass as a rendered one',
      ];
    }
    if (motion.status === 'failed') {
      return [`motion check failed: ${motion.difference || 'no difference reported'}`];
    }
    // A still frame proves something was drawn ONCE. Two frames that differ are
    // the only cheap proof the loop is still running, and claiming `passed`
    // while comparing fewer than two is the claim this catches.
    if (!(motion.frames_compared >= 2)) {
      return [
        'motion check reported passed but compared fewer than two frames; capture at least two and report what changed',
      ];
    }
    return [];
  },

  designPrompt: ({ appId, confirmedSpec, strategyContext, revision }) => [
    'Act as the local app design lead by invoking $frontend-design.',
    `Prepare the implementation design for local app "${appId}" from this confirmed request:`,
    confirmedSpec,
    strategyContext,
    revision ? `Revision feedback:\n${revision}` : '',
    '',
    'This app is a DRAWN SURFACE, not a set of screens. Do not return a screen hierarchy or a navigation model; there is no router. Return targets as an array of {os, form_factor} entries (iPhone, Android phone, iPad/tablet, Android tablet, or desktop) and how each was confirmed or inferred from the fixed Mobile Runtime Environment reminder (Host OS, Device class, Execution target, Launch mode).',
    'Then settle the simulation: what advances every frame in terms of dt, the phase machine and what leaves each phase, every player action in BOTH its pointer form and its key form, how a run ends and what carries over, and roughly what has to be drawn per frame for it to fit a phone GPU.',
    'An action with no key form cannot be driven by the host automation path and therefore cannot be verified. Give every action a key.',
    'Do not treat viewport, safe-area, color-scheme, reduced-motion, or input-mode as prompt facts. Those are dynamic runtime values that the generated app must read from window.lingxi.v2.deviceContext. Reduced motion in particular is a real requirement for a drawn app: decide now what it means for this simulation.',
    'Assume the host already scaffolded the workspace and pinned the package/runtime contract. Design against the existing project shape; do not request package, template, scaffold, fallback, or toolchain decisions.',
    'If the confirmed brief needs an original bitmap asset (sprite, texture, background), conditionally detect ImageGen; when ready, record prompt/source/use and generate under public/. If unavailable, continue with procedurally drawn shapes rather than treating ImageGen as a hard dependency.',
    'Do not propose package changes or any root-file edits in the design result. The build root, package graph, and locked infra are host-owned.',
  ],

  generatePrompt: ({ appId, confirmedSpec, strategyContext, design, strategyPolicy }) => [
    strategyPolicy.runDesign
      ? 'Generate the complete drawn-surface implementation. Invoke $accessibility and $react-best-practices as independent reviewers while writing the source.'
      : 'Generate the complete drawn-surface implementation from the confirmed spec. Use the compact design decisions in this prompt and invoke $accessibility and $react-best-practices as independent reviewers while writing the source.',
    `Implement local app "${appId}" from the confirmed request and design:`,
    confirmedSpec,
    strategyContext,
    JSON.stringify(design || { summary: 'Use the confirmed spec as the compact design brief.' }),
    '',
    CONTRACT,
    '',
    'Draw something on the FIRST frame. A surface that only becomes visible after the player starts a run is indistinguishable from a broken one, both to the player and to the verification stage.',
    'Honour deviceContext.reducedMotion: when it is set, keep the app playable with the animation reduced rather than disabling the loop.',
    'When the app persists progress in a declared collection, use the locked upsertRecord/deleteRecord helpers and the queryCollection response shape. Read only records[].document for app fields. Do not make localStorage authoritative and do not swallow bridge errors.',
    'Stay inside the host-scaffolded dependency set. If a desired approach would require package or root-file changes, choose a source-only implementation instead of requesting dependency work.',
    `After the source is complete, call LocalAppBuild with {"app_id":"${appId}"}. If it fails, inspect LocalAppLogs with log="build", fix only editable source files, and retry the build once. The host may prepare locked workspace dependencies as part of build recovery; do not run a package manager or request an alternate package flow.`,
    `If the build succeeds, call LocalAppRuntime with {"app_id":"${appId}","action":"start"} and preserve the returned preview URL in preview_url. If the build or runtime step fails, return ok=false and preview_url as an empty string.`,
    'STOP once the build succeeds and the runtime is started. Do not verify your own output here: do not capture frames, do not drive the surface with LocalAppActOnUi, and do not Sleep waiting for it to settle. A separate Verify stage does all of that afterwards with a budget of its own.',
    'Return the structured build result only after generation, build, and runtime start have all completed.',
  ],

  verifyPrompt: ({ appId, build, strategyContext, verificationBreadth, design }) => [
    'Invoke $frontend-qa for deterministic local-app verification.',
    `Verify local app "${appId}" using the preview from the build result:`,
    JSON.stringify(build),
    strategyContext,
    verificationBreadth,
    JSON.stringify(design || { summary: 'Use the confirmed spec and generated app as the design reference.' }),
    '',
    'This app DRAWS its interface, so LocalAppInspectUi is blind to it: it returns an empty element list whether the app is rendering correctly, rendering nothing, or has crashed. An empty snapshot is not evidence of anything. Your evidence is captured frames.',
    `Call LocalAppCaptureUi to see the actual frame. Drive the surface with LocalAppActOnUi action "pointer" (value "x,y" or "x,y,phase" in CSS pixels; phase tap|down|move|up) and action "key" (value "<key>" or "<key>,phase"; phase press|down|up). Exercise every input the design lists, in BOTH forms where it has both.`,
    'Return render_check {status, canvas_surfaces, frames_captured, interactions_driven, evidence}. There is no not_applicable: this app draws its whole interface. evidence must describe what you SAW — shapes, colours, positions, and how they changed — in concrete visual terms. Naming the app or restating the spec is not evidence that it rendered; if you cannot describe the picture, you did not look at one.',
    'Return motion_check {status, frames_compared, difference}. Capture at least two frames at different times, with an input driven between them where the design says one should change the picture, and report what actually differed. A still frame proves something was drawn once; only a difference proves the loop is running. "Nothing changed" is a FAILED motion check unless the app was deliberately paused.',
    `If the app declares a collection that a run writes (a high score, saved progress), perform that real action, then call LocalAppQueryData with {"app_id":"${appId}","collection":"<id>"} and verify the persisted record under records[].document. Return data_roundtrip.status=not_applicable when the app declares no writable collection — that is expected here and is NOT the gate.`,
    'Also confirm the surface is reachable without a pointer: drive the key form of every action and report it in interactions_driven. An action that only responds to pointer input is a finding.',
    'findings is the list of UNRESOLVED DEFECTS that still need a source change, each with its evidence and the file to change. It is not a record of what you checked and it is not a summary: every entry you put there sends the app into another repair round and, once the rounds run out, fails the build quoting your own text back. A verification that found nothing wrong returns findings as an empty array no matter how much it verified. What you checked goes in checked_matrix, what you concluded goes in summary, and what a capture showed goes in render_check.evidence.',
    'Return ok, findings, checked matrix, browser_available, webview_checked, degraded_verification, data_roundtrip, render_check, and motion_check. Do not repair source in this pass.',
  ],

  repairPrompt: ({ appId, verification, renderFindings, strategyContext }) => [
    `Repair the findings from frontend-qa for local app "${appId}".`,
    JSON.stringify(verification),
    ...(renderFindings.length > 0
      ? [`Additional blocking findings from the render gate: ${renderFindings.join('; ')}`]
      : []),
    strategyContext,
    CONTRACT,
    'Fix the smallest source-level cause. Do not install packages, change root infra files, or edit workspace package files, and do not claim verification yet.',
    'A blank or frozen surface is usually one of four things: the loop never started, the effect cleanup cancelled it immediately, the drawing buffer has zero size because the canvas has no layout height, or the draw happens outside the transform the resize handler set. Check those before rewriting the simulation.',
    `After repairing, call LocalAppBuild with {"app_id":"${appId}"}. Let the host prepare locked dependencies if needed. If the build fails, read the build log, fix only editable source files, and retry once.`,
    `If the build succeeds, call LocalAppRuntime with {"app_id":"${appId}","action":"restart"} and preserve its preview URL in preview_url. Otherwise return ok=false and preview_url as an empty string.`,
    'Return the structured build result only after repair, rebuild, and runtime restart have all completed.',
  ],
};
