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

// args: { app_id: string, spec: string|object, expected_writable_collections: string[], strategy?: 'fast'|'balanced'|'thorough', complexity?: object, revision_prompt?: string, model?: string, runtime_profile?: { family: 'react_dom'|'canvas_2d'|'three_3d'|'phaser_2d'|'babylon_3d', revision: number, contract_sha256: string } }
const input = args && typeof args === 'object' ? args : {};
const appId = typeof input.app_id === 'string' ? input.app_id : '';
if (!appId) {
  throw new Error(`${meta.name} requires args.app_id (the app to implement)`);
}
// The spec is the ONLY description of what to build, and the skill's flow
// requires the user to have approved it. An absent/empty spec would leave the
// generator free to invent the app, which is exactly the confirmation step
// this segment must not bypass. Accept only a non-empty string or a non-array
// object with at least one own field; JSON.stringify would otherwise turn
// arrays, booleans, numbers, null and undefined into truthy prompt text.
const rawSpec = input.spec;
const hasObjectSpec =
  rawSpec !== null &&
  typeof rawSpec === 'object' &&
  !Array.isArray(rawSpec) &&
  Object.getOwnPropertyNames(rawSpec).length > 0;
const confirmedSpec =
  typeof rawSpec === 'string'
    ? rawSpec
    : hasObjectSpec
      ? JSON.stringify(rawSpec)
      : '';
const trimmedConfirmedSpec = confirmedSpec.trim();
let parsedSerializedSpec;
if (typeof rawSpec === 'string') {
  try {
    parsedSerializedSpec = JSON.parse(trimmedConfirmedSpec);
  } catch (_error) {
    parsedSerializedSpec = undefined;
  }
}
const parsedSerializedSpecIsValid =
  parsedSerializedSpec !== undefined &&
  ((typeof parsedSerializedSpec === 'string' &&
    parsedSerializedSpec.trim().length > 0) ||
    (parsedSerializedSpec !== null &&
      typeof parsedSerializedSpec === 'object' &&
      !Array.isArray(parsedSerializedSpec) &&
      Object.getOwnPropertyNames(parsedSerializedSpec).length > 0));
if (
  !confirmedSpec ||
  trimmedConfirmedSpec.length === 0 ||
  (typeof rawSpec === 'string' &&
    parsedSerializedSpec !== undefined &&
    !parsedSerializedSpecIsValid)
) {
  throw new Error(
    `${meta.name} requires args.spec (the spec the user confirmed) — refusing to invent one`,
  );
}

// Mobile's launcher overwrites this list from the host-materialized manifest;
// direct workflow harnesses still provide it explicitly. Treat the resulting
// task contract as valid only after checking its shape; silently dropping a
// malformed entry would turn a missing persistence check into a false pass.
const rawExpectedWritableCollections = input.expected_writable_collections;
if (
  !Array.isArray(rawExpectedWritableCollections) ||
  rawExpectedWritableCollections.some(
    (collection) =>
      typeof collection !== 'string' || collection.trim().length === 0,
  )
) {
  throw new Error(
    `${meta.name} requires args.expected_writable_collections to be an array of non-empty collection ids`,
  );
}
const expectedWritableCollections = [
  ...new Set(rawExpectedWritableCollections.map((collection) => collection.trim())),
];

// The runtime profile is a persisted host fact, not a per-run guess. Validate
// it before the first agent call and carry it through every stage; a design
// response, import graph, or hostile wording inside `spec` must never be
// allowed to select or silently downgrade the rendering/runtime core.
const rawRuntimeProfile =
  input.runtime_profile && typeof input.runtime_profile === 'object'
    ? input.runtime_profile
    : null;
if (!rawRuntimeProfile || Array.isArray(rawRuntimeProfile)) {
  throw new Error(
    `${meta.name} requires args.runtime_profile {family, revision, contract_sha256}`,
  );
}
const allowedRuntimeProfileFamilies = Array.isArray(SHAPE.allowedRuntimeProfileFamilies)
  ? SHAPE.allowedRuntimeProfileFamilies
  : [];
const runtimeProfileFamily =
  typeof rawRuntimeProfile.family === 'string'
    ? rawRuntimeProfile.family.trim()
    : '';
const runtimeProfileRevision = Number(rawRuntimeProfile.revision);
const runtimeProfileContractSha =
  typeof rawRuntimeProfile.contract_sha256 === 'string'
    ? rawRuntimeProfile.contract_sha256.trim()
    : '';
