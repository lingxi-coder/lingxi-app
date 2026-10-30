# Teams T3 runtime output-quota checkpoint — 2026-10-29

**The quota prerequisite is validated locally; T3 remains incomplete.**
The product baseline is `c8cf764a0` on `claude/teams`. A new isolated companion
Harness worktree, `../harness-runtime`, branch `codex/teams-runtime`, contains
commit `6ac2f6f4e617a21a71b57e479bf2b7e195adcaea` based on the product's existing
`eed08ca06c35371c1eb7ea5b02ebc10bcae63a12` pin. No push, merge, deployment or IAM
change was made. The product dependency pin and all four client UIs are unchanged.

## Implemented capability

One trusted, in-memory `ScopeOutputQuota` has a positive per-request output cap
and finite physical generation count. `DesktopConfig` passes the same Arc into
the main `ApiService` and `credentials::api_service_from_stack`. Independent/App
service forks preserve that instance; Session-backed compaction, recap,
automatic/session memory and vision helper queries share it.

Every retry clamps explicit/default output tokens before preparing. After SDK
sealing, both the resolved budget and final wire bytes are checked. Missing or
excessive caps and generation multipliers such as `n=2` are refused. Subscription
routes that cannot enforce the cap are refused without dropping the cap and
retrying. No sealed or signed request is rewritten.

At each actual HTTP/WebSocket generation marker, both the existing operation
budget and the shared scope budget are reserved. Admission checks do not charge.
Pre-dispatch failure rolls reservations back; dispatched requests are never
refunded for ambiguous errors or missing usage. Exhaustion is terminal. A
permanently revoked quota is rechecked after awaited host admission. Limited
services skip optional WebSocket prewarm and reject independent DeepSeek web
requests. An ordinary service with no quota keeps its existing behavior.

For supported providers honoring their wire cap, requested generation output is
bounded by physical dispatch count times per-request cap. This is **not** an
input-token, total-token, tool-operation or monetary budget. Quota revocation
blocks future generation dispatches; it does not prove that a previously admitted
request or tool process has terminated.

## Verification

All checks ran from the new companion worktree with the exact fixed SDK
`ff71b2ec1139bc67cc8719d78b1735aca967bbe8`, offline dependency resolution and mock
provider transports. No real model, Firebase account or saved browser session
was used.

| Check | Result |
| --- | --- |
| Full `llm-runtime` library regression, serial test execution | **1,440 pass**, zero failed/ignored/skipped |
| New quota cases included in that full regression | **14 pass**: concurrent/rollback semantics, explicit cap, retry, fork/helper sharing, separate operation budget, late revocation, streaming, WebSocket/prewarm, codec fields, multipliers, subscription rejection and awaited host admission |
| Focused `scope_` filter | **30 pass**, zero failed/ignored; includes the 14 new cases and 16 existing scope cases |
| Default-feature `harness-runtime` library check | Pass, including final post-format check; this command did not enable the Desktop assembly feature |
| Changed Rust source formatting and `git diff --check` | Pass |
| Independent final source review | APPROVE; eight Rust source files, no blocking findings |

Commands:

```sh
cargo test --offline --locked -p llm-runtime --lib --no-default-features -- --test-threads=1
cargo test --offline --locked -p llm-runtime --lib scope_ --no-default-features
cargo check --offline --locked -p harness-runtime --lib
```

Scope correction recorded during the subsequent profile checkpoint: the command
above checked the default engine library, not Desktop assembly. Desktop-feature
compilation and real assembly/shell fixtures are recorded separately in
`../teams-t3-profile-20261029/report.md`.

The subscription regression first seals a valid uncapped synthetic subscription
request, then confirms that adding the hard cap is rejected for that cap. It
does not pass merely because of invalid credentials, billing, endpoint or request
envelope. Real provider adherence to the encoded cap is still unverified.

The independent review initially identified the multiple-completion bypass;
the final implementation and regression reject it. Review was read-only and
did not run tests. Full regression evidence, exact source/log hashes and scope
limits are in `results.json` and the compressed logs beside this report.

## Still required before enabling T3

1. A distinct trusted Team launch profile and process, using the exact original
   human UID/generation in its isolated home/workspace/transcript/memory identity.
   Copy immutable persona/skills. Do not inherit owner folders, router/MCP tokens,
   plugin secrets or computer/browser capabilities. Only allow tools with an
   unbypassable sandbox/scope boundary.
2. Attach this quota once per authenticated invocation, never per helper or
   recovered process. Bound input, turns and the total five-minute deadline.
   Apply the same capability to every generation entry point; Independent
   side-query or direct provider-service paths cannot substitute for it.
3. Reliable process termination on deadline/revocation, owner-only tool approvals
   and a distinct original-human question route. Generic owner IPC/RC must not
   answer Team-human questions. A cancellation acknowledgement is not proof of
   process termination.
4. Queue/claim/one-shot-start integration with durable local state, result retry
   and ambiguous recovery. A restart must not reset the quota and blindly replay
   a started invocation. Validate real host/model crashes, overlapping listeners,
   offline/rejoin and revocation with the backend protocol.
5. Bind/list/target/question/recovery UI on Desktop, Web, iOS and Android together,
   followed by complete native/product/device acceptance. T3 stays unchecked.

The companion commit is not available on the remote and the product still uses
its previous Harness pin. No product entry point creates a limited runtime yet.
Any pin integration must preserve the unpublished companion commit and remain a
local change unless the user separately authorizes push. Firebase release gates
and the existing dedicated test-account/session authorization are unchanged.

## Workspace and disk

Canonical Harness and other worktrees were not edited. Existing build artifacts
were read to clone a starting Cargo cache into this worktree with APFS copy-on-write;
subsequent compilation wrote only this worktree's target. No cache was deleted
in this continuation. The new source checkout and target are retained.

Available space was rechecked throughout and remained above the 2 GiB stop line.
Space changed substantially due to external activity; `results.json` records the
last observation. All our checks finished and no new emulator/browser/server was
started. Dedicated real test credentials and sessions were neither opened nor
modified; preserve their Git-ignored directory on future worktree cleanup.
