// Unified Local App workflow (lingxi-local-app:local-app-build).
// Host-owned launch context is the authority for profile, catalog and
// workspace identity. This script only orchestrates Plugin agents.
export const meta = {
  name: 'local-app-build',
  description: 'Create, update, or verify a Local App through the verified Plugin agent pipeline.',
  phases: [{ title: 'Select and Design' }, { title: 'Generate and Build' }, { title: 'Operate and Verify' }],
};

const WORKFLOW_ID = 'lingxi-local-app:local-app-build';
const ALLOWED_EXTERNAL = ['operation', 'app_id', 'spec', 'revision_prompt', 'quality_level'];
const INTERNAL_KEYS = ['host_context', 'workflow_run_id', 'selector_capability', 'validated_selection_handle', 'expected_writable_collections', 'runtime_profile', 'template_selection'];
const QUALITY = ['fast', 'balanced', 'thorough'];
const FINDING_KINDS = ['render', 'motion', 'data', 'webview', 'acceptance', 'build'];
const HOST_CHROME_CONTRACT = 'The host draws NO chrome around a running app: the app must provide every visible title, navigation and back affordance. The host floats ONE control over the BOTTOM-LEADING corner, so keep the leading 80 CSS px by the bottom 80 CSS px clear from the safe area and keep time-critical controls off its temporary expansion strip.';
const input = args && typeof args === 'object' && !Array.isArray(args) ? args : {};
const unknown = Object.keys(input).filter((key) => !ALLOWED_EXTERNAL.includes(key) && !INTERNAL_KEYS.includes(key));
if (unknown.length > 0) throw new Error(`${WORKFLOW_ID}: unknown external field(s): ${unknown.join(', ')}`);
if (!['create', 'update', 'verify'].includes(input.operation)) throw new Error(`${WORKFLOW_ID}: operation must be create, update, or verify`);
if (typeof input.app_id !== 'string' || input.app_id.trim().length === 0) throw new Error(`${WORKFLOW_ID}: app_id is required`);
if (input.spec !== undefined && (typeof input.spec !== 'string' || input.spec.trim().length === 0)) throw new Error(`${WORKFLOW_ID}: spec must be a non-empty confirmed specification`);
const quality = input.quality_level === undefined ? 'balanced' : input.quality_level;
if (!QUALITY.includes(quality)) throw new Error(`${WORKFLOW_ID}: quality_level must be fast, balanced, or thorough`);

