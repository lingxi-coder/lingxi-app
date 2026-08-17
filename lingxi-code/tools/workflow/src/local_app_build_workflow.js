export const meta = {
  name: 'local-app-build',
  description: 'Design, generate, build, and verify a confirmed local app.',
  phases: [
    { title: 'Design' },
    { title: 'Generate & Build' },
    { title: 'Verify' },
  ],
};

// args: { app_id: string, spec: string|object, revision_prompt?: string, model?: string }
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
  '- Keep Vite\'s official dist/ output default. Do not redirect build.outDir. Workspace dist/ is disposable preview output; the host excludes prior dist/, builds from an isolated project/ root with --outDir dist --emptyOutDir, and serves only build/store/dist/.',
  '- The page reaches host data/network/device ONLY through window.lingxi.v2 and the checked-in bridge adapter.',
  '- Manifest capabilities are exact enums: data_mutation, ui_control, camera, photo_library, microphone, location, notifications, llm, agent_notify. data_mutation is for conversation-agent mutate_data calls; page-owned window.lingxi.v2.data writes do not request it solely for storage.',
  '- For page storage, import queryCollection, upsertRecord, and deleteRecord from the locked bridge. Read app fields from records[].document. Never invent action/create/record mutation shapes.',
  '- localStorage must never be authoritative for a declared collection and must not hide a failed native write. Do not swallow bridge errors; surface a recoverable UI error and keep failed state retryable.',
  '- Dependencies are fixed by the host-owned template and lockfile set. The host may prepare the workspace dependencies when needed, but this workflow may not add, remove, install, reconcile, or re-lock packages itself.',
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

const VERIFICATION_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    ok: { type: 'boolean' },
    findings: { type: 'array', items: { type: 'string' } },
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
    'summary',
  ],
};

phase('Design');
const designResult = await agent(
  [
    'Act as the local app design lead by invoking $frontend-design.',
    `Prepare the implementation design for local app "${appId}" from this confirmed request:`,
    confirmedSpec,
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
const design = requireAgentResult(designResult, 'the design step');

phase('Generate & Build');
const generated = await agent(
  [
    'Generate the complete React implementation. Invoke $accessibility and $react-best-practices as independent reviewers while writing the source.',
    `Implement local app "${appId}" from the confirmed request and design:`,
    confirmedSpec,
    JSON.stringify(design),
    '',
    CONTRACT,
    '',
    'When the UI persists declared collection data, use the locked upsertRecord/deleteRecord helpers and queryCollection response shape. Read only records[].document for app fields. Do not make localStorage, IndexedDB, or an in-memory cache authoritative over native collection data. Do not swallow bridge errors or convert a rejected native write into UI success.',
    'Use platform tokens and adapters instead of scattered platform conditionals. Include loading, empty, error, success, disabled, offline, permission-denied, and reduced-motion states where relevant. Build actual copy and interaction paths, not a placeholder shell.',
    'Stay inside the host-scaffolded dependency set. If a desired approach would require package or root-file changes, choose a source-only implementation instead of requesting dependency work.',
    `After the source is complete, call mcp__local_apps__build with {"app_id":"${appId}"}. If it fails, inspect mcp__local_apps__read_logs with log="build", fix only editable source files, and retry the build once. The host may prepare locked workspace dependencies as part of build recovery; do not run a package manager or request an alternate package flow.`,
    `If the build succeeds, call mcp__local_apps__manage_runtime with {"app_id":"${appId}","action":"start"} and preserve the returned preview URL in preview_url. If the build or runtime step fails, return ok=false and preview_url as an empty string.`,
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
// The two allowed repair rounds are repair -> rebuild -> re-verify cycles. A remaining
// finding is returned honestly instead of being hidden by another iteration.
for (let round = 0; round <= 2; round += 1) {
  phase('Verify');
  verification = await agent(
    [
      'Invoke $frontend-qa for deterministic local-app verification.',
      `Verify local app "${appId}" using the preview from the build result:`,
      JSON.stringify(build),
      JSON.stringify(design),
      '',
      'Use Browser when the capability exists: preview URL, required viewport matrix, console, navigation, and core interactions. Browser is also required for mobile-sized viewports when available; a narrow viewport alone does not prove a platform. Then use the real Local App WebView inspect_ui/act_on_ui/read_logs path to verify bridge/data/device context/system back semantics. Cover iPhone, Android phone, iPad portrait+landscape, Android tablet portrait+landscape, and desktop when those targets are in scope; inject platform context separately from viewport size.',
      `For every declared collection that a core UI path writes, perform that real UI action, then call mcp__local_apps__query_data with {"app_id":"${appId}","collection":"<id>"} and verify the persisted record under records[].document. A localStorage-only value, optimistic UI state, or swallowed bridge rejection is a failed data roundtrip. Return data_roundtrip.status=passed only with this host-query evidence, not_applicable only when the app has no writable collection UI, otherwise failed and set ok=false.`,
      'If Browser is unavailable, use the existing inspect_ui/act_on_ui/read_logs path and report verification as degraded rather than claiming full visual QA.',
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
  if (verification && verification.ok) break;
  if (round === 2) break;
  repairRounds += 1;
  phase('Generate & Build');
  const repaired = await agent(
    [
      `Repair the findings from frontend-qa for local app "${appId}".`,
      JSON.stringify(verification),
      CONTRACT,
      'Fix the smallest source-level cause. Do not install packages, change root infra files, or edit workspace package files, and do not claim verification yet.',
      `After repairing, call mcp__local_apps__build with {"app_id":"${appId}"}. Let the host prepare locked dependencies if needed. If the build fails, read the build log, fix only editable source files, and retry once.`,
      `If the build succeeds, call mcp__local_apps__manage_runtime with {"app_id":"${appId}","action":"restart"} and preserve its preview URL in preview_url. Otherwise return ok=false and preview_url as an empty string.`,
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
if (verification.ok !== true) {
  const findings = Array.isArray(verification.findings)
    ? verification.findings.join('; ')
    : verification.summary || 'no findings reported';
  throw new Error(
    `local-app-build: verification still has findings for app "${appId}" after the two allowed ` +
      `repair rounds${verification.degraded_verification ? ' (verification was degraded)' : ''}: ` +
      findings,
  );
}

return {
  ok: true,
  preview_url: build.preview_url,
  repair_rounds: repairRounds,
  verification,
  summary: 'Design, generation, build, and frontend QA completed.',
};
