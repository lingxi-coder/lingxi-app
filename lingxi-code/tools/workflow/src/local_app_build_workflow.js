export const meta = {
  name: 'local-app-build',
  description: 'Design, dependency-check, generate, offline-build, and verify a confirmed local app.',
  phases: [
    { title: 'Design' },
    { title: 'Dependencies' },
    { title: 'Generate' },
    { title: 'Build' },
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
  '- The normal new-app path is the official Vite CLI in a truly empty staging source root: npm create vite@latest . -- --template react --no-interactive. Use the same command with --template react-ts only when the confirmed design explicitly requires TypeScript; never ask the agent to choose a template interactively.',
  '- The official CLI scaffold runs through the existing Mobile Linux Shell/Bash tool when it is available. Do not add a Vite wrapper, scaffold MCP, or host-side project generator. After scaffold, inject the LingXi bridge, deviceContext, platform adapter, source policy, manifest, and business UI into the app source.',
  '- Do not overwrite an existing source root. If Shell is unavailable, reuse an existing source tree or use only the repository-verified .lingxi/vite-fallback/ copy as an explicit offline-fallback and report that direct Vite/npm commands did not run. If the official CLI cannot reach the registry, use that same offline-fallback and report the mode and reason; other CLI failures remain failures.',
  '- Generate only editable source. Do not edit package.json/package-lock.json in the Generate phase; use the existing Shell tool in the Dependencies phase. Preserve the official Vite index.html and vite.config.* unless a confirmed host integration requires a minimal compatible edit. Keep lib/lingxi-bridge.js host-controlled.',
  '- The page reaches host data/network/device ONLY through window.lingxi.v1 and the checked-in bridge adapter.',
  '- Manifest capabilities are exact enums: data_mutation, ui_control, camera, photo_library, microphone, location, notifications, llm, agent_notify. data_mutation is for conversation-agent mutate_data calls; page-owned window.lingxi.v1.data writes do not request it solely for storage.',
  '- For page storage, import queryCollection, upsertRecord, and deleteRecord from the locked bridge. Read app fields from records[].document. Never invent action/create/record mutation shapes.',
  '- localStorage must never be authoritative for a declared collection and must not hide a failed native write. Do not swallow bridge errors; surface a recoverable UI error and keep failed state retryable.',
  '- Build is offline. Dependencies are changed only by the existing Shell tool in this app workspace, using the exact confirmed specs for npm install/uninstall/ci; Shell keeps its existing network and command approval.',
  '- Source versioning uses ordinary workspace Git history plus the package-lock digest. Use the existing Git capability when available, otherwise standard git commands through Shell; do not add a second version store or command surface.',
  '- A checkpoint restore compares the current and target package-lock digest, reports the difference, and uses existing Shell npm ci only when the installed tree needs reconciliation.',
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
    scaffold_mode: { enum: ['official-cli', 'offline-fallback', 'existing'] },
    template: { enum: ['react', 'react-ts'] },
    summary: { type: 'string' },
  },
  required: ['targets', 'scaffold_mode', 'template', 'summary'],
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
    'Do not treat viewport, safe-area, color-scheme, reduced-motion, or input-mode as prompt facts. Those are dynamic runtime values that the generated app must read from window.lingxi.v1.deviceContext.',
    'Return template as exactly react or react-ts. Use react-ts only when the confirmed request/design explicitly names TypeScript; otherwise use react.',
    'For a new app with no package.json, after the design brief is confirmed use the existing Shell tool to scaffold in a truly empty temporary source directory under the app workspace. For the default JavaScript template run exactly: npm create vite@latest . -- --template react --no-interactive. For an explicitly confirmed TypeScript app run exactly: npm create vite@latest . -- --template react-ts --no-interactive. Copy the completed staging contents into the still-empty app source root only after verifying it contains no user source files; never overwrite an existing app.',
    'Because the app workspace already contains host metadata under .lingxi, the CLI target must be a newly created empty staging directory (for example, staging="$(mktemp -d .lingxi-vite-cli.XXXXXX)" followed by (cd "$staging" && npm create vite@latest . -- --template react --no-interactive)), not the non-empty workspace root. Clean up only that known staging directory after a successful controlled copy.',
    'If Shell is unavailable, do not invent a replacement CLI path. Reuse an existing source tree or clean the known staging directory, copy .lingxi/vite-fallback/. into the empty source root, and record scaffold_mode=offline-fallback plus the reason that Shell was unavailable. If the registry/network is unavailable, use that same fallback and report the reason. Do not use the fallback for unrelated CLI errors, and do not claim the official CLI ran when it did not. Existing projects with package.json use their existing scaffold and report scaffold_mode=existing.',
    'If the confirmed brief needs an original bitmap asset (photo, illustration, texture, hero, or background), conditionally detect ImageGen; when ready, record prompt/source/use and generate under public/. If unavailable, ask once whether to configure it or skip, then continue with CSS, gradients, user assets, or a placeholder without treating ImageGen as a hard dependency. Never use ImageGen for ordinary UI icons.',
    'The CLI scaffold is the only Design-phase source initialization. Do not install packages in this phase; Dependencies owns npm install/npm uninstall/npm ci.',
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

phase('Dependencies');
const dependenciesResult = await agent(
  [
    `Manage dependencies for local app "${appId}" using the confirmed design below.`,
    JSON.stringify(design),
    '',
    'Use the existing Shell tool from the current app workspace first to inspect package.json, package-lock.json, and the installed tree (for example npm ls --depth=0 --json). If Shell is unavailable, stop and report the exact dependency changes that would be required.',
    'For a freshly scaffolded official Vite project, run ordinary npm install in this same workspace when node_modules is absent or incomplete; the manifest and lockfile remain normal app files. Show any additional exact package specs before installing them.',
    'If the confirmed design needs packages not already present, show the exact npm package specs before executing them, then use the existing Shell tool in this workspace with npm install -- <exact shell-quoted specs> or npm uninstall -- <exact shell-quoted specs>. Stay within the existing Shell maximum timeout (600000ms). Its existing network/command approval and command logs apply; npm may execute lifecycle scripts as part of the explicitly approved command.',
    'Optional Tailwind (tailwindcss plus @tailwindcss/vite), Motion (motion imported as motion/react), and Lucide (lucide-react) are ordinary per-app proposals, never implicit dependencies.',
    'After a checkpoint restore, if package-lock.json differs from the current installed tree, use the same existing Shell tool in this workspace for npm ci. Do not create a wrapper, package-management abstraction, package store, SBOM, or new MCP/API surface.',
    'Use the existing Git capability for checkpoint status, diff, and source restore operations when available; otherwise use standard git commands through Shell. Preserve app data and keep package-lock.json in the normal workspace history.',
    'Reject empty, newline/NUL, option-like, ambiguous, or more-than-64 specs before invoking Shell; keep every displayed spec exact and never silently add a package.',
  ].join('\n'),
  { ...modelOptions, label: 'dependencies', phase: 'Dependencies', throwOnError: true },
);
const dependencies = requireAgentResult(dependenciesResult, 'the dependency step');

phase('Generate');
const generated = await agent(
  [
    'Generate the complete React implementation. Invoke $accessibility and $react-best-practices as independent reviewers while writing the source.',
    `Implement local app "${appId}" from the confirmed request, design, and dependency snapshot:`,
    confirmedSpec,
    JSON.stringify(design),
    JSON.stringify(dependencies),
    '',
    CONTRACT,
    '',
    'When the UI persists declared collection data, use the locked upsertRecord/deleteRecord helpers and queryCollection response shape. Read only records[].document for app fields. Do not make localStorage, IndexedDB, or an in-memory cache authoritative over native collection data. Do not swallow bridge errors or convert a rejected native write into UI success.',
    'Use platform tokens and adapters instead of scattered platform conditionals. Include loading, empty, error, success, disabled, offline, permission-denied, and reduced-motion states where relevant. Build actual copy and interaction paths, not a placeholder shell. Do not build or start anything in this phase.',
  ].join('\n'),
  { ...modelOptions, label: 'generate', phase: 'Generate', throwOnError: true },
);
// Without this check the Build phase would happily build the untouched
// scaffold, the host would stamp the app `ready`, and the workflow would
// report success for an app that is still a blank template.
requireAgentResult(generated, 'the source-generation step');

phase('Build');
let build = requirePreviewOnSuccess(await agent(
  [
    `Build local app "${appId}" with the host-owned offline builder.`,
    `Call mcp__local_apps__build with {"app_id":"${appId}"}.`,
    'If it fails, inspect mcp__local_apps__read_logs with log="build", fix only editable source files, and retry the build once in this phase. Do not run npm or enable network during build.',
    `When green, call mcp__local_apps__manage_runtime with {"app_id":"${appId}","action":"start"} and return its preview URL in preview_url. On build or runtime failure, return preview_url as an empty string.`,
    CONTRACT,
  ].join('\n'),
  {
    ...modelOptions,
    label: 'build',
    phase: 'Build',
    schema: BUILD_RESULT_SCHEMA,
    throwOnError: true,
  },
), 'initial build');

phase('Verify');
let verification = null;
let repairRounds = 0;
// The two allowed repair rounds are repair -> rebuild -> re-verify cycles. A remaining
// finding is returned honestly instead of being hidden by another iteration.
for (let round = 0; round <= 2; round += 1) {
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
  await agent(
    [
      `Repair the findings from frontend-qa for local app "${appId}".`,
      JSON.stringify(verification),
      CONTRACT,
      'Fix the smallest source-level cause. Do not install packages or edit workspace package files in the repair pass, and do not claim verification yet.',
    ].join('\n'),
    {
      ...modelOptions,
      label: `repair-${repairRounds}`,
      phase: 'Verify',
      throwOnError: true,
    },
  );
  build = requirePreviewOnSuccess(await agent(
    [
      `Rebuild local app "${appId}" after repair round ${repairRounds}.`,
      `Call mcp__local_apps__build with {"app_id":"${appId}"}; build is offline and may not invoke npm.`,
      `If the build succeeds, call mcp__local_apps__manage_runtime with {"app_id":"${appId}","action":"restart"} so the repaired build is served, and preserve the restarted runtime preview URL in preview_url. If the build fails, read the build log and do not restart the runtime.`,
      'Return one combined build/runtime result with ok, preview_url, and summary. On build failure, return preview_url as an empty string.',
      CONTRACT,
    ].join('\n'),
    {
      ...modelOptions,
      label: `rebuild-${repairRounds}`,
      phase: 'Verify',
      schema: BUILD_RESULT_SCHEMA,
      throwOnError: true,
    },
  ), `rebuild ${repairRounds}`);
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
  scaffold_mode: design && design.scaffold_mode,
  template: design && design.template,
  repair_rounds: repairRounds,
  verification,
  summary: 'Design, dependencies, generation, offline build, and frontend QA completed.',
};
