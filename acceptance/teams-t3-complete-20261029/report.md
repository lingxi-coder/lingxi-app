# Teams T3 implementation and acceptance

Status: local implementation and feature acceptance pass. Production publication,
live-provider/account acceptance and full-application release gates remain open;
this record does not mark T3 released or complete.

## Source

- Product branch: `claude/teams`, feature commit
  `cc818ea041aae1dc8739e91213de640dba8919c6`, base
  `52ac96c6bf9d4c1be43b414ea3a6c7a937599ee5`.
- Companion branch: `codex/teams-runtime`, local commit
  `2580fbcd1e6c5671bb25241cf5ccc465b30c3456`.
- Product Cargo manifest and lockfile use that immutable companion revision.
  The locked resolver found 79 canonical Harness packages with
  `development: false`; the lockfile change contains revision replacements only.
- No branch has been pushed and no Firebase deployment or IAM mutation has
  occurred. Existing real test accounts and their private saved sessions remain
  available under the prior authorization.

## Implemented behavior

All four clients expose permitted Bot cards, owner/privacy/presence disclosures,
Bot DM creation, explicit group/channel targets, run status, original-human
questions, durable exact answer retries and owner manual recovery. Plain `@name`
does not execute a Bot. Web and Mobile direct binding to Desktop.

The trusted macOS Desktop account/session binds an immutable local Bot snapshot
and discovers authenticated queues. Dedicated host/worker processes keep Firebase
credentials in the parent and pass only the selected static Anthropic key to the
worker. Owner tool approvals allow once or deny; human questions use a separate
original-UID/generation broker. Unsupported host/provider/auth routes refuse.

Private invocation journals and the durable discovery index prevent automatic
replay after ambiguous execution or lost finish ACK. Saved results retry delivery
without a model key, claim or tool execution. Busy observations preserve pending
result/recovery diagnostics and do not create notification retry loops.

Private Bash execution uses the macOS sandbox and a separate process guardian.
Worker death, EOF, cancellation, deadline and ordinary return clean the command
group before success. Detach mechanisms are denied. The five-minute authority
lease covers model, permission and question waits; it is never renewed by polling.
OS stalls are outside a hard real-time guarantee. These limits are not monetary,
CPU, total-input-token, memory or disk ceilings.

## Current verification

Counts below are distinct within each selected suite. Overlapping focused or
historical runs are not added together.

| Area | Current evidence |
| --- | --- |
| Rust runtime | 113 selected tests pass: POSIX process 56, connector 16, invocation process 15, profile 13, interaction 12, native boundary 1. The older ignored connector fixture is replaced by the actual product-account fixture here. |
| Desktop | 150 selected account, Bot, approval, storage, host and connector tests pass; main and renderer TypeScript checks pass. |
| Web/shared UI | 31 tests and both Web TypeScript checks pass; normal production Web build passes. |
| Backend/rules/SDK | 57 backend/rules cases and seven actual Desktop/Web Firebase SDK cases pass. |
| Packaging | 39 existing packaging tests pass. The official Flare signed app/ZIP passes static verification, and its actual signed Team companions pass the account/backend integration. Full application smoke is blocked by the existing user Broker. |
| Android | All actual Teams Kotlin sources compile with Firebase/Compose; 19 JVM tests pass. Actual emulator UI journey passes including real SDK server read, human answer, one final reply, Bot DM, plain mention and explicit group target. |
| iOS | All eight actual Teams Swift sources compile with Firebase 12.19.2. Thirteen unit tests and four final UI cases pass, zero failures/skips. The same active question has no answer controls for its Bot owner, then is answered by its original human and completes. Bot DM and explicit group-target journeys also pass. |
| Browser journey | Nine checks pass against real Auth/Firestore/HTTP and the actual host/worker: three answered live triggers each complete with exactly one final Bot reply. |
| Publication preparation | Five tests pass; all 19,265 live file hashes are preserved by the candidate overlay, which adds 11 hashed assets and Teams HTML. |

The integration fixture uses actual `AccountService`, Firebase SDK authentication,
Firestore rules, HTTP commands and dedicated Rust executables. Model SSE and all
identities are synthetic and loopback-only. It proves owner approval, rejection of
a non-origin answer, exact answer receipt retry, one Bash side effect, one final
reply, committed finish-ACK loss followed by result-only restart recovery, and an
offline request completed once after host restart.

The mobile executables are isolated feature harnesses referencing the actual
product source. They do not contain the whole product Rust/FFI/Linux stack and do
not establish full signed mobile package or physical-device acceptance. Simulator
Auth uses ad-hoc signing with a probe-only Keychain namespace; the normal product
signing configuration is unchanged.

The package was built from the dirty product base before the local feature
commit. Frozen implementation hashes tie that build to the feature commit;
the packaged main byte-for-byte matches the built main output. Documentation and
acceptance records are separate from runtime inputs. ZIP SHA-256:
`bc54c1dbbc9a336a2b674cb0c0a5eafeeb38427935b73de3fd9762581e9c783f`.
The full wrapper exits 1 at its existing-Broker safety guard; that result is not
reported as a successful full-application smoke.

## Acceptance fixture corrections

Failed and invalid preliminary runs are retained outside the sealed passing set.
Dynamic model plans now register the accepted immutable source message identity;
runtime wrapper changes cannot silently create a new plan or reset its turn.
Unknown inputs fail explicitly. Browser final-reply checks exposed and verified
this correction.

iOS traversal uses actual nested switch thumbs, exact long-label predicates and
scrolling above the keyboard. Question fields are scoped by the rendered immutable
message-row identifier because SwiftUI propagates that identifier to its children.
Every final XCTest invocation must execute its exact expected count with zero
failures/skips. Earlier empty-filter or vacuous form-identifier negative cases do
not count as acceptance. Expired fixture questions are not extended or replayed.

## Remaining delivery gates

1. Obtain the explicit branch/publication/IAM approval required by
   `HANDOFF-teams.md`, using the [scoped publication plan](publication-plan.md).
   Then perform production/test-account and real-provider acceptance.
2. Full Desktop application smoke needs separate permission to interrupt and
   restore the active user's application/Broker. It remains protected.
3. Keep full-product mobile/package/physical-device evidence separate from the
   passing native feature harnesses. T1/T2 publication gates are not implicitly
   closed by this T3 implementation.

The [results](results.json) link 97 source hashes, package/native binary hashes,
24 compressed logs with verified raw/compressed hashes, 14 screenshots and final
UI/runtime metadata. Preliminary failures remain in owned output and are excluded
from the sealed passing set. The package log is retained with its explicit
static-pass/full-smoke-blocked status. No test account/session material is copied.

Independent source and publication-plan reviews approve the scoped changes.
STANDARD cleanup and subsequent targeted regressions pass. Manual cache cleanup
was restricted to the user-approved companion `target/debug/incremental` path;
source, accounts, sessions and acceptance data were preserved. Two generated root
SwiftPM cache markers remain intact and unstaged. Main branches and unrelated
checkouts remain unchanged.
