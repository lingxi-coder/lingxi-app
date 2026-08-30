// ============================================================================
// PHASE 2 SKELETON — NOT YET RUNNABLE.
//
// Target file for LOCAL-APP-PLUGIN-DESIGN-V2.md §12.2, the per-App MCP
// authoring workflow invoked as `lingxi-local-app:local-app-mcp-authoring`.
// The `mcp-designer` agent this workflow is supposed to call already exists
// at lingxi-code/plugins/lingxi-local-app/agents/mcp-designer.md, and its own
// frontmatter description already says so plainly: "§12 of the frozen
// design; not runnable yet." This file mirrors that same posture at the
// workflow layer instead of contradicting it.
//
// §12.2's flow (Host-owned AppEvidence → mcp-designer → needs_input?/
// required_flow_changes? loop → Host schema/binding/capability validation →
// canonical approval-contract digest → Native approval → candidate catalog +
// private logical server → MCP QA → atomic active catalog promote →
// conditional listChanged) depends on machinery that is §18 Phase 6 scope
// and does not exist yet: Host-owned AppEvidence assembly, the
// approval-contract digest (§12.2's exact coverage list — design contract,
// template/Profile, dependency input, full tool surface, Flow semantic
// references, required Flow changes, excluded capabilities, Host-derived
// permission ceiling), the MCP proposal receipt/diff flow (§17.4a), and the
// candidate-catalog promote path (§16.4). Calling agent() here without any
// of that would either hang or invent a proposal-and-approval sequence this
// plugin cannot actually honor yet — exactly what this task was told not to
// do.
//
// DISCOVERY GROUNDING: see the identical note in ./local-app-build.js — the
// `.js`-only `workflows/*.js` scan this file depends on to ever be loaded
// (design §5.5) is not yet implemented in plugin/src/discovery.rs or
// plugin/src/manifest.rs's `PluginComponents` (still 7 slots, no
// `workflows` field) as of this commit.
// ============================================================================

export const meta = {
  name: 'local-app-mcp-authoring',
  description:
    'Turn one App\'s own evidence and a user goal into a single-App MCP tool proposal, then take it through Host validation, approval, and atomic catalog promote (design §12.2). PHASE 2 SKELETON: only validates the external input contract; the mcp-designer → validation → approval → promote pipeline is not implemented until design §18 Phase 6.',
  phases: [{ title: 'Validate' }],
};

// §12.2's allowed external shape: { app_id: string, user_goal: string }
const ALLOWED_KEYS = ['app_id', 'user_goal'];

// §12.2's explicit "不接受" list, verbatim: raw tool definitions, server
// name, annotations, permission rules, Flow ID, workspace path,
// catalog/proposal digest. As with local-app-build.js, the design states
// these as prose nouns rather than a JSON key list; the spellings below are
// this file's best-effort transcription of that list, not an authoritative
// wire contract.
const FORBIDDEN_KEYS = [
  'tools',
  'tool_definitions',
  'server_name',
  'annotations',
  'permission_rules',
  'flow_id',
  'workspace_path',
  'catalog_digest',
  'proposal_digest',
];

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

phase('Validate');

const input = args && typeof args === 'object' && !Array.isArray(args) ? args : {};

const presentForbidden = FORBIDDEN_KEYS.filter((key) =>
  Object.prototype.hasOwnProperty.call(input, key),
);
if (presentForbidden.length > 0) {
  throw new Error(
    `${meta.name} rejects caller-supplied ${presentForbidden.join(', ')} — design §12.2 reserves these for ` +
      'Host-derived state (server identity, annotations, permission ceiling, final Flow binding, catalog ' +
      'digest); the agent proposes tool shape and Flow references only (§12.4).',
  );
}

const presentUnknown = Object.keys(input).filter((key) => !ALLOWED_KEYS.includes(key));
if (presentUnknown.length > 0) {
  throw new Error(
    `${meta.name} received unrecognized field(s) ${presentUnknown.join(', ')} — design §12.2's external input ` +
      `is limited to ${ALLOWED_KEYS.join(', ')}`,
  );
}

if (!isNonEmptyString(input.app_id)) {
  throw new Error(`${meta.name} requires args.app_id (non-empty string)`);
}
if (!isNonEmptyString(input.user_goal)) {
  throw new Error(`${meta.name} requires args.user_goal (non-empty string) — design §12.2`);
}

log(
  `${meta.name}: input contract validated for app_id="${input.app_id}". This is a Phase 2 skeleton — Host-owned ` +
    'AppEvidence assembly, the mcp-designer authoring loop, Host schema/binding/capability validation, the ' +
    'approval-contract digest, and the candidate-catalog promote path all land in design §18 Phase 6 and are ' +
    'not implemented here.',
);

throw new Error(
  `${meta.name}: not yet implemented. Design §12.2 defines this workflow's contract; the mcp-designer → ` +
    'validation → approval → promote pipeline is design §18 Phase 6 scope. This Phase 2 commit intentionally ' +
    'stops after input validation rather than fabricating an AppMcpProposal or approval receipt.',
);
