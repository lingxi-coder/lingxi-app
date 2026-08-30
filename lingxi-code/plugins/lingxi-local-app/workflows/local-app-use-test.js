// ============================================================================
// PHASE 2 SKELETON — NOT YET RUNNABLE.
//
// Target file for LOCAL-APP-PLUGIN-DESIGN-V2.md §11.3, the use-test workflow
// invoked as `lingxi-local-app:local-app-use-test`. The `local-app-test`
// skill already references it by this exact `{name, args}` shape —
// `{"name": "local-app-use-test", "args": {"app_id": "<id>", ...}}` — in
// lingxi-code/plugins/lingxi-local-app/skills/local-app-test/SKILL.md's
// "Running it" section, and that skill file already says the workflow is
// invoked "per the plugin's frozen design, §11.3" without claiming it
// exists yet. This file matches that precedent: it is the same honest
// not-yet-runnable posture, now as an actual (parseable, discoverable)
// script instead of prose.
//
// §11.3, verbatim on the split of responsibility this workflow owns:
//   "Workflow 先调用 operator 取得 runtime/interaction/render evidence，再调用
//    tester 对 acceptance checks 和 profile policy 生成结构化报告；两者都通过
//    workflow 内现有 agent() primitive 启动" — i.e. two `agent()` calls,
//   `agentType: 'operator'` then `agentType: 'tester'`, using the two agent
//   files that already exist at
//   lingxi-code/plugins/lingxi-local-app/agents/operator.md and
//   .../agents/tester.md. Calling them for real needs the Host-injected
//   build/runtime-profile identity §11.3 also requires ("Host 注入
//   build/profile identity") — that injection path does not exist yet (same
//   gap as local-app-build's §8.3 host_context enricher), so this skeleton
//   does not call agent() and does not guess at what operator/tester would
//   be told.
//
// DISCOVERY GROUNDING: see the identical note in ./local-app-build.js — the
// `.js`-only `workflows/*.js` scan this file depends on to ever be loaded
// (design §5.5) is not yet implemented in plugin/src/discovery.rs or
// plugin/src/manifest.rs's `PluginComponents` (still 7 slots, no
// `workflows` field) as of this commit.
// ============================================================================

export const meta = {
  name: 'local-app-use-test',
  description:
    'Drive a running Local App for evidence, then evaluate it against acceptance checks and profile policy (design §11.3). PHASE 2 SKELETON: only validates the external input contract; the operator/tester evidence-and-verdict pipeline is not implemented until design §18 Phase 4/6.',
  phases: [{ title: 'Validate' }],
};

// §11.3: "输入只包含 app_id、scope、scenario policy." The design gives these
// as prose nouns, not a JSON schema. `scope` and the scenario/quality policy
// shape are cross-referenced against local-app-test/SKILL.md, which is the
// only place in this codebase that spells out what a caller actually sends:
// a `app_id`, and (per "turn each acceptance check into a scenario ... a
// starting state, a short ordered sequence of user-observable actions, and
// what a pass looks like") a bounded `scenarios` array, plus the
// "scenario/quality policy" phrase which — by analogy with local-app-build's
// identically-named `quality_level` (fast|balanced|thorough) — is modelled
// here as the same enum. None of this is a Rust struct anywhere yet, so
// treat the shape below as inferred-from-prose, not a wire contract.
const QUALITY_LEVELS = ['fast', 'balanced', 'thorough'];
const ALLOWED_KEYS = ['app_id', 'scope', 'scenarios', 'quality_level'];

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

phase('Validate');

const input = args && typeof args === 'object' && !Array.isArray(args) ? args : {};

const presentUnknown = Object.keys(input).filter((key) => !ALLOWED_KEYS.includes(key));
if (presentUnknown.length > 0) {
  throw new Error(
    `${meta.name} received unrecognized field(s) ${presentUnknown.join(', ')} — design §11.3 says the input ` +
      'only carries app_id, scope, and a scenario/quality policy; profile/build identity is Host-injected, ' +
      'never caller-supplied.',
  );
}

if (!isNonEmptyString(input.app_id)) {
  throw new Error(`${meta.name} requires args.app_id (non-empty string)`);
}
if (!isNonEmptyString(input.scope)) {
  throw new Error(`${meta.name} requires args.scope (non-empty string) — design §11.3`);
}
if (
  !Array.isArray(input.scenarios) ||
  input.scenarios.length === 0 ||
  input.scenarios.some(
    (scenario) =>
      !scenario ||
      typeof scenario !== 'object' ||
      Array.isArray(scenario) ||
      !isNonEmptyString(scenario.name),
  )
) {
  throw new Error(
    `${meta.name} requires args.scenarios: a non-empty array of {name, ...} objects — the bounded scenarios ` +
      'the local-app-test skill shapes from the app\'s acceptance checks before calling this workflow.',
  );
}
if (input.quality_level !== undefined && !QUALITY_LEVELS.includes(input.quality_level)) {
  throw new Error(`${meta.name}: args.quality_level, when present, must be one of ${QUALITY_LEVELS.join(', ')}`);
}

log(
  `${meta.name}: input contract validated for app_id="${input.app_id}" with ${input.scenarios.length} scenario(s). ` +
    'This is a Phase 2 skeleton — the operator/tester evidence-and-verdict pipeline and the Host-injected ' +
    'build/profile identity land in design §18 Phase 4/6 and are not implemented here. Note also that the ' +
    'canvas-family "reject fast quality_level" rule (design §11.3, mirroring local-app-build) needs the ' +
    'Host-injected runtime profile family to enforce and is therefore deferred with everything else.',
);

throw new Error(
  `${meta.name}: not yet implemented. Design §11.3 defines this workflow's input/output contract and the ` +
    'operator-then-tester call sequence; the orchestration body is design §18 Phase 4/6 scope. This Phase 2 ' +
    'commit intentionally stops after input validation rather than fabricating a UseTestReport.',
);
