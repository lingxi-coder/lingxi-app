// Structured evidence workflow for an existing, persisted Local App.
export const meta = {
  name: 'local-app-use-test',
  description: 'Operate a Local App and grade its acceptance scenarios with Host-bound evidence.',
  phases: [{ title: 'Operate' }, { title: 'Test' }, { title: 'Verify' }],
};

const WORKFLOW_ID = 'lingxi-local-app:local-app-use-test';
const input = args && typeof args === 'object' && !Array.isArray(args) ? args : {};
const allowed = ['app_id', 'scope', 'scenarios', 'quality_level', 'host_context', 'workflow_run_id', 'runtime_profile', 'expected_writable_collections'];
const unknown = Object.keys(input).filter((key) => !allowed.includes(key));
if (unknown.length) throw new Error(`${WORKFLOW_ID}: unknown field(s): ${unknown.join(', ')}`);
if (typeof input.app_id !== 'string' || !input.app_id.trim()) throw new Error(`${WORKFLOW_ID}: app_id is required`);
if (typeof input.scope !== 'string' || !input.scope.trim()) throw new Error(`${WORKFLOW_ID}: scope is required`);
if (!Array.isArray(input.scenarios) || !input.scenarios.length || input.scenarios.some((scenario) => !scenario || typeof scenario !== 'object' || typeof scenario.name !== 'string' || !scenario.name.trim())) throw new Error(`${WORKFLOW_ID}: scenarios must be a non-empty array of named scenarios`);
const context = input.host_context;
if (!context || context.source !== 'verified_host' || context.app_id !== input.app_id) throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_REQUIRED: profile identity must be Host-injected`);
const profile = input.runtime_profile || context.runtime_profile;
if (!profile || typeof profile.family !== 'string') throw new Error(`${WORKFLOW_ID}: PERSISTED_PROFILE_REQUIRED`);
const isCanvas = profile.surface === 'canvas' || profile.family !== 'react_dom';
const quality = input.quality_level === undefined ? 'balanced' : input.quality_level;
if (!['fast', 'balanced', 'thorough'].includes(quality)) throw new Error(`${WORKFLOW_ID}: quality_level must be fast, balanced, or thorough`);
if (isCanvas && quality === 'fast') throw new Error(`${WORKFLOW_ID}: CANVAS_FAST_REJECTED`);

const findingSchema = { type: 'object', properties: { kind: { type: 'string', enum: ['render', 'motion', 'data', 'webview', 'acceptance', 'build'] }, severity: { type: 'string', const: 'blocking' }, evidence: { type: 'string', minLength: 1 } }, required: ['kind', 'severity', 'evidence'], additionalProperties: false };
const reportSchema = { type: 'object', properties: { ok: { type: 'boolean' }, findings: { type: 'array', items: findingSchema }, checked_matrix: { type: 'array' }, browser_available: { type: 'boolean' }, webview_checked: { type: 'boolean' }, degraded_verification: { type: 'boolean' }, data_roundtrip: { type: 'object' }, render_check: { type: 'object' }, motion_check: { type: 'object' }, summary: { type: 'string' } }, required: ['ok', 'findings', 'checked_matrix', 'browser_available', 'webview_checked', 'degraded_verification', 'data_roundtrip', 'render_check', 'motion_check', 'summary'] };
const run = async (prompt, options) => {
  const result = await agent(prompt, { ...options, workflowId: WORKFLOW_ID, throwOnError: true });
  if (!result || typeof result !== 'object' || Array.isArray(result)) throw new Error(`${WORKFLOW_ID}: ${options.agentType} returned malformed structured output`);
  return result;
};
phase('Operate');
const operator = await run(`Use only the Host-persisted runtime profile in host_context; do not call the create-only template-selection resolver. Operate app ${input.app_id} for these bounded scenarios: ${JSON.stringify(input.scenarios)}. Gather raw runtime, DOM/canvas, frame, motion, data and WebView evidence. Do not judge pass/fail. Host context=${JSON.stringify(context)}`, { agentType: 'operator', label: 'operator', phase: 'Operate', schema: reportSchema });
phase('Test');
const tester = await run(`Check operator evidence for app ${input.app_id} against every acceptance scenario. Render, motion, data and WebView are blocking policy gates. Keep malformed evidence as a blocking finding; never turn missing fields into a pass. Operator evidence=${JSON.stringify(operator)}`, { agentType: 'tester', label: 'tester', phase: 'Test', schema: reportSchema });
phase('Verify');
const report = await run(`Verify the operator/tester evidence for app ${input.app_id} and return a truthful structured report. Do not edit or repair source. For Canvas/Three/Phaser/Babylon report render_check and motion_check; render not_applicable is valid only for DOM with zero canvas surfaces. Include frames=0, zero surfaces, missing motion, and frames<2 as blocking evidence when applicable. Host context=${JSON.stringify(context)}. Tester evidence=${JSON.stringify(tester)}`, { agentType: 'verifier', label: 'verifier', phase: 'Verify', schema: reportSchema });
if (!Array.isArray(report.findings)) throw new Error(`${WORKFLOW_ID}: malformed finding container`);
const countAtLeast = (value, minimum) => typeof value === 'number' && Number.isFinite(value) && value >= minimum;
const findings = report.findings.map((value) => (value && typeof value === 'object' && !Array.isArray(value) && typeof value.evidence === 'string' && value.evidence.trim()) ? value : { kind: 'acceptance', severity: 'blocking', evidence: 'malformed finding' });
if (report.webview_checked !== true) findings.push({ kind: 'webview', severity: 'blocking', evidence: 'webview_checked=false' });
if (isCanvas && (report.render_check?.status !== 'passed' || !countAtLeast(report.render_check?.canvas_surfaces, 1) || !countAtLeast(report.render_check?.frames_captured, 1))) findings.push({ kind: 'render', severity: 'blocking', evidence: 'render policy gate requires one surface and one captured frame' });
if (isCanvas && (report.motion_check?.status !== 'passed' || !countAtLeast(report.motion_check?.frames_compared, 2))) findings.push({ kind: 'motion', severity: 'blocking', evidence: 'motion policy gate requires two compared frames' });
if (Array.isArray(input.expected_writable_collections) && input.expected_writable_collections.length && report.data_roundtrip?.status !== 'passed') findings.push({ kind: 'data', severity: 'blocking', evidence: 'data round-trip did not pass for a declared collection' });
if (findings.length || report.ok !== true) throw new Error(`${WORKFLOW_ID}: verification failed: ${JSON.stringify(findings)}`);
return { ...report, ok: true, workflow_id: WORKFLOW_ID, app_id: input.app_id, quality_level: quality, operator, tester };
