export const meta = {
  name: 'local-app-build',
  description: 'Adaptively design, generate, build, and verify a confirmed local app.',
  phases: [
    { title: 'Design' },
    { title: 'Generate & Build' },
    { title: 'Verify' },
  ],
};

// args: { app_id: string, spec: string|object, strategy?: 'fast'|'balanced'|'thorough', complexity?: object, revision_prompt?: string, model?: string }
const input = args && typeof args === 'object' ? args : {};
const appId = typeof input.app_id === 'string' ? input.app_id : '';
if (!appId) {
  throw new Error('local-app-build requires args.app_id (the app to implement)');
}
// The spec is the ONLY description of what to build, and the skill's flow
// requires the user to have approved it. An absent/empty spec would leave the
// generator free to invent the app, which is exactly the confirmation step
// this segment must not bypass — note that `JSON.stringify(undefined ?? {})`
// is the literal string `"{}"`, which is truthy.
const confirmedSpec =
  typeof input.spec === 'string' ? input.spec : JSON.stringify(input.spec ?? '');
if (
  !confirmedSpec ||
  confirmedSpec.trim().length === 0 ||
  confirmedSpec === '""' ||
  confirmedSpec === '{}'
) {
  throw new Error(
    'local-app-build requires args.spec (the spec the user confirmed) — refusing to invent one',
  );
}

const STRATEGY_POLICY = {
  fast: {
    runDesign: false,
    maxRepairRounds: 1,
    verificationMode: 'smoke',
    label: 'fast',
  },
  balanced: {
    runDesign: true,
    maxRepairRounds: 1,
    verificationMode: 'confirmed-targets',
    label: 'balanced',
  },
  thorough: {
    runDesign: true,
    maxRepairRounds: 2,
    verificationMode: 'full-matrix',
    label: 'thorough',
  },
};

const requestedStrategy =
  input.strategy === undefined ||
  input.strategy === null ||
  (typeof input.strategy === 'string' && input.strategy.trim().length === 0)
    ? 'balanced'
    : typeof input.strategy === 'string'
      ? input.strategy.trim().toLowerCase()
      : '';
if (!Object.prototype.hasOwnProperty.call(STRATEGY_POLICY, requestedStrategy)) {
  throw new Error(
    `local-app-build received unsupported strategy ${JSON.stringify(input.strategy)}; expected fast, balanced, or thorough`,
  );
}
const strategyPolicy = STRATEGY_POLICY[requestedStrategy];

const clamp = (value, min, max) => Math.min(max, Math.max(min, value));
const rawComplexity = input.complexity && typeof input.complexity === 'object' ? input.complexity : {};
const rawScore = Number(rawComplexity.score);
const complexityScore = Number.isFinite(rawScore) ? Math.round(clamp(rawScore, 0, 10)) : 0;
const rawConfidence = Number(rawComplexity.confidence);
const complexityConfidence = Number.isFinite(rawConfidence)
  ? Number(clamp(rawConfidence, 0, 1).toFixed(2))
  : 0;
const complexityBand =
  complexityScore <= 2 ? 'low' : complexityScore <= 5 ? 'medium' : 'high';
const complexityReasons = Array.isArray(rawComplexity.reasons)
  ? rawComplexity.reasons
      .filter((reason) => typeof reason === 'string')
      .map((reason) => reason.trim())
      .filter(Boolean)
      .slice(0, 6)
      .map((reason) => reason.slice(0, 160))
  : [];