const context = input.host_context;
if (!context || typeof context !== 'object' || context.source !== 'verified_host') throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_REQUIRED: launch context must be injected by Host`);
if (context.app_id !== input.app_id || context.operation !== input.operation) throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_MISMATCH: app and operation are Host-bound`);
if (typeof context.workflow_run_id !== 'string' || context.workflow_run_id.length === 0) throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_MISSING_RUN: workflow_run_id is Host-bound`);
const persisted = input.operation !== 'create';
if (persisted && (!context.runtime_profile || context.dependency_snapshot?.verified !== true)) throw new Error(`${WORKFLOW_ID}: PERSISTED_PROFILE_REQUIRED: update/verify require Host-persisted profile and dependency snapshot`);
if (input.operation === 'create' && (typeof context.invocation_capability !== 'string' || !/^mcpv_[A-Za-z0-9]{32}$/.test(context.invocation_capability))) throw new Error(`${WORKFLOW_ID}: HOST_INVOCATION_CAPABILITY_REQUIRED: create must carry a Host-minted MCP authoring capability`);
const catalog = context.template_catalog;
if (!catalog || typeof catalog.catalog_digest !== 'string' || !Array.isArray(catalog.available_template_ids)) throw new Error(`${WORKFLOW_ID}: VERIFIED_CATALOG_REQUIRED: Host must inject catalog identity`);
const profile = () => context.runtime_profile;
if (persisted && !['react_dom', 'canvas_2d', 'three_3d', 'phaser_2d', 'babylon_3d'].includes(profile()?.family)) throw new Error(`${WORKFLOW_ID}: PERSISTED_PROFILE_INVALID: Host runtime profile family is missing or unsupported`);
if (persisted && quality === 'fast' && profile().family !== 'react_dom') throw new Error(`${WORKFLOW_ID}: CANVAS_FAST_REJECTED: canvas profiles require balanced or thorough quality`);
let selectedTemplateId = '';
const canvas = () => selectedTemplateId !== 'react-dom-r1' && (selectedTemplateId.length > 0 || profile()?.surface === 'canvas' || (profile()?.family && profile().family !== 'react_dom'));
const repairBudget = quality === 'thorough' ? 2 : 1;
const specialistFor = () => {
  const family = profile()?.family || (selectedTemplateId === 'react-dom-r1' ? 'react_dom' : selectedTemplateId === 'canvas-2d-r1' ? 'canvas_2d' : selectedTemplateId === 'three-3d-r1' ? 'three_3d' : selectedTemplateId === 'phaser-2d-r1' ? 'phaser_2d' : selectedTemplateId === 'babylon-3d-r1' ? 'babylon_3d' : 'react_dom');
  return { react_dom: '$ionic-react-local-app', canvas_2d: '$canvas-2d-local-app', three_3d: '$threejs-local-app', phaser_2d: '$phaser-2d-local-app', babylon_3d: '$babylon-3d-local-app' }[family] || '$ionic-react-local-app';
};

const requireObject = (value, stage) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${WORKFLOW_ID}: ${stage} returned malformed structured output`);
  return value;
};
const selectionSchema = { type: 'object', properties: { catalog_digest: { type: 'string', minLength: 1 }, template_id: { type: 'string', minLength: 1 }, reason: { type: 'string', minLength: 1 }, rejected: { type: 'array', items: { type: 'object', properties: { template_id: { type: 'string', minLength: 1 }, reason: { type: 'string', minLength: 1 } }, required: ['template_id', 'reason'], additionalProperties: false } }, validated_selection_handle: { type: 'string', pattern: '^vsel_[A-Za-z0-9]{32}$' } }, required: ['catalog_digest', 'template_id', 'reason', 'rejected', 'validated_selection_handle'], additionalProperties: false };
const designSchema = { type: 'object', properties: { runtime_family: { type: 'string' }, acceptance_checks: { type: 'array' }, summary: { type: 'string' } }, required: ['runtime_family', 'acceptance_checks', 'summary'] };
const createStageSchema = { type: 'object', properties: { ok: { type: 'boolean' }, dependency_input_sha256: { type: 'string' }, summary: { type: 'string' } }, required: ['ok', 'summary'], additionalProperties: false };
const buildSchema = { type: 'object', properties: { ok: { type: 'boolean' }, preview_url: { type: 'string' }, summary: { type: 'string' } }, required: ['ok', 'preview_url', 'summary'] };
const promoteSchema = { type: 'object', properties: { ok: { type: 'boolean' }, verification_sha256: { type: 'string' }, catalog_sha256: { type: 'string' }, publication_state: { type: 'string' }, summary: { type: 'string' } }, required: ['ok', 'publication_state', 'summary'], additionalProperties: false };
const findingSchema = { type: 'object', properties: { kind: { type: 'string', enum: FINDING_KINDS }, severity: { type: 'string', const: 'blocking' }, evidence: { type: 'string', minLength: 1 } }, required: ['kind', 'severity', 'evidence'], additionalProperties: false };
const reportSchema = { type: 'object', properties: { ok: { type: 'boolean' }, findings: { type: 'array', items: findingSchema }, checked_matrix: { type: 'array' }, browser_available: { type: 'boolean' }, webview_checked: { type: 'boolean' }, degraded_verification: { type: 'boolean' }, data_roundtrip: { type: 'object' }, render_check: { type: 'object' }, motion_check: { type: 'object' }, summary: { type: 'string' } }, required: ['ok', 'findings', 'checked_matrix', 'browser_available', 'webview_checked', 'degraded_verification', 'data_roundtrip', 'render_check', 'motion_check', 'summary'] };
const normalizeFinding = (value, kind = 'acceptance') => {
  if (typeof value === 'string' && value.trim()) return { kind, severity: 'blocking', evidence: value.trim() };
  if (!value || typeof value !== 'object' || Array.isArray(value)) return { kind, severity: 'blocking', evidence: 'malformed finding: expected object or non-empty string' };
  const findingKind = FINDING_KINDS.includes(value.kind) ? value.kind : kind;
  const evidence = typeof value.evidence === 'string' && value.evidence.trim() ? value.evidence.trim() : 'malformed finding: missing evidence';
  return { kind: findingKind, severity: 'blocking', evidence };
};
const countAtLeast = (value, minimum) => Number.isFinite(Number(value)) && Number(value) >= minimum;
const blockingFindings = (report) => {
  const findings = Array.isArray(report.findings) ? report.findings.map((value) => normalizeFinding(value)) : [normalizeFinding('malformed finding container: findings must be an array')];
  if (report.webview_checked !== true) findings.push(normalizeFinding('webview_checked=false: native WebView path was not proven', 'webview'));
  if (Array.isArray(context.expected_writable_collections) && context.expected_writable_collections.length > 0 && report.data_roundtrip?.status !== 'passed') findings.push(normalizeFinding('data round-trip did not pass for a Host-declared collection', 'data'));
  if (canvas() && (report.render_check?.status !== 'passed' || !countAtLeast(report.render_check?.canvas_surfaces, 1) || !countAtLeast(report.render_check?.frames_captured, 1))) findings.push(normalizeFinding('canvas render policy gate did not pass: require at least one surface and one captured frame', 'render'));
  if (canvas() && (report.motion_check?.status !== 'passed' || !countAtLeast(report.motion_check?.frames_compared, 2))) findings.push(normalizeFinding('canvas motion policy gate did not pass: require at least two compared frames', 'motion'));
  if (!canvas() && report.render_check?.status !== 'passed' && report.render_check?.status !== 'not_applicable') findings.push(normalizeFinding('DOM render policy gate did not pass', 'render'));
  if (report.ok !== true && findings.length === 0) findings.push(normalizeFinding('verification report returned ok=false without a blocking finding'));
  return findings;
};
let calls = 0;
const run = async (prompt, options) => {
  calls += 1;
  return requireObject(await agent(prompt, { ...options, workflowId: WORKFLOW_ID, throwOnError: true }), options.agentType);
};

