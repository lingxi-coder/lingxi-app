// Per-App MCP authoring (lingxi-local-app:local-app-mcp-authoring).
// The workflow owns orchestration; Host-owned tools remain the authority for
// validation, receipts, candidate persistence and promotion.
export const meta = {
  name: 'local-app-mcp-authoring',
  description: 'Author, validate, QA and promote one Local App MCP catalog with Host-bound evidence.',
  phases: [{ title: 'Evidence and proposal' }, { title: 'Validate and approve' }, { title: 'QA and promote' }],
};

const WORKFLOW_ID = 'lingxi-local-app:local-app-mcp-authoring';
const EXTERNAL_KEYS = ['app_id', 'user_goal'];
// `host_context` is injected by the trusted Host; every other top-level key
// is user/model input and must be rejected, including operation/quality/run
// identity claims.
const INTERNAL_KEYS = ['host_context'];
const FORBIDDEN = ['tools', 'tool_definitions', 'server_name', 'annotations', 'permission_rules', 'flow_id', 'workspace_path', 'proposal_digest', 'catalog_digest', 'digests'];
const QUALITY = ['fast', 'balanced', 'thorough'];
const input = args && typeof args === 'object' && !Array.isArray(args) ? args : {};
const unknown = Object.keys(input).filter((key) => !EXTERNAL_KEYS.includes(key) && !INTERNAL_KEYS.includes(key));
if (unknown.length) throw new Error(`${WORKFLOW_ID}: external input is limited to app_id,user_goal; unknown field(s): ${unknown.join(', ')}`);
const forged = FORBIDDEN.filter((key) => Object.prototype.hasOwnProperty.call(input, key));
if (forged.length) throw new Error(`${WORKFLOW_ID}: rejects caller-supplied Host-owned field(s): ${forged.join(', ')}`);
if (typeof input.app_id !== 'string' || !input.app_id.trim()) throw new Error(`${WORKFLOW_ID}: app_id is required`);
if (typeof input.user_goal !== 'string' || !input.user_goal.trim()) throw new Error(`${WORKFLOW_ID}: user_goal is required`);
const context = input.host_context;
if (!context || context.source !== 'verified_host' || context.app_id !== input.app_id) throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_REQUIRED: Host must inject AppEvidence and run identity`);
if (typeof context.workflow_run_id !== 'string' || !context.workflow_run_id.trim()) throw new Error(`${WORKFLOW_ID}: HOST_CONTEXT_MISSING_RUN`);
if (typeof context.invocation_capability !== 'string' || !/^mcpv_[A-Za-z0-9]{32}$/.test(context.invocation_capability)) throw new Error(`${WORKFLOW_ID}: HOST_INVOCATION_CAPABILITY_REQUIRED: only a Host-minted workflow capability may start authoring`);
const operation = context.operation || 'initial';
const quality = context.quality_level || 'balanced';
if (!QUALITY.includes(quality)) throw new Error(`${WORKFLOW_ID}: quality_level must be fast, balanced, or thorough`);

const proposalSchema = {
  type: 'object',
  properties: {
    app_id: { type: 'string', minLength: 1 }, manifest_revision: { type: 'integer', minimum: 0 }, user_goal_sha256: { type: 'string', pattern: '^[0-9a-f]{64}$' }, summary: { type: 'string', minLength: 1 }, tools: { type: 'array' }, required_flow_changes: { type: 'array', items: { type: 'string' } }, excluded_capabilities: { type: 'array', items: { type: 'string' } },
  },
  required: ['app_id', 'manifest_revision', 'user_goal_sha256', 'summary', 'tools', 'required_flow_changes', 'excluded_capabilities'], additionalProperties: false,
};
const qaSchema = {
  type: 'object', properties: { ok: { type: 'boolean' }, findings: { type: 'array' }, mcp_schema: { type: 'string' }, flow_binding: { type: 'string' }, calls: { type: 'string' }, isolation: { type: 'string' }, verification_sha256: { type: 'string' }, summary: { type: 'string' } },
  required: ['ok', 'findings', 'mcp_schema', 'flow_binding', 'calls', 'isolation', 'verification_sha256', 'summary'], additionalProperties: false,
};
const objectResult = (value, stage) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${WORKFLOW_ID}: ${stage} returned malformed structured output`);
  return value;
};
const run = async (prompt, options) => objectResult(await agent(prompt, { ...options, workflowId: WORKFLOW_ID, throwOnError: true }), options.agentType);

// Evidence is assembled by the Host, not from caller paths or model claims.
const evidence = await run(`Read Host-owned AppEvidence for app ${input.app_id}. Use LocalAppGet and the app's own bounded data only. Return persisted profile/build/catalog identities without inventing paths, digests or server metadata. Host context: ${JSON.stringify(context)}`, { agentType: 'mcp-designer', label: 'app-evidence', phase: 'Evidence and proposal', schema: { type: 'object', properties: { summary: { type: 'string' }, runtime_profile: { type: 'object' }, collections: { type: 'array' }, active_catalog: { type: ['object', 'null'] } }, required: ['summary', 'runtime_profile', 'collections', 'active_catalog'], additionalProperties: false } });
const proposal = await run(`Design an AppMcpProposal for app ${input.app_id} and user goal ${input.user_goal}. Return only app_id, manifest_revision, user_goal_sha256, summary, tools with name/title/description/input_schema/output_schema and semantic Flow references, required_flow_changes, and excluded_capabilities. Do not return server, annotations, icons, execution, _meta, permissions, ceiling, paths, receipts or catalog digests. If a meaningful bounded Flow is unavailable, return required_flow_changes or excluded_capabilities instead of inventing one. Evidence: ${JSON.stringify(evidence)}`, { agentType: 'mcp-designer', label: 'mcp-designer', phase: 'Evidence and proposal', schema: proposalSchema });
if (proposal.app_id !== input.app_id) throw new Error(`${WORKFLOW_ID}: proposal app_id does not match Host-bound app`);
if (Array.isArray(proposal.required_flow_changes) && proposal.required_flow_changes.length) return { status: 'needs_input', workflow_id: WORKFLOW_ID, app_id: input.app_id, required_flow_changes: proposal.required_flow_changes, proposal };