const complexity = {
  score: complexityScore,
  band: complexityBand,
  confidence: complexityConfidence,
  reasons: complexityReasons,
};
let agentCalls = 0;
const runAgent = async (prompt, options) => {
  agentCalls += 1;
  return agent(prompt, options);
};
const strategyContext = [
  `Selected workflow strategy: ${strategyPolicy.label}.`,
  `Complexity score: ${complexity.score}/10 (${complexity.band}), confidence ${complexity.confidence}.`,
  complexity.reasons.length > 0 ? `Complexity reasons: ${complexity.reasons.join('; ')}` : '',
  `Verification mode: ${strategyPolicy.verificationMode}.`,
].filter(Boolean).join('\n');
log(strategyContext);
const revision = typeof input.revision_prompt === 'string' ? input.revision_prompt : '';
const requestedModel = typeof input.model === 'string' ? input.model.trim() : '';
const modelSeparator = requestedModel.indexOf('/');
const modelOptions = requestedModel
  ? modelSeparator > 0 && modelSeparator < requestedModel.length - 1
    ? {
        model: requestedModel.slice(modelSeparator + 1),
        modelProfile: requestedModel.slice(0, modelSeparator),
      }
    : { model: requestedModel }
  : {};

const CONTRACT = [
  'Workspace contract (violations break the app):',
  '- Edit ONLY files under app/, src/, components/, lib/, styles/, public/ in the current workspace.',
  '- The host has already scaffolded the workspace, checked in the locked package manifests, and pinned the root build contract before this workflow starts. Do not run npm, npx, node, Vite scaffolds, Shell-driven package installs, or any alternate project generator in any phase.',
  '- Root infrastructure is locked: do not edit package.json, pnpm-lock.yaml, pnpm-workspace.yaml, index.html, vite.config.*, components.json, jsconfig.json, host metadata under .lingxi/, lib/device-context.js, lib/lingxi-bridge.js, lib/lingxi-provider.jsx, lib/platform-adapter.js, or styles/foundation.css.',
  '- Generate only editable source and assets. Start with app/screens/home-screen.jsx, reuse the locked package set and editable components/ui sources, and preserve the lazy #/_components lab outside normal navigation unless the user asks otherwise.',
  '- Keep Vite\'s official dist/ output default. Do not redirect build.outDir. The host mounts this workspace as the sole writable build root (guest-visible as the project path), excludes prior .lingxi-build-state/build-output/dist/, runs with --outDir dist --emptyOutDir, and serves only the atomically promoted build/store/dist/.',
  '- The page reaches host data/network/device ONLY through window.lingxi.v2 and the checked-in bridge adapter.',
  '- Use the checked-in bridge helpers for host AI: requestLlmChat for complete responses, streamLlmChat with onLlmStreamFrame for ordered streaming frames, and never call a provider SDK directly.',
  '- Use the checked-in native helpers getClipboardText, setClipboardText, shareContent, synthesizeSpeech, readFile, writeFile, getDeviceStatus, triggerHaptics, openDeepLink, listCalendarEvents, searchContacts, and getMedia for host capabilities; do not call platform SDKs from the page.',
  '- Manifest capabilities are exact enums: data_mutation, ui_control, camera, photo_library, microphone, location, notifications, clipboard, share, text_to_speech, files_read, files_write, device_status, haptics, deep_link, calendar, contacts, media, llm, agent_notify, background_schedule. data_mutation is for conversation-agent LocalAppMutateData calls; page-owned window.lingxi.v2.data writes do not request it solely for storage. background_schedule is required when the app registers a system background flow. WebAssembly and Web Workers are available to every local app and need no capability at all: the served policy already allows wasm compilation and blob: workers, so never invent one to ask for them.',
  '- For page storage, import queryCollection, upsertRecord, and deleteRecord from the locked bridge. Read app fields from records[].document. Never invent action/create/record mutation shapes.',
  '- localStorage must never be authoritative for a declared collection and must not hide a failed native write. Do not swallow bridge errors; surface a recoverable UI error and keep failed state retryable.',
  '- Dependencies are fixed by the host-owned template and lockfile set. The host may prepare the workspace dependencies when needed, but this workflow may not add, remove, install, reconcile, or re-lock packages itself.',
  '- For a drawn surface — a game, a 3D scene, a custom visualization — render into a <canvas> and own the frame loop yourself with requestAnimationFrame inside an effect, cancelling it on unmount. 2D needs no dependency. For 3D, three@0.185.1 is in the locked set: import it directly and drive the renderer from your own loop. Nothing outside the locked set can be installed, so do not design around a game engine, a physics library, or a WebGL wrapper that is not there.',
  '- Source versioning uses ordinary workspace Git history. Use the existing Git capability when available; do not add a second version store or command surface.',
  '- Data collections, network domains, capabilities, target OS, and form factor must be confirmed before source generation.',
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
        },
        required: ['os', 'form_factor'],
      },
    },
    summary: { type: 'string' },
  },
  required: ['targets', 'summary'],
};