let selection;
let designSpec = null;
if (input.operation === 'create') {
  selection = await run(`Read LocalAppTemplateCatalog, choose the simplest available template for app ${input.app_id}, then call LocalAppValidateTemplateSelection with Host catalog_digest, template_id, reason, rejected, app_id=${input.app_id}, workflow_run_id=${context.workflow_run_id}, selector_capability=${context.selector_capability}. Never invent caller roles, and never provide family, revision, path or digest from your own output. Return the Host-issued handle unchanged. Confirmed specification: ${input.spec || ''}`, { agentType: 'template-selector', label: 'template-selector', phase: 'Select and Design', schema: selectionSchema });
  if (selection.catalog_digest !== catalog.catalog_digest || !catalog.available_template_ids.includes(selection.template_id)) throw new Error(`${WORKFLOW_ID}: selector returned a stale or unavailable template`);
  if (typeof selection.validated_selection_handle !== 'string' || !selection.validated_selection_handle.startsWith('vsel_')) throw new Error(`${WORKFLOW_ID}: selector did not return a Host-issued validated_selection_handle`);
  selectedTemplateId = selection.template_id;
  if (quality === 'fast' && selection.template_id !== 'react-dom-r1') throw new Error(`${WORKFLOW_ID}: CANVAS_FAST_REJECTED: selector chose a canvas profile for fast quality`);
  if (quality !== 'fast') designSpec = await run(`Resolve the Host selection through LocalAppResolveTemplateSelection for app ${input.app_id}, workflow_run_id ${context.workflow_run_id}, handle ${selection.validated_selection_handle}; produce the platform-aware design spec for the resolved profile. ${HOST_CHROME_CONTRACT}`, { agentType: 'designer', label: 'designer', phase: 'Select and Design', schema: designSchema });
}