const isValidSha256 = /^[0-9a-f]{64}$/.test(runtimeProfileContractSha);
if (
  runtimeProfileFamily.length === 0 ||
  !Number.isInteger(runtimeProfileRevision) ||
  runtimeProfileRevision < 1 ||
  !isValidSha256
) {
  throw new Error(
    `${meta.name} requires args.runtime_profile {family, revision>=1, contract_sha256=64 lowercase hex}`,
  );
}
if (
  allowedRuntimeProfileFamilies.length > 0 &&
  !allowedRuntimeProfileFamilies.includes(runtimeProfileFamily)
) {
  throw new Error(
    `${meta.name} requires args.runtime_profile.family to be exactly one of ${allowedRuntimeProfileFamilies.join(', ')}`,
  );
}
const validatedRuntimeProfile = {
  family: runtimeProfileFamily,
  revision: runtimeProfileRevision,
  contract_sha256: runtimeProfileContractSha,
};


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
  `Persisted runtime profile: ${validatedRuntimeProfile.family} r${validatedRuntimeProfile.revision} (${validatedRuntimeProfile.contract_sha256}).`,
].filter(Boolean).join('\n');
log(strategyContext);
const revision =
  typeof input.revision_prompt === 'string' ? input.revision_prompt.trim() : '';
// The revision is part of what the user asked for, so EVERY downstream stage
// has to see it. Verify and Repair label this spec the source of truth; handing
// them the pre-revision spec under that label makes them file the revision
// itself as a defect — burning a repair round and then failing a build that did
// exactly what was asked. Design receives `revision` as its own argument and
// still gets the unfolded `confirmedSpec`, so folding here duplicates nothing.
const downstreamConfirmedSpec = revision
  ? `${confirmedSpec}\n\nRevision feedback:\n${revision}`
  : confirmedSpec;
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

const normalizeFinding = (finding) => {
  if (typeof finding === 'string') {
    const trimmed = finding.trim();
    return (
      trimmed ||
      'verification returned an empty string finding; treat it as blocking and include a concrete finding message'
    );
  }
  if (!finding || typeof finding !== 'object' || Array.isArray(finding)) {
    return 'verification returned a non-string finding; treat it as blocking and normalize frontend-qa findings to strings';
  }
  const severity =
    typeof finding.severity === 'string' && finding.severity.trim().length > 0
      ? finding.severity.trim()
      : 'unknown';
  const target =
    typeof finding.target === 'string' && finding.target.trim().length > 0
      ? finding.target.trim()
      : 'unknown target';
  const evidence =
    typeof finding.evidence === 'string' && finding.evidence.trim().length > 0
      ? finding.evidence.trim()
      : JSON.stringify(finding);
  return `verification returned a structured finding (${severity} at ${target}): ${evidence}`;
};

// `normalizeFinding` fails closed on every malformed ELEMENT, but the two call
// sites below read the container itself. Coercing a non-array container to `[]`
// discards whatever the verifier was trying to name — a verifier that answers
// `findings: "the settings button is clipped"` would otherwise pass silently.
// Absent/null stays empty: omitting the key is how a clean pass reports itself.
const verifierFindings = (verification) => {
  const findings = verification?.findings;
  if (Array.isArray(findings)) return findings;
  if (findings === undefined || findings === null) return [];
  return [
    `verification returned a non-array findings container (${typeof findings}); ` +
      'treat it as blocking and return frontend-qa findings as an array',
  ];
};

const uniqueFindings = (...groups) => {
  const seen = new Set();
  const findings = [];
  for (const group of groups) {
    if (!Array.isArray(group)) continue;
    for (const finding of group) {
      const normalized = normalizeFinding(finding);
      if (!normalized || seen.has(normalized)) continue;
      seen.add(normalized);
      findings.push(normalized);
    }
  }
  return findings;
};

// Data verification is a hard gate whenever the host-materialized manifest
// declares one or more collections. A verifier cannot opt out by returning
// not_applicable, omitting the result, or naming only a subset of the expected
// collections. Preserve the original failed-roundtrip wording so existing
// diagnostics remain stable.
const dataRoundtripFindings = (verification) => {
  const roundtrip = verification?.data_roundtrip;
  if (roundtrip?.status === 'failed') {
    return [
      `native data round-trip failed: ${roundtrip.evidence || 'no evidence'}`,
    ];
  }
  if (expectedWritableCollections.length === 0) return [];

  const expected = expectedWritableCollections.join(', ');
  if (!roundtrip || typeof roundtrip !== 'object') {
    return [
      `native data round-trip result is missing; expected writable collections: ${expected}`,
    ];
  }
  if (roundtrip.status !== 'passed') {
    return [
      `native data round-trip was not passed for expected writable collections: ${expected} (status: ${roundtrip.status || 'missing'})`,
    ];
  }

  const observed = Array.isArray(roundtrip.collections)
    ? roundtrip.collections.filter((collection) => typeof collection === 'string')
    : [];
  const missing = expectedWritableCollections.filter(
    (collection) => !observed.includes(collection),
  );
  return missing.length > 0
    ? [
        `native data round-trip passed without expected writable collection(s): ${missing.join(', ')}`,
      ]
    : [];
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
    SHAPE.designPrompt({
      appId,
      confirmedSpec,
      strategyContext,
      revision,
      runtimeProfile: validatedRuntimeProfile,
    }).join('\n'),
    {
      ...modelOptions,
      label: 'design',
      phase: 'Design',
      schema: DESIGN_RESULT_SCHEMA,
      throwOnError: true,
    },
  );
  design = requireAgentResult(designResult, 'the design step');
  if (
    SHAPE.requireDesignRuntimeFamily === true &&
    design.runtime_family !== validatedRuntimeProfile.family
  ) {
    throw new Error(
      `${meta.name} design runtime_family mismatch for app "${appId}": expected ${validatedRuntimeProfile.family}, got ${JSON.stringify(design.runtime_family)}`,
    );
  }
}

