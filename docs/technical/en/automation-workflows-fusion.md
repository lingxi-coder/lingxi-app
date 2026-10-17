# Cron, Workflow and Fusion

Understand the live execution paths for schedules, scripted agent orchestration and evidence-based multi-model work.

<a id="execution-matrix"></a>

## Composition support

| Capability | Desktop | Mobile |
| --- | --- | --- |
| Cron | CronScheduler is built, loaded and started when host scheduler/trust policy and flags permit; child runtimes do not own another scheduler. | Persistent schedules and foreign-host-triggered firing. The Rust mobile composition does not run a continuous CronScheduler. |
| Workflow | TaskRegistryWorkflowLauncher and QuickJS-backed local_workflow tasks are wired. | MobileWorkflowLauncher is wired into mobile assembly; OS lifecycle still governs execution opportunities. |
| Fusion | Production services connect attempt admission, pricing, evidence, implementation and durable receipts. | Do not infer a full mobile executor from shared DTO/history projection. Verify the native host entrypoint before offering execution. |

<a id="cron-lifecycle"></a>

## Scheduled execution and OS constraints

Schedule persistence, due-occurrence selection and turn execution are separate phases. Cron run_due serializes storage read-modify-write with a process-wide lock and releases it before running the turn. Host scheduling/trust policy controls when autonomous turns may start, and persisted results retain the originating task identity.

Mobile firing returns session_id, result_text, terminal status and retryable. Its turn budget is 180 seconds beneath the OS worker lifecycle. Mobile recurring intervals must be at least 15 minutes; one-shot schedules are exempt. A schedule accepted by Rust does not guarantee an exact OS wake time or that iOS/Android grant background execution.

<a id="workflow-runtime"></a>

## Workflow scripts and resume

Workflow accepts inline script, named workflow or scriptPath. scriptPath takes precedence and is reopened through its approved rooted identity rather than by an untrusted pathname. Scripts are limited to 512 KiB and run through the QuickJS workflow runtime. A missing launcher reports a clear unwired failure.

A successful launch normally returns async_launched and task/run IDs, then completion arrives as a task notification. Parse/compile diagnostics can be returned without a registered task; check the returned error and task identity. ResumeFromRunId is same-session and reuses unchanged agent-call results; stop the old task before resuming an edited workflow.

### review-workflow.js · complete Workflow script

```javascript
export const meta = {
  name: 'boundary-review',
  description: 'Inspect provider and host ownership, then summarize evidence.',
  phases: [{ title: 'Inspect' }, { title: 'Summarize' }],
};

const question = typeof args === 'object' && args !== null
  ? String(args.question ?? 'Explain the provider and host boundaries')
  : String(args ?? 'Explain the provider and host boundaries');

phase('Inspect');
const evidence = await agent(
  'Read the project instructions and source. Answer with concrete file evidence: ' + question,
  { label: 'inspect', phase: 'Inspect' },
);
if (evidence === null) throw new Error('Inspection agent did not complete');

phase('Summarize');
return await agent(
  'Summarize this evidence and identify any unverified assumptions:\n' + evidence,
  { label: 'summarize', phase: 'Summarize' },
);
```

Verification method: Environment validation

### Workflow tool input · arguments retain their JSON type

```json
{
  "scriptPath": "./review-workflow.js",
  "args": {
    "question": "Which modules own the provider and host boundaries?"
  }
}
```

Verification method: API excerpt

This is a Workflow script, whose body runs in the host async wrapper; its top-level return is intentional and is not a standalone browser/Node module. Agent calls use the actual host tools and model services, so execution still requires credentials, grants and a running launcher. The JSON payload does not bypass authorization.

<a id="fusion-lifecycle"></a>

## Fusion evidence and durable results

Desktop Fusion composes model resolution, panel/attempt admission, pricing and budget ownership, evidence collection and optional implementation. Evidence from multiple models still needs synthesis and a recorded result; concurrent completion is not agreement. Attempts retain model/profile/account attribution for cost and recovery.

FusionRecorder and desktop lifecycle retain durable publication receipts. ShutdownReport.publications exposes receipts observed while draining. Handle incomplete publication through the owning coordinator and retry barrier; do not report success solely because an analyst or implementer model finished streaming.

<a id="automation-checks"></a>

## Acceptance scenarios

- Exercise schedule storage, a duplicate due occurrence, denied autonomous admission, a timed-out mobile turn and retryable versus terminal failure. Test workflow malformed input, missing launcher, compile error, cancellation and same-session resume.
- Fusion checks cover pool admission, pricing and durable handoff. Real multi-model runs, native background delivery and post-crash publication require separate target acceptance records.

### Workflow and Fusion checks

```bash
cargo test --locked -p tool-workflow --lib
cargo test --locked -p tasks --lib
cargo test --locked -p fusion --lib
cargo test --locked -p harness-runtime --features desktop --lib fusion_attempt
```

Verification method: Environment validation

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/desktop/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/assembly.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/desktop/cron_management.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/cron_management.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/mobile/host/automation.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/mobile/host/automation.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/mobile/host/assembly.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/cron/src/run_due.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/cron/src/run_due.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/tools/workflow/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/tools/workflow/src/lib.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/tools/workflow/src/workflow_input_schema.json](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/tools/workflow/src/workflow_input_schema.json) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/workflow/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/workflow/src/lib.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/workflow/src/description.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/workflow/src/description.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/tasks/src/handlers/local_workflow.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/tasks/src/handlers/local_workflow.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/desktop/fusion_services.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/fusion_services.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/desktop/fusion_attempts.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/fusion_attempts.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/desktop/fusion_recorder.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/fusion_recorder.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/runtime/src/desktop/fusion_implement.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/runtime/src/desktop/fusion_implement.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)
- [harness-runtime/crates/fusion/src/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/08813211974c0db4a57b1860a99604c5a6d6b6dc/crates/fusion/src/orchestrator.rs) — Committed source (08813211974c0db4a57b1860a99604c5a6d6b6dc)

- harness-runtime: HEAD 08813211974c0db4a57b1860a99604c5a6d6b6dc; Referenced input fingerprint sha256:17262ac652ec09f3f2cc282728a85bd756689c38ec992eaa4e8351a3a510ea19; Product pin 08813211974c0db4a57b1860a99604c5a6d6b6dc

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