let build;
let createApproval;
if (input.operation === 'create') {
  const handle = selection?.validated_selection_handle;
  if (!handle) throw new Error(`${WORKFLOW_ID}: CREATE_HANDLE_REQUIRED`);
  const staged = await run(`Resolve the Host selection through LocalAppResolveTemplateSelection before any write. Call LocalAppStageCreate with app_id=${input.app_id}, workflow_run_id=${context.workflow_run_id}, validated_selection_handle=${handle}, quality_level=${quality}, and the structured design_spec ${JSON.stringify(designSpec)} when present. Verify dependency_input_sha256 and prepare only the run-scoped isolated staging candidate. Do not call LocalAppScaffold, LocalAppBuild or LocalAppRuntime yet, and do not write the real app workspace before native approval. Spec: ${input.spec || ''}`, { agentType: 'builder', label: 'builder-stage', phase: 'Generate and Build', schema: createStageSchema });
  if (staged.ok !== true) throw new Error(`${WORKFLOW_ID}: create staging did not succeed`);
  createApproval = requireObject(await workflow('lingxi-local-app:local-app-mcp-authoring', {
    app_id: input.app_id,
    user_goal: input.spec || input.revision_prompt || 'Initial Local App MCP surface',
    host_context: {
      source: 'verified_host',
      operation: 'initial',
      app_id: input.app_id,
      workflow_run_id: context.workflow_run_id,
      invocation_capability: context.invocation_capability,
      template_catalog: context.template_catalog,
      expected_writable_collections: context.expected_writable_collections || [],
      dependency_snapshot: { verified: false },
      active_catalog: null
    }
  }), 'nested-mcp-authoring');
  if (createApproval.status === 'mcp_authoring_required') {
    return { ok: true, workflow_id: WORKFLOW_ID, operation: input.operation, app_id: input.app_id, quality_level: quality, agent_calls: calls, repair_rounds: 0, status: createApproval.status, candidate_preserved: true, summary: createApproval.reason || 'Initial MCP authoring did not yield a bounded publishable tool surface.' };
  }
  if (createApproval.status !== 'create_approved' || !createApproval.approval?.receipt_id) throw new Error(`${WORKFLOW_ID}: create approval did not yield a unified scaffold receipt`);
  build = await run(`Read LocalAppGet for app ${input.app_id} so the Host-confirmed current name and brief are the only values that reach the create transaction. Then call LocalAppScaffold with app_id=${input.app_id}, workflow_run_id=${context.workflow_run_id}, receipt_id=${createApproval.approval.receipt_id}, and that exact Host-confirmed name/brief. Only after scaffold succeeds may you invoke exactly the matching runtime specialist ${specialistFor()} to implement the app workspace, then call LocalAppBuild and LocalAppRuntime. ${HOST_CHROME_CONTRACT} Do not issue any second approval flow or publish directly. Spec: ${input.spec || ''}`, { agentType: 'builder', label: 'builder-build', phase: 'Generate and Build', schema: buildSchema });
  if (build.ok !== true || typeof build.preview_url !== 'string' || !build.preview_url.trim()) throw new Error(`${WORKFLOW_ID}: create builder did not produce a successful preview`);
} else if (input.operation !== 'verify') {
  build = await run(`Use only the Host-persisted runtime profile in host_context; do not reselect or resolve a create candidate. Invoke exactly the matching runtime specialist ${specialistFor()} for that persisted profile. Implement the confirmed Local App update for ${input.app_id}; use only App-managed files, then call LocalAppBuild and LocalAppRuntime. ${HOST_CHROME_CONTRACT} Persist no final receipt or publish. Host context: ${JSON.stringify(context)}. Revision: ${input.revision_prompt || ''}`, { agentType: 'builder', label: 'builder', phase: 'Generate and Build', schema: buildSchema });
  if (build.ok !== true || typeof build.preview_url !== 'string' || !build.preview_url.trim()) throw new Error(`${WORKFLOW_ID}: builder did not produce a successful preview`);
}