const BUILD_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    preview_url: { type: 'string' },
    summary: { type: 'string' },
  },
  required: ['ok', 'preview_url', 'summary'],
};

// Local-app stages opt into throwOnError so the provider/runtime reason reaches
// the parent task immediately. Keep this null guard as a compatibility fallback
// for older or alternate workflow hosts that implement the default agent()
// contract (failed/killed agents resolve to null).
const requireAgentResult = (result, stage) => {
  if (!result) {
    throw new Error(
      `local-app-build: ${stage} produced no result for app "${appId}"; ` +
        'the step did not run to completion. Re-run with resumeFromRunId to retry from here.',
    );
  }
  return result;
};

const requirePreviewOnSuccess = (result, stage) => {
  requireAgentResult(result, stage);
  if (
    result.ok === true &&
    (typeof result.preview_url !== 'string' || result.preview_url.trim().length === 0)
  ) {
    throw new Error(`${stage} succeeded without a preview_url`);
  }
  return result;
};

// The render gate. Each rule catches a DIFFERENT way the verify stage can
// complete while having observed nothing, so none of them substitutes for the
// others — and they are mutually exclusive on `status`, so one result never
// produces two findings that contradict each other.
//
// Lives in a function, not inline after the loop, because a finding it raises
// has to be worth a REPAIR ROUND like every other blocking finding: the loop's
// break condition calls this too. Inline, a blank canvas failed the build with
// zero repairs spent while `data_roundtrip` — the same shape of defect — got
// its round.
const renderCheckFindings = (verification) => {
  const renderCheck = verification?.render_check;
  // Fails CLOSED. `render_check` is in the schema's `required` list and the
  // subagent runner validates against it, but a gate that silently disappears
  // when its input is missing is the same outcome the gate exists to prevent —
  // and `findings` right beside it is already normalized defensively for
  // exactly this reason. Omitting the newest field is the most likely
  // structured-output miss there is.
  if (!renderCheck || typeof renderCheck !== 'object') {
    return [
      'the verification reported no render_check; report {status, canvas_surfaces, frames_captured, interactions_driven, evidence} ' +
        'so a drawn surface cannot pass unobserved',
    ];
  }
  if (renderCheck.status === 'failed') {
    return [`render check failed: ${renderCheck.evidence || 'no evidence'}`];
  }
  if (renderCheck.status === 'passed') {
    // Claiming the frame was checked without ever obtaining one. The two fields
    // come from different places — a claim and a count — so they can disagree,
    // and this is the disagreement that matters.
    if (!(renderCheck.frames_captured >= 1)) {
      return [
        'render check reported passed but captured no frame; call LocalAppCaptureUi and report frames_captured',
      ];
    }
    return [];
  }
  // `not_applicable` with a canvas present: part of this app is invisible to
  // inspect_ui and no frame was accepted as evidence. Keyed on the canvas, NOT
  // on an empty element list — a game with a score bar and a restart button has
  // a non-empty list and still has a board nobody looked at.
  if (renderCheck.canvas_surfaces >= 1) {
    return [
      'a canvas surface was present but never verified as an image: inspect_ui cannot see what it draws. ' +
        'Call LocalAppCaptureUi, look at the frame, and drive the surface with pointer/key before reporting a result',
    ];
  }
  return [];
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

let design = null;
if (strategyPolicy.runDesign) {
  phase('Design');
  const designResult = await runAgent(
    [
      'Act as the local app design lead by invoking $frontend-design.',
      `Prepare the implementation design for local app "${appId}" from this confirmed request:`,
      confirmedSpec,
      strategyContext,
      revision ? `Revision feedback:\n${revision}` : '',
      '',
      'Return a concrete, machine-readable design brief containing targets as an array of {os, form_factor} entries (iPhone, Android phone, iPad/tablet, Android tablet, or desktop), how each target was confirmed or inferred from the fixed Mobile Runtime Environment reminder (Host OS, Device class, Execution target, Launch mode), screen hierarchy, navigation/back behavior, complete UI states, design tokens, and the platform adapter strategy. If multiple targets are requested, describe distinct platform presentations sharing business logic.',
      'Do not treat viewport, safe-area, color-scheme, reduced-motion, or input-mode as prompt facts. Those are dynamic runtime values that the generated app must read from window.lingxi.v2.deviceContext.',
      'Assume the host already scaffolded the workspace and pinned the package/runtime contract. Design against the existing project shape; do not request package, template, scaffold, fallback, or toolchain decisions.',
      'If the confirmed brief needs an original bitmap asset (photo, illustration, texture, hero, or background), conditionally detect ImageGen; when ready, record prompt/source/use and generate under public/. If unavailable, ask once whether to configure it or skip, then continue with CSS, gradients, user assets, or a placeholder without treating ImageGen as a hard dependency. Never use ImageGen for ordinary UI icons.',
      'Do not propose package changes or any root-file edits in the design result. The build root, package graph, and locked infra are host-owned.',
    ].join('\n'),
    {
      ...modelOptions,
      label: 'design',
      phase: 'Design',
      schema: DESIGN_RESULT_SCHEMA,
      throwOnError: true,
    },
  );
  design = requireAgentResult(designResult, 'the design step');
}

phase('Generate & Build');
const generated = await runAgent(
  [
    strategyPolicy.runDesign
      ? 'Generate the complete React implementation. Invoke $accessibility and $react-best-practices as independent reviewers while writing the source.'
      : 'Generate the complete React implementation from the confirmed spec. Use the compact design decisions in this prompt and invoke $accessibility and $react-best-practices as independent reviewers while writing the source.',
    `Implement local app "${appId}" from the confirmed request and design:`,
    confirmedSpec,
    strategyContext,
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
  ].join('\n'),
  {
    ...modelOptions,
    label: 'generate-build',
    phase: 'Generate & Build',
    schema: BUILD_RESULT_SCHEMA,
    throwOnError: true,
  },
);
// Generation and the deterministic host build share one agent context. This
// avoids paying for a second model startup merely to call build + start, while
// the structured result still prevents an untouched scaffold or missing
// preview from being reported as success.
let build = requirePreviewOnSuccess(generated, 'initial build');

let verification = null;
let repairRounds = 0;
// Strategy-specific repair rounds are repair -> rebuild -> re-verify cycles. A
// remaining finding is returned honestly instead of being hidden by another iteration.
for (let round = 0; round <= strategyPolicy.maxRepairRounds; round += 1) {
  phase('Verify');
  const verificationBreadth =
    strategyPolicy.verificationMode === 'smoke'
      ? 'Run smoke verification for the confirmed primary target only: root render, fatal console errors, the primary interaction, and the native WebView path. Do not claim full cross-platform matrix coverage.'
      : strategyPolicy.verificationMode === 'confirmed-targets'
        ? 'Cover every confirmed target and the complete app states, navigation, accessibility basics, native WebView bridge, and primary interactions. Do not expand into unsupported targets.'
      : 'Cover the full confirmed matrix: iPhone, Android phone, iPad portrait+landscape, Android tablet portrait+landscape, and desktop when those targets are in scope. Use Browser plus native WebView, all core interactions, error/permission/offline states, and every declared collection write path.';
  verification = await runAgent(
    [
      'Invoke $frontend-qa for deterministic local-app verification.',
      `Verify local app "${appId}" using the preview from the build result:`,
      JSON.stringify(build),
      strategyContext,
      verificationBreadth,
      JSON.stringify(design || { summary: 'Use the confirmed spec and generated app as the design reference.' }),
      '',
      'Use Browser when the capability exists for the selected verification breadth. A narrow viewport alone does not prove a platform. Then use the real Local App WebView LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs path to verify bridge/data/device context/system back semantics.',
      `For every declared collection that a core UI path writes, perform that real UI action, then call LocalAppQueryData with {"app_id":"${appId}","collection":"<id>"} and verify the persisted record under records[].document. A localStorage-only value, optimistic UI state, or swallowed bridge rejection is a failed data roundtrip. Return data_roundtrip.status=passed only with this host-query evidence, not_applicable only when the app has no writable collection UI, otherwise failed and set ok=false.`,
      'If Browser is unavailable, use the existing LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs path and report verification as degraded rather than claiming full visual QA.',
      'If this app draws to a canvas or WebGL surface, the DOM path above is BLIND to it: LocalAppInspectUi returns an empty element list whether the app is rendering correctly, rendering nothing, or has crashed, so an empty snapshot is NOT evidence of anything. Call LocalAppCaptureUi to see the actual frame, and drive it with LocalAppActOnUi action "pointer" (value "x,y" or "x,y,phase" in CSS pixels; phase tap|down|move|up) or action "key" (value "<key>" or "<key>,phase"; phase press|down|up). A canvas app verified only through inspect_ui has not been verified.',
      'Also return render_check {status, canvas_surfaces, frames_captured, interactions_driven, evidence}. canvas_surfaces is the canvasCount field from your LAST LocalAppInspectUi, frames_captured is how many frames LocalAppCaptureUi actually returned, and interactions_driven lists the pointer/key actions you actually drove. Set status=passed only when you have looked at a captured frame and confirmed it shows the app rendering; not_applicable ONLY when canvas_surfaces is 0 and the DOM snapshot already carried the evidence; failed when the frame shows a blank, broken or crashed render. If canvas_surfaces is 1 or more you may not answer not_applicable: part of the app draws itself and inspect_ui cannot see it, however many buttons surround it.',
      'findings is the list of UNRESOLVED DEFECTS that still need a source change, each with its evidence and the file to change. It is not a record of what you checked and it is not a summary: every entry you put there sends the app into another repair round and, once the rounds run out, fails the build quoting your own text back. A verification that found nothing wrong returns findings as an empty array no matter how much it verified. What you checked goes in checked_matrix, what you concluded goes in summary, and what a capture showed goes in render_check.evidence.',
      'Return ok, findings, checked matrix, browser_available, webview_checked, degraded_verification, and data_roundtrip {status, collections, evidence}. Do not repair source in this pass.',
    ].join('\n'),
    {
      ...modelOptions,
      label: `verify-${round}`,
      phase: 'Verify',
      schema: VERIFICATION_RESULT_SCHEMA,
      throwOnError: true,
    },
  );
  const verificationFindings = Array.isArray(verification?.findings)
    ? verification.findings
    : [];
  const dataRoundtripFailed = verification?.data_roundtrip?.status === 'failed';
  // The render gate is part of the break decision, not only of the final throw:
  // a blank canvas is a source defect a repair round can fix, and it must buy
  // one exactly like a non-empty `findings` or a failed data round-trip does.
  const renderFindings = renderCheckFindings(verification);
  if (
    verification?.ok === true &&
    verificationFindings.length === 0 &&
    !dataRoundtripFailed &&
    renderFindings.length === 0
  ) {
    break;
  }
  if (round === strategyPolicy.maxRepairRounds) break;
  repairRounds += 1;
  phase('Generate & Build');
  const repaired = await runAgent(
    [
      `Repair the findings from frontend-qa for local app "${appId}".`,
      JSON.stringify(verification),
      // The gate's own findings are DERIVED, not written by the verifier, so
      // they are absent from the JSON above. Without this line a render-gate
      // repair round arrives with `findings: []` and nothing to act on.
      ...(renderFindings.length > 0
        ? [`Additional blocking findings from the render gate: ${renderFindings.join('; ')}`]
        : []),
      strategyContext,
      CONTRACT,
      'Fix the smallest source-level cause. Do not install packages, change root infra files, or edit workspace package files, and do not claim verification yet.',
      `After repairing, call LocalAppBuild with {"app_id":"${appId}"}. Let the host prepare locked dependencies if needed. If the build fails, read the build log, fix only editable source files, and retry once.`,
      `If the build succeeds, call LocalAppRuntime with {"app_id":"${appId}","action":"restart"} and preserve its preview URL in preview_url. Otherwise return ok=false and preview_url as an empty string.`,
      'Return the structured build result only after repair, rebuild, and runtime restart have all completed.',
    ].join('\n'),
    {
      ...modelOptions,
      label: `repair-build-${repairRounds}`,
      phase: 'Generate & Build',
      schema: BUILD_RESULT_SCHEMA,
      throwOnError: true,
    },
  );
  build = requirePreviewOnSuccess(repaired, `repair build ${repairRounds}`);
}

// The task's terminal status comes from whether this SCRIPT throws — an `Ok`
// return is reported as Completed no matter what it contains (see
// `tasks/src/handlers/local_workflow.rs`). So a dead agent (null), a
// self-declared failure (`ok:false`) or unresolved QA findings have to throw;
// otherwise the parent session's task notification announces a successful
// build for an app that never built. The message carries the findings so
// throwing costs no honesty.
requireAgentResult(build, 'the build step');
if (build.ok !== true) {
  throw new Error(
    `local-app-build: the build did not succeed for app "${appId}" after ${repairRounds} repair ` +
      `round(s): ${build.summary || 'no summary'}`,
  );
}
if (typeof build.preview_url !== 'string' || build.preview_url.length === 0) {
  throw new Error(
    `local-app-build: the build reported success for app "${appId}" but returned no preview url; ` +
      'the preview was never started.',
  );
}
requireAgentResult(verification, 'the verification step');
const verificationFindings = Array.isArray(verification.findings)
  ? verification.findings.slice()
  : [];
if (verification.data_roundtrip?.status === 'failed') {
  verificationFindings.push(
    `native data round-trip failed: ${verification.data_roundtrip.evidence || 'no evidence'}`,
  );
}
// Same gate the loop's break condition used, so the last round is judged by
// exactly the rule that decided whether to spend a repair on it.
verificationFindings.push(...renderCheckFindings(verification));
if (verification.ok !== true || verificationFindings.length > 0) {
  const findings = verificationFindings.length > 0
    ? verificationFindings.join('; ')
    : verification.summary || 'no findings reported';
  throw new Error(
    `local-app-build: verification still has findings for app "${appId}" after the ${strategyPolicy.maxRepairRounds} allowed ` +
      `repair rounds${verification.degraded_verification ? ' (verification was degraded)' : ''}: ` +
      findings,
  );
}

return {
  ok: true,
  strategy: requestedStrategy,
  complexity,
  agent_calls: agentCalls,
  preview_url: build.preview_url,
  repair_rounds: repairRounds,
  verification_mode: strategyPolicy.verificationMode,
  verification,
  summary: strategyPolicy.runDesign
    ? `${strategyPolicy.label} strategy completed design, generation, build, and frontend QA.`
    : `${strategyPolicy.label} strategy completed generation, build, and frontend QA.`,
};
