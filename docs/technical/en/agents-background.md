# Subagents and background tasks

Separate agent-pool admission, task ownership, mailbox delivery and terminal publication across foreground and detached work.

<a id="agent-boundary"></a>

## Agent context and admission

An agent carries its definition, resolved model/profile, allowed tools, working context, cancellation and parent ownership. Pool admission enforces capacity rather than assuming unlimited parallelism. Child permission mode is derived from trusted parent/definition facts and policy; a model argument cannot manufacture broader authority.

The main orchestrator and subagent runner share provider and tool contracts but have distinct histories and output projection. Keep agent_id, task_id, session_id and provider request IDs separate. Subagent transcript rows retain message identity, including partial stream and terminal error evidence.

<a id="background-composition"></a>

## Background execution needs a host decorator

Desktop background_agent is the actual async-agent decorator. It converts Agent run_in_background into a persistent LocalAgent task, installs the mailbox and pumps SendMessage toward task resume. The bare pool spawner reports an unwired-path error for unsupported detached admission; merely constructing a pool does not provide durable background execution.

TaskRegistry owns status, output and cancellation for long-lived work. The host routes lifecycle/progress notifications and publishes a terminal result once. A detached caller or closed foreground panel must not silently lose the task owner or its durable receipt.

<a id="mailbox-and-observer"></a>

## Messages, observers and parked agents

Mailbox delivery and model execution are different phases. A message accepted by the host is not proof that the recipient model has consumed it. Runtime agent listings and transcript loading project shared state; observer guards prevent output from a stale owner or replaced session from appearing in the current conversation.

Parked agents preserve resumable state rather than being treated as running forever. Restoration must recover their session ownership, tool grants and mailbox linkage before admission. An agent record without a live or restored owner is not an executable background task.

<a id="failure-and-cancel"></a>

## Failure, capacity and cancellation

Report admission starvation separately from provider failure and tool failure. Cancellation propagates through the task/agent owner; collect final transcript and output cleanup before releasing leases. A partially emitted response can be retained on provider failure, while a terminal task status still records the failure.

Coordinator/teammate behavior is enabled by the selected composition, shared registry and transport. Desktop in-process and pane teammates have host-specific lifecycle. Do not infer cross-device collaboration or mobile parity from the presence of the collaboration Cargo feature alone.

<a id="agent-checks"></a>

## Exercise admission and owner cleanup

- Test pool capacity, a queued cancellation, foreground detachment, mailbox resume, retained partial output and a terminal publication retry. Existing unit/fixture checks do not establish multi-provider live behavior.
- Validate pane processes, workspace cleanup and external task side effects on the actual desktop OS. Cold restart acceptance must show restored ownership, not just a persisted task list.

### Agent and desktop composition checks

```bash
cargo test --locked -p agent --lib
cargo test --locked -p harness-runtime --features desktop --test coordinator_activation
cargo test --locked -p harness-runtime --features desktop --test pool_starvation
cargo test --locked -p harness-runtime --features desktop --test background_output_writer
```

Verification method: Environment validation

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/agent/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/agent/src/context.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/agent/src/handle.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/agent/src/handle.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/agent/src/runner.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/agent/src/runner.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/agent/src/pool.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/agent/src/pool.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/agent/src/pool/capacity.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/agent/src/pool/capacity.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/runtime/src/desktop/background_agent.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/runtime/src/desktop/background_agent.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/runtime/src/desktop/session_agents.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/runtime/src/desktop/session_agents.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/tasks/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/tasks/src/lib.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/tasks/src/handlers/local_agent.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/tasks/src/handlers/local_agent.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/coordinator/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/coordinator/src/lib.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/runtime/src/parked_agent_restore.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/runtime/src/parked_agent_restore.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)

- harness-runtime: HEAD d2e3aed63ef232526967a8bf176a06d6a25ae8fa; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin d2e3aed63ef232526967a8bf176a06d6a25ae8fa

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