// Host re-reads the flow and derives all non-agent fields before approval.
const validated = await run(`Use LocalAppValidateMcpProposal for app ${input.app_id}, workflow run ${context.workflow_run_id}. Host must re-read trusted Flow contexts, reject cross-app/forward/dynamic/forbidden bindings, derive permission ceiling and execution metadata, calculate canonical proposal_sha256, approval_contract_sha256 and tool_surface_sha256, and persist stage prepared. Never accept model-supplied server, annotations, permissions, paths or catalog identity. Proposal: ${JSON.stringify(proposal)}`, { agentType: 'mcp-designer', label: 'host-proposal-validation', phase: 'Validate and approve', schema: { type: 'object', properties: { ok: { type: 'boolean' }, status: { type: 'string' }, proposal_sha256: { type: 'string' }, approval_contract_sha256: { type: 'string' }, tool_surface_sha256: { type: 'string' }, findings: { type: 'array' } }, required: ['ok', 'status', 'findings'], additionalProperties: false } });
if (validated.ok !== true) throw new Error(`${WORKFLOW_ID}: Host rejected proposal: ${JSON.stringify(validated.findings || validated)}`);
if (!Array.isArray(proposal.tools) || proposal.tools.length === 0) return { status: 'mcp_authoring_required', workflow_id: WORKFLOW_ID, app_id: input.app_id, candidate_preserved: true, proposal, validation: validated, reason: 'No meaningful Flow-bound tool cleared authoring; Host retains the prepared candidate without receipt or promotion.' };
const approved = validated.status === 'approved_reusable'
  ? { approved: true, status: 'approved_reusable', receipt_id: null }
  : await run(`Use LocalAppApproveMcpProposal for app ${input.app_id}, workflow run ${context.workflow_run_id}. Approve only the Host-generated review surface and mint one promote receipt for this prepared candidate. Source bytes/build ids are not review-surface changes; any tool/schema/Flow/ceiling change requires a new diff and receipt. Validation: ${JSON.stringify(validated)}`, { agentType: 'mcp-designer', label: 'native-approval', phase: 'Validate and approve', schema: { type: 'object', properties: { approved: { type: 'boolean' }, receipt_id: { type: ['string', 'null'] }, status: { type: 'string' } }, required: ['approved', 'status', 'receipt_id'], additionalProperties: false } });
if (approved.approved !== true) return { status: 'approval_required', workflow_id: WORKFLOW_ID, app_id: input.app_id, proposal, validation: validated };
if (operation === 'initial') return { status: 'create_approved', workflow_id: WORKFLOW_ID, app_id: input.app_id, operation, quality_level: quality, proposal, validation: validated, approval: approved };
const qa = await run(`Use LocalAppQaMcpCandidate for app ${input.app_id}, workflow run ${context.workflow_run_id}. Re-read the approved candidate, validate schema limits, typed Flow binding, build identity, bounded call contract, and app isolation. UI evidence being unavailable must be reported as unverified, not used to bypass MCP QA. Mark the durable journal smoke_passed then mcp_verified only after the Host gates pass. Return verification_sha256. Candidate: ${JSON.stringify(validated)}`, { agentType: 'tester', label: 'mcp-qa', phase: 'QA and promote', schema: qaSchema });
if (qa.ok !== true || qa.mcp_schema !== 'passed' || qa.flow_binding !== 'passed' || qa.calls !== 'passed' || qa.isolation !== 'passed') throw new Error(`${WORKFLOW_ID}: MCP QA failed closed: ${JSON.stringify(qa.findings)}`);
const promoted = await run(`Use LocalAppPromoteMcpCandidate to atomically promote the Host-owned non-empty catalog for app ${input.app_id}, workflow run ${context.workflow_run_id}. Consume the existing receipt when present, persist the candidate catalog under Host app data, swap the build/catalog pair only together, preserve the previous pair on failure, and mark the journal promoted. If the tool surface is unchanged, retain authoring_revision. Never promote an empty catalog. Receipt: ${JSON.stringify(approved.receipt_id)}. QA: ${JSON.stringify(qa)}; validation: ${JSON.stringify(validated)}`, { agentType: 'verifier', label: 'mcp-promote', phase: 'QA and promote', schema: { type: 'object', properties: { promoted: { type: 'boolean' }, catalog_sha256: { type: 'string' }, status: { type: 'string' }, publication_state: { type: 'string' } }, required: ['promoted', 'status', 'publication_state'], additionalProperties: false } });
if (promoted.promoted !== true) throw new Error(`${WORKFLOW_ID}: Host did not promote MCP candidate: ${promoted.status}`);
return { status: 'promoted', workflow_id: WORKFLOW_ID, app_id: input.app_id, operation, quality_level: quality, proposal, validation: validated, qa, promotion: promoted };