let report;
let repairRounds = 0;
for (;;) {
  const identityInstruction = selection?.validated_selection_handle
    ? `Resolve the Host selection with LocalAppResolveTemplateSelection using handle ${selection.validated_selection_handle}.`
    : 'Use only the Host-persisted runtime profile in host_context; do not call the create-only template-selection resolver.';
  const operator = await run(`${identityInstruction} Drive app ${input.app_id} through bounded scenarios and collect raw runtime, DOM/canvas, render, motion, data and WebView evidence. Verify the app supplies its own navigation affordances and respects the host's bottom-leading keep-clear region.`, { agentType: 'operator', label: `operator-${repairRounds}`, phase: 'Operate and Verify', schema: reportSchema });
  const tester = await run(`${identityInstruction} Check operator evidence for app ${input.app_id} against acceptance checks. Render/motion/data/webview are ordinary blocking findings. Operator evidence: ${JSON.stringify(operator)}`, { agentType: 'tester', label: `tester-${repairRounds}`, phase: 'Operate and Verify', schema: reportSchema });
  report = await run(`${identityInstruction} Validate operator and tester evidence for app ${input.app_id}; do not repair source. Return findings as structured blocking findings and set render_check/motion_check/data_roundtrip/webview_checked truthfully. Evidence: ${JSON.stringify(tester)}`, { agentType: 'verifier', label: `verifier-${repairRounds}`, phase: 'Operate and Verify', schema: reportSchema });
  const findings = blockingFindings(report);
  if (report.ok === true && findings.length === 0) break;
  if (input.operation === 'verify') {
    return { ok: false, workflow_id: WORKFLOW_ID, operation: input.operation, app_id: input.app_id, quality_level: quality, agent_calls: calls, repair_rounds: 0, status: 'verification_failed', findings, verification: report, preview_url: '', summary: report.summary };
  }
  if (repairRounds >= repairBudget) throw new Error(`${WORKFLOW_ID}: verification still has findings after ${repairBudget} repair round(s): ${JSON.stringify(findings)}`);
  repairRounds += 1;
  build = await run(`Repair only blocking findings for app ${input.app_id}, resolve the Host profile/selection first, preserve Host-managed files and dependencies, and preserve this layout contract: ${HOST_CHROME_CONTRACT} Rebuild and restart. Findings: ${JSON.stringify(findings)}`, { agentType: 'builder', label: `repair-${repairRounds}`, phase: 'Generate and Build', schema: buildSchema });
  if (build.ok !== true || typeof build.preview_url !== 'string' || !build.preview_url.trim()) throw new Error(`${WORKFLOW_ID}: repair builder did not produce a successful preview`);
}

let promotion;
let mcpUpdate = null;
if (input.operation === 'update') {
  if (typeof context.invocation_capability !== 'string' || !/^mcpv_[A-Za-z0-9]{32}$/.test(context.invocation_capability)) throw new Error(`${WORKFLOW_ID}: HOST_INVOCATION_CAPABILITY_REQUIRED: update MCP impact-check requires a Host-minted capability`);
  mcpUpdate = requireObject(await workflow('lingxi-local-app:local-app-mcp-authoring', {
    app_id: input.app_id,
    user_goal: input.revision_prompt || input.spec || 'Reconcile the Local App MCP surface with this update',
    host_context: {
      source: 'verified_host',
      operation: 'revise',
      app_id: input.app_id,
      workflow_run_id: context.workflow_run_id,
      invocation_capability: context.invocation_capability,
      runtime_profile: context.runtime_profile,
      template_catalog: context.template_catalog,
      expected_writable_collections: context.expected_writable_collections || [],
      dependency_snapshot: context.dependency_snapshot,
      active_catalog: context.active_catalog || null
    }
  }), 'nested-mcp-authoring');
  if (['needs_input', 'mcp_authoring_required', 'approval_required'].includes(mcpUpdate.status)) {
    return { ok: false, workflow_id: WORKFLOW_ID, operation: input.operation, app_id: input.app_id, quality_level: quality, agent_calls: calls, repair_rounds: repairRounds, status: mcpUpdate.status, candidate_preserved: true, verification: report, mcp_update: mcpUpdate, preview_url: build?.preview_url || '', summary: mcpUpdate.reason || 'MCP impact-check requires follow-up before publication.' };
  }
  if (mcpUpdate.status !== 'promoted') throw new Error(`${WORKFLOW_ID}: update MCP impact-check returned an unsupported status: ${mcpUpdate.status}`);
}
if (input.operation === 'create') {
  promotion = await run(`Use LocalAppQaMcpCandidate for app ${input.app_id}, workflow run ${context.workflow_run_id}, then LocalAppPromoteMcpCandidate without a receipt_id. The unified create receipt was already consumed by LocalAppScaffold; this final stage must only QA the built candidate and atomically promote the build/catalog pair. Return verification_sha256, catalog_sha256 and publication_state.`, { agentType: 'verifier', label: 'mcp-promote', phase: 'Operate and Verify', schema: promoteSchema });
  if (promotion.ok !== true || promotion.publication_state !== 'published_unverified') throw new Error(`${WORKFLOW_ID}: create promotion did not publish the verified candidate`);
}

return { ok: true, workflow_id: WORKFLOW_ID, operation: input.operation, app_id: input.app_id, quality_level: quality, agent_calls: calls, repair_rounds: repairRounds, verification: report, approval: createApproval || null, mcp_update: mcpUpdate, promotion: promotion || null, preview_url: build?.preview_url || '', summary: report.summary };
