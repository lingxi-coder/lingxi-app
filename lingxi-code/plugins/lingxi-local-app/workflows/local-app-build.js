// ============================================================================
// PHASE 2 SKELETON — NOT YET RUNNABLE.
//
// This is the target file for the design's ONE unified build workflow
// (LOCAL-APP-PLUGIN-DESIGN-V2.md §11.1, invoked as `lingxi-local-app:local-app-build`).
// Today the product still runs on the two SEPARATE builtin workflows compiled
// into the binary — `local-app-build` and `local-canvas-build`
// (lingxi-code/tools/workflow/src/builtins.rs:34-63, registered via
// `BuiltinWorkflowRegistry` at builtins.rs:65). §18 Phase 4 item 1 ("build
// workflow create/update/verify branches") is what replaces those two with
// this one file; §8.3 is explicit that unifying them means DELETING the
// Profile→workflow name map, not adding a third name beside the other two.
// That merge has not happened — this file's body does not attempt it.
//
// What this skeleton proves today:
//   - The plugin ships a `workflows/local-app-build.js` file for discovery to
//     find once §5.5's `.js`-only directory scan lands (design-only as of
//     this writing — see the grounding note below).
//   - `export const meta` parses as the engine's required first-statement
//     literal (lingxi-code/workflow/src/lib.rs:368-389, `validate_meta`).
//   - The script contains no non-deterministic API, so
//     `check_determinism` (lingxi-code/workflow/src/lib.rs:613-645) accepts
//     it and pause/resume digesting stays sound.
//   - The EXTERNAL input contract from §11.1 is enforced byte-for-byte:
//     only `operation`/`app_id`/`spec`/`revision_prompt`/`quality_level` are
//     accepted, and every field §11.1 names as forbidden external input is
//     rejected before anything else runs.
//
// What this skeleton deliberately does NOT do:
//   - It does not call `agent()`. There is no Host launch-context enricher
//     yet (§8.3's `host_context` injection), no `validate_template_selection`
//     Host tool, no candidate journal, and no `get_validated_selection(handle)`
//     read path (§18 Phase 3) — an agent() call here would either hang on
//     tools that do not exist or silently fabricate the orchestration §18
//     Phase 4 has not built. A workflow that LOOKS finished but cannot do
//     what its description promises is worse than one that says so plainly.
//   - It does not merge the DOM (`local_app_build_workflow.js`) and canvas
//     (`local_app_canvas_workflow.js`) design/verify schemas. Those two
//     diverge structurally today (compare
//     lingxi-code/tools/workflow/src/local_app_build_workflow.js:59-143 with
//     local_app_canvas_workflow.js:63-149) and deciding how they merge is
//     itself §18 Phase 4 scope, not something Phase 2 content authoring
//     should pre-empt.
//
// DISCOVERY GROUNDING (verify before trusting this file is ever loaded):
//   §5.5 says Plugin workflow discovery scans `<plugin-root>/workflows/*.js`
//   only (LOCAL-APP-PLUGIN-DESIGN-V2.md:404-416) and namespaces the result as
//   `<plugin-name>:<meta.name>` (§5.5, line 418), which is why `meta.name`
//   below is the BARE name `local-app-build`, not the `lingxi-local-app:`
//   prefixed form used in prose. As of this commit that discovery path is
//   DESIGN-ONLY: `plugin/src/manifest.rs`'s `PluginComponents` still has only
//   its original 7 component slots (commands/agents/skills/output_styles/
//   hooks/mcp_servers/lsp_servers — see the doc comment on that struct) with
//   no `workflows` field, and `plugin/src/discovery.rs` has no
//   `discover_workflows`/`.js`-glob function alongside its existing
//   `glob_skill_dirs`/`resolve_markdown_declared_paths`. Wiring that up is
//   §18 Phase 0a's "manifest/discovery 契约" that Phase 2 depends on but does
//   not itself implement.
// ============================================================================

export const meta = {
  name: 'local-app-build',
  description:
    'Create, update, or verify a confirmed Local App from Host-injected launch context (design §11.1). PHASE 2 SKELETON: only validates the external input contract; the design/generate/build/verify orchestration is not implemented until design §18 Phase 3/4.',
  phases: [{ title: 'Validate' }],
};

// §11.1's allowed external shape:
//   { operation: "create"|"update"|"verify", app_id: string, spec?: string,
//     revision_prompt?: string, quality_level?: "fast"|"balanced"|"thorough" }
const ALLOWED_KEYS = ['operation', 'app_id', 'spec', 'revision_prompt', 'quality_level'];

// §11.1's explicit forbidden-external-input list, verbatim:
//   runtime_profile, renderer, surface, template_id, template path/revision/digest,
//   workspace path, expected collections, host_context
// The design states these as prose nouns, not a JSON key list, so the exact
// snake_case spellings below are this file's best-effort transcription —
// flagged here rather than silently presented as an authoritative wire
// contract. Host-owned launch context (§8.3) is what actually keeps these
// out of caller reach once Phase 4 lands; this guard is a Phase-2-honest
// belt-and-suspenders check on top of that, not a substitute for it.
const FORBIDDEN_KEYS = [
  'runtime_profile',
  'renderer',
  'surface',
  'template_id',
  'template_path',
  'template_revision',
  'template_digest',
  'workspace_path',
  'expected_writable_collections',
  'expected_collections',
  'host_context',
];

const QUALITY_LEVELS = ['fast', 'balanced', 'thorough'];

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
    `${meta.name} rejects caller-supplied ${presentForbidden.join(', ')} — design §11.1 reserves these for the ` +
      'Host launch-context enricher (§8.3), which does not exist yet; there is no path for this workflow to accept them today.',
  );
}

const presentUnknown = Object.keys(input).filter((key) => !ALLOWED_KEYS.includes(key));
if (presentUnknown.length > 0) {
  throw new Error(
    `${meta.name} received unrecognized field(s) ${presentUnknown.join(', ')} — design §11.1's external input is ` +
      `limited to ${ALLOWED_KEYS.join(', ')}`,
  );
}

if (!['create', 'update', 'verify'].includes(input.operation)) {
  throw new Error(`${meta.name} requires args.operation to be exactly one of create, update, verify`);
}
if (!isNonEmptyString(input.app_id)) {
  throw new Error(`${meta.name} requires args.app_id (non-empty string)`);
}
if (input.spec !== undefined && typeof input.spec !== 'string') {
  throw new Error(`${meta.name}: args.spec, when present, must be a string`);
}
if (input.revision_prompt !== undefined && typeof input.revision_prompt !== 'string') {
  throw new Error(`${meta.name}: args.revision_prompt, when present, must be a string`);
}
if (input.quality_level !== undefined && !QUALITY_LEVELS.includes(input.quality_level)) {
  throw new Error(`${meta.name}: args.quality_level, when present, must be one of ${QUALITY_LEVELS.join(', ')}`);
}

log(
  `${meta.name}: input contract validated for operation="${input.operation}" app_id="${input.app_id}". ` +
    'This is a Phase 2 skeleton — the Host launch-context enricher, template-selector handle resolution, and ' +
    'designer/builder/verifier orchestration land in design §18 Phase 3/4 and are not implemented here.',
);

throw new Error(
  `${meta.name}: not yet implemented. Design §11.1 defines this workflow's contract; the orchestration body ` +
    'is design §18 Phase 4 scope. This Phase 2 commit intentionally stops after input validation rather than ' +
    'simulating create/update/verify behaviour that does not exist yet.',
);