phase('Generate & Build');
const generated = await runAgent(
  SHAPE.generatePrompt({
    appId,
    confirmedSpec: downstreamConfirmedSpec,
    strategyContext,
    design,
    strategyPolicy,
    runtimeProfile: validatedRuntimeProfile,
  }).join('\n'),
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
    SHAPE.verifyPrompt({
      appId,
      build,
      strategyContext,
      verificationBreadth,
      design,
      runtimeProfile: validatedRuntimeProfile,
      confirmedSpec: downstreamConfirmedSpec,
      expectedWritableCollections,
    }).join('\n'),
    {
      ...modelOptions,
      label: `verify-${round}`,
      phase: 'Verify',
      schema: VERIFICATION_RESULT_SCHEMA,
      throwOnError: true,
    },
  );
  verification = requireAgentResult(verification, 'the verification step');
  // A failed build has no trustworthy preview for a native WebView check. Let
  // it consume the configured repair rounds so the original build error is
  // preserved; a successful build still fails closed immediately when the
  // native path was not proven.
  if (build.ok === true && verification.webview_checked !== true) {
    // This throw precedes the terminal block, so the terminal block's
    // "(verification was degraded)" annotation never reaches this case. Without
    // the verifier's own reason the operator sees only that the run died, with
    // a successful build and a live preview behind it. The gate itself stays
    // closed on purpose: an unproven native path is not something a repair
    // round can fix, and passing it would report an unverified app as OK.
    throw new Error(
      `${meta.name}: verification must set webview_checked=true for app "${appId}"; ` +
        'the native WebView path was not proven' +
        `${verification.degraded_verification === true ? ' (verification was degraded)' : ''}: ` +
        `${verification.summary || 'the verifier reported no reason'}`,
    );
  }
  const verificationFindings = verifierFindings(verification);
  const normalizedVerificationFindings = uniqueFindings(verificationFindings);
  // Every workflow-derived gate participates in both the repair decision and
  // the terminal throw. Keep this list separate from the verifier's own
  // findings so the repair prompt can add only derived findings that are not
  // already present in its serialized verification payload.
  const derivedFindings = uniqueFindings(
    dataRoundtripFindings(verification),
    renderCheckFindings(verification),
    typeof SHAPE.extraBlockingFindings === 'function'
      ? SHAPE.extraBlockingFindings(verification)
      : [],
  );
  const blockingFindings = uniqueFindings(
    normalizedVerificationFindings,
    derivedFindings,
  );
  if (
    build.ok === true &&
    verification?.ok === true &&
    verification.webview_checked === true &&
    blockingFindings.length === 0
  ) {
    break;
  }
  if (round === strategyPolicy.maxRepairRounds) break;
  repairRounds += 1;
  phase('Generate & Build');
  const serializedVerificationFindings = JSON.stringify(verificationFindings);
  const repaired = await runAgent(
    SHAPE.repairPrompt({
      appId,
      verification,
      // The raw verification payload already carries ordinary findings. Add
      // only actionable strings that are not represented there literally:
      // workflow-derived gates and synthesized diagnostics for malformed or
      // empty verifier findings.
      renderFindings: uniqueFindings(
        normalizedVerificationFindings.filter(
          (finding) => !serializedVerificationFindings.includes(finding),
        ),
        derivedFindings.filter(
          (finding) => !normalizedVerificationFindings.includes(finding),
        ),
      ),
      strategyContext,
      confirmedSpec: downstreamConfirmedSpec,
      design,
      runtimeProfile: validatedRuntimeProfile,
    }).join('\n'),
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
const verificationFindings = verifierFindings(verification);
// Same gate the loop's break condition used, so the last round is judged by
// exactly the rule that decided whether to spend a repair on it.
const finalFindings = uniqueFindings(
  verificationFindings,
  dataRoundtripFindings(verification),
  renderCheckFindings(verification),
  typeof SHAPE.extraBlockingFindings === 'function'
    ? SHAPE.extraBlockingFindings(verification)
    : [],
);
if (verification.ok !== true || finalFindings.length > 0) {
  const findings = finalFindings.length > 0
    ? finalFindings.join('; ')
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
