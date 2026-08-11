export const meta = {
  name: 'local-app-build',
  description: 'Implement a confirmed local-app spec: write the source, build until green, and start the preview.',
  phases: [
    { title: 'Generate' },
    { title: 'Build' },
  ],
};

// args: { app_id: string, spec: string|object, revision_prompt?: string }
const input = args && typeof args === 'object' ? args : {};
const appId = typeof input.app_id === 'string' ? input.app_id : '';
if (!appId) {
  throw new Error('local-app-build requires args.app_id (the app to implement)');
}
// The spec is the ONLY description of what to build, and the skill's flow
// requires the user to have approved it. An absent/empty spec would leave the
// generator free to invent the app, which is exactly the confirmation step
// this segment must not bypass.
const spec = typeof input.spec === 'string' ? input.spec : JSON.stringify(input.spec ?? '');
if (!spec || spec.trim().length === 0 || spec === '""' || spec === '{}') {
  throw new Error(
    'local-app-build requires args.spec (the spec the user confirmed) — refusing to invent one',
  );
}
const revision = typeof input.revision_prompt === 'string' ? input.revision_prompt : '';

const CONTRACT = [
  'Workspace contract (violations break the app):',
  '- Edit ONLY files under app/, components/, lib/, styles/, public/ in the current workspace.',
  '- NEVER touch the locked files: package.json, package-lock.json, vite.config.mjs, index.html, app/main.jsx, lib/lingxi-bridge.js.',
  '- The page reaches host data/network/device ONLY through the window.lingxi.v1 bridge (see lib/lingxi-bridge.js and the workspace LINGXI.md).',
  '- No new npm dependencies: the offline runtime ships a fixed node_modules.',
  '- Data collections, network domains and capabilities must be declared through mcp__local_apps__update_manifest before the page relies on them.',
].join('\n');

phase('Generate');
const generated = await agent(
  [
    revision
      ? `Revise local app "${appId}" per this user feedback:\n${revision}\n\nOriginal confirmed spec, for context:`
      : `Implement local app "${appId}" from this confirmed spec:`,
    spec,
    '',
    CONTRACT,
    '',
    'Read the existing scaffold first, then write complete, working React source for everything the spec names. Prefer small focused components under components/. Do not build or start anything in this step.',
  ].join('\n'),
  { label: 'generate', phase: 'Generate' },
);
// A subagent that dies (API error, kill, retry exhaustion) does NOT throw —
// agent() resolves to null. Without this check the Build phase would happily
// build the untouched template, the host would stamp the app `ready`, and the
// workflow would report success for an app that is still a blank scaffold.
if (!generated) {
  throw new Error(
    `local-app-build: the source-generation step produced no result for app "${appId}"; ` +
      'the workspace was not implemented. Re-run with resumeFromRunId to retry from here.',
  );
}

phase('Build');
const outcome = await agent(
  [
    `Make local app "${appId}" build and serve.`,
    `1. Call mcp__local_apps__build with {"app_id":"${appId}"}.`,
    '2. If the build fails: read the error summary (and mcp__local_apps__read_logs with log="build" when it helps), fix the source files, and build again — up to 3 attempts total.',
    `3. When the build is green, call mcp__local_apps__manage_runtime with {"app_id":"${appId}","action":"start"} and capture the preview url it returns.`,
    '',
    CONTRACT,
  ].join('\n'),
  {
    label: 'build-and-fix',
    phase: 'Build',
    schema: {
      type: 'object',
      properties: {
        ok: { type: 'boolean' },
        preview_url: { type: 'string' },
        summary: { type: 'string' },
      },
      required: ['ok', 'summary'],
    },
  },
);

// The task's terminal status comes from whether this SCRIPT throws — an
// `Ok` return is reported as Completed no matter what it contains. So a
// dead agent (null) or a self-declared failure (`ok:false`) has to throw,
// otherwise the parent session's task notification announces a successful
// build for an app that never built.
if (!outcome) {
  throw new Error(
    `local-app-build: the build step produced no result for app "${appId}". ` +
      'Re-run with resumeFromRunId to retry from here.',
  );
}
if (outcome.ok !== true) {
  throw new Error(
    `local-app-build: the build did not succeed for app "${appId}": ${outcome.summary || 'no summary'}`,
  );
}
if (typeof outcome.preview_url !== 'string' || outcome.preview_url.length === 0) {
  throw new Error(
    `local-app-build: the build reported success for app "${appId}" but returned no preview url; ` +
      'the preview was never started.',
  );
}

return outcome;
