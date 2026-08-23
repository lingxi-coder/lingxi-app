// Shared driver for every local-app build workflow.
//
// COMPILE-TIME APPEND, not an import: `builtins.rs` concatenates a shape file
// (which owns `meta`, the workspace contract and the result schemas) with this
// file. The shape comes first because the workflow runtime requires
// `export const meta = { … }` to be the FIRST statement in the script.
//
// Anything a routed app and a drawn surface genuinely share lives here — strategy
// policy, the build result schema, the render gate, the repair loop and the
// terminal throws — so the two workflows cannot drift apart on what "the build
// failed" means.

// args: { app_id: string, spec: string|object, strategy?: 'fast'|'balanced'|'thorough', complexity?: object, revision_prompt?: string, model?: string }
const input = args && typeof args === 'object' ? args : {};
const appId = typeof input.app_id === 'string' ? input.app_id : '';
if (!appId) {
  throw new Error(`${meta.name} requires args.app_id (the app to implement)`);
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
    `${meta.name} requires args.spec (the spec the user confirmed) — refusing to invent one`,
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
    `${meta.name} received unsupported strategy ${JSON.stringify(input.strategy)}; expected fast, balanced, or thorough`,
  );
}
// A shape may refuse a strategy outright. `fast` is the one strategy that skips
// Design, which is backwards for a drawn surface: such an app has almost no
// navigation and almost all of its difficulty in the mechanics and the frame
// loop — exactly what Design settles. Advice in the skill is not enough, because
// the complexity rubric scores a single-surface app at zero and lands on `fast`
// by arithmetic.
if (
  Array.isArray(SHAPE.allowedStrategies) &&
  !SHAPE.allowedStrategies.includes(requestedStrategy)
) {
  throw new Error(
    `${meta.name} does not accept the ${requestedStrategy} strategy; use one of ${SHAPE.allowedStrategies.join(', ')}`,
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
      `${meta.name}: ${stage} produced no result for app "${appId}"; ` +
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

let design = null;
if (strategyPolicy.runDesign) {
  phase('Design');
  const designResult = await runAgent(
    SHAPE.designPrompt({ appId, confirmedSpec, strategyContext, revision }).join('\n'),
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
  SHAPE.generatePrompt({ appId, confirmedSpec, strategyContext, design, strategyPolicy }).join('\n'),
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
    SHAPE.verifyPrompt({ appId, build, strategyContext, verificationBreadth, design }).join('\n'),
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
  const renderFindings = renderCheckFindings(verification).concat(
    typeof SHAPE.extraBlockingFindings === 'function'
      ? SHAPE.extraBlockingFindings(verification)
      : [],
  );
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
    SHAPE.repairPrompt({ appId, verification, renderFindings, strategyContext }).join('\n'),
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
    `${meta.name}: the build did not succeed for app "${appId}" after ${repairRounds} repair ` +
      `round(s): ${build.summary || 'no summary'}`,
  );
}
if (typeof build.preview_url !== 'string' || build.preview_url.length === 0) {
  throw new Error(
    `${meta.name}: the build reported success for app "${appId}" but returned no preview url; ` +
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
verificationFindings.push(
  ...renderCheckFindings(verification),
  ...(typeof SHAPE.extraBlockingFindings === 'function'
    ? SHAPE.extraBlockingFindings(verification)
    : []),
);
if (verification.ok !== true || verificationFindings.length > 0) {
  const findings = verificationFindings.length > 0
    ? verificationFindings.join('; ')
    : verification.summary || 'no findings reported';
  throw new Error(
    `${meta.name}: verification still has findings for app "${appId}" after the ${strategyPolicy.maxRepairRounds} allowed ` +
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
