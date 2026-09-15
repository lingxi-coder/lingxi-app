# Streaming tool completion stall investigation

Final outcome: the shared tool-cost deadlock and live-control transport blocking
were repaired. The native credential admission/timeout fixes passed signed
positive and negative probes. The Flare packaging wrapper completed static and
full runtime smoke verification, and the formal Desktop app reconnected and
restored the original conversation with DeepSeek Flash and an enabled composer.

## Incident and evidence

The September 13 Desktop incident stopped after an Edit had changed a file,
while the interface continued to show a running tool. A subsequent user prompt
was queued rather than executed. The application remained responsive.

Evidence was collected before stopping or restarting the process:

- The affected engine started at 20:42:55 PDT. At 20:46:21 the Edit pre-hook
  completed and the file modification timestamp advanced. No post-hook start
  or persisted result followed.
- The durable ledger's revision 256 is the **previous Grep response**: its
  input/output/cache/reasoning counts exactly match that response's transcript
  usage. Revision 257 records the subsequent Edit's one added and one removed
  line. The Edit's model response has not reached the ledger.
- A two-second native process sample showed idle asynchronous workers rather
  than a thread blocked in filesystem sync or a synchronous mutex. TCP inspection
  showed the local Desktop connection and no outgoing model-service connection.
- Accessibility inspection showed Full access, DeepSeek Flash, the running Edit,
  and the Stop button. No permission dialog was visible.
- The running packaged JavaScript predates the recent preference-persistence
  changes; source-tree changes alone do not update an already running engine.

The private source file and transcript are not copied into this report. The
original process sample is `/tmp/lingxi-stuck-engine.sample.txt`; the application
diagnostic log and session ledger remain in their original local data directories.
Pre-restart copies are retained as `/tmp/lingxi-stall-desktop-before-restart.jsonl`,
`/tmp/lingxi-stall-ledger-before-restart.jsonl`, and
`/tmp/lingxi-stall-transcript-before-restart.jsonl`.

## Completion cycle

The streaming executor owns borrowed tool futures in `FuturesUnordered`.
They only advance when their caller polls them. An Edit can enter
`CostTracker::record_code_change` while model output is still streaming:

1. The tool obtains a FIFO durability turn and submits the code-change record.
2. The writer persists the record and sends its acknowledgement.
3. The stream ends before the tool future is polled again to consume that
   acknowledgement and release its durability turn.
4. Streaming finalization waits for the model-response cost receipt. That
   response is queued behind the tool's durability turn.
5. Finalization cannot reach its later tool-draining phase until settlement
   finishes; the tool cannot release the turn without being polled there.

This is an asynchronous scheduling deadlock. It explains a completed file
write, an idle engine, an unfinished tool, and absent subsequent response cost.
Permission or model changes are not required to create the cycle.

Relevant code: `lingxi-code/cost/src/tracker.rs`,
`lingxi-code/cost/src/persistence.rs`,
`lingxi-code/orchestrator/src/streaming_executor.rs`, and
`lingxi-code/orchestrator/src/conversation/drivers/mod.rs`.

The repair transfers durable code-change completion to an owned task, following
the existing ownership pattern for model-response settlement. Its caller still
waits for completion, but suspending or dropping that caller cannot retain an
already acknowledged durability turn. Acknowledgement identity checks, FIFO
ordering, and failure freezing remain required; adding a timeout and pretending
that persistence succeeded would violate those requirements.

## Independent live-control defect

The WebSocket frame pump awaited a whole command handler inside its socket-read
branch. A slow model-switch hook consequently prevented the same loop from
reading cancellation/permission replies or flushing outgoing events. A renderer
confirmation timeout did not cancel that server-side wait.

The repair keeps ordinary commands ordered while polling their work alongside
socket I/O. A narrow host-selected lane handles cancellation and interaction
replies. Cancellation may bypass ordinary commands only when it matches the
current active turn: a cancellation targeting a queued prompt must retain its
place after that prompt, or it would be ignored before the prompt starts.
Already started handlers still complete before connection teardown;
queued commands and bytes are bounded. This defect can worsen apparent hangs,
but the retained incident evidence does not establish that a model-switch hook
was active in this particular incident.

Relevant code: `lingxi-code/bridge/src/mcp_endpoint.rs` and
`lingxi-code/apps/bridge-server/src/server.rs`.

## Alternatives checked

- A stale snapshot at revision 228 with a newer write-ahead log is expected:
  append operations do not rewrite the snapshot on every mutation. There is no
  special revision-256 checkpoint on this path.
- Repeated durable acknowledgements work when callers continue being polled;
  the missing progress depends on caller scheduling, not edit count.
- No active remote inference connection or visible permission request was found.
- The actual file mutation precedes the stall, so blindly retrying the edit can
  report a missing old string or apply an additional side effect.

## Scope and verification

The cost repair is shared by Desktop, CLI, and the Android/iOS engine. Mobile
uses its in-process transport rather than the Desktop WebSocket pump. No new
dependency or platform-specific preference rule is needed for this repair.

Deterministic verification:

| Check | Before repair | After repair |
| --- | --- | --- |
| Park a tool caller after its edit request, send the exact ack, then request durable preflight | Times out | Passes; dropping the caller also preserves completion |
| Real streaming orchestrator: withhold Edit ack until MessageStop, then settle and request the next response | Model settlement times out | Edit result and next response complete; all 8 durable-handoff tests pass |
| Slow WebSocket command while sending output, cancellation, and approval | Times out with synchronous dispatch restored | Passes; 26 bridge unit tests and 4 endpoint integration tests pass |
| Cancel targets a queued prompt rather than the current active turn | Could overtake prompt admission | Active-owner priority classification regression passes |
| A callback finishes and signals shutdown with another command queued | Cleanup could first-poll the queued callback | Only actually started callbacks are drained; deterministic shutdown test passes |
| 258 real coordinator code-change writes with a deliberately unchanged snapshot | Not a failing path | Passes |

The complete cost-library suite passes: 197 tests. The complete bridge-server
unit suite passes: 119 tests, including active-turn cancellation classification.
Targeted diff whitespace
checks, `cargo clippy -p cost --lib --no-deps`, and
`cargo check -p bridge-server --tests` pass (existing repository warnings remain).
No timeout was added to hide failed writes, no acknowledgement
validation was removed, and ordinary controls were not made freely concurrent.

Test logs are `/tmp/lingxi-cost-parked-before.log`,
`/tmp/lingxi-cost-owned-edit-after.log`,
`/tmp/lingxi-durable-edit-regression.log`,
`/tmp/lingxi-durable-edit-green.log`, and
`/tmp/lingxi-bridge-slow-control-{red,green}.log`.

After collecting evidence, one normal Stop action left the old application's
visible state unchanged. A source fix cannot repair that already suspended
future in the existing process. Signed-package preflight passed; package/build
verification is tracked separately in `/tmp/lingxi-stall-package.log`.

### Initial packaged-runtime failure

The Flare wrapper built the release engine and renderer and signed the package.
Static package verification passed. A fresh independent
`codesign --verify --deep --strict --verbose=2` also reported `valid on disk`
and `satisfies its Designated Requirement`.

The application, native credential client, packaged Broker, and installed Broker
all have Team ID `AZ4AX7J833` and authority
`Apple Development: lingfeng luo (KQ7KX8LCYL)`. Packaged and installed Broker
CDHashes match (`ace5c38afbb8bc150c95ac22faab073e4df09d39`). Missing signatures
and a mismatched installed Broker are therefore not supported explanations.

The initial isolated packaged smoke test failed because credential-broker XPC
requests do not return within their native deadline. Sampling located waits in
macOS peer code-signature/timestamp trust validation; after restarting this
application's Broker, a second sample showed guest-code discovery inside
Security/CoreFoundation. Neither sample establishes an application mutex or
Keychain data-operation deadlock. No signature check was disabled, and no keys
were deleted or changed.

The formal app was launched from the newly signed package and loaded its saved
projects. It briefly connected a bridge, but credential errors prevented complete
session recovery. That attempt was not a passed packaged-runtime acceptance;
the successful final acceptance is recorded below.
Evidence: `/tmp/lingxi-stall-package-final-verify.log`,
`/tmp/lingxi-broker-stack.txt`, `/tmp/lingxi-broker-after-restart.sample.txt`, and
`/tmp/lingxi-smoke-credential-client.sample.txt`.

The smoke harness now reports its current stage, last bootstrap observation,
renderer errors, and application diagnostics on failure. Its bootstrap budget
allows the two bounded credential requests to finish rather than terminating
before their own deadlines; this did not bypass the failing sidecar assertion.

### Follow-up: native XPC admission

A signed health-only native integration probe also failed after 16.27 seconds,
without Electron or a project session. Callback dispatch and object lifetimes
were checked against the local Foundation SDK: XPC replies use a private queue,
so the client's main-thread semaphore does not itself block delivery.

Resource-policy experiments were inconclusive: removing Darwin background
policy produced a success, but a reverse comparison also succeeded with the
original background policy. A warmed old Broker accepted a direct health probe
in 140 ms. Cache and load effects therefore remain relevant; this investigation
does not establish that every observed trust delay has a single cause.

The Broker listener nevertheless performed two equivalent client checks: a
synchronous PID-based verification on its serial admission queue, followed by
the mandatory XPC signing requirement. The redundant PID check was removed.
The effective-user check and exact Team/client-identifier requirement remain,
installed before `resume()`. This follows Apple's documented
[XPC listener admission pattern](https://developer.apple.com/documentation/foundation/nsxpclistener/setconnectioncodesigningrequirement(_:));
the system checks the peer identity before delivering messages.

The new `CredentialBrokerXpcProbeMain.swift` sends only a fixed health request.
Four binaries compiled from the same source differ only in signing identity:

| Probe | Result |
| --- | --- |
| Correct development identity, before negative cases | Health success, 47 ms |
| Wrong client identifier, same Team | Immediate Cocoa 4097, no health response |
| Production identifier against development service | Immediate Cocoa 4097, no health response |
| Ad-hoc signature with the correct identifier | Immediate Cocoa 4097, no health response |
| Correct development identity, after negative cases | Health success, 54 ms |

4097 alone does not distinguish authorization refusal from service failure.
The identical request/source, signature-only differences, and successful
positive controls before and after establish that identity restrictions remain
effective. Timeouts do not count as a passing rejection. The native wrapper
health probe also passed against the candidate in 2.54 seconds. Logs:
`/tmp/lingxi-xpc-authorization-probes/paired-results.log` and
`/tmp/lingxi-xpc-health-probe/candidate.log`.

The 19 Node credential/smoke-harness tests pass. The final signed Desktop
rebuild and full smoke acceptance are tracked in `/tmp/lingxi-xpc-package.log`.

The native client also now closes and snapshots its reply state under a lock
before invalidating the connection. Reply/error callbacks accept only the first
result, and a timed-out waiter reports the deadline explicitly. This removes a
separate race between late callbacks and the calling thread's result reads.

Final acceptance passed through `npm run package:mac:flare`, including
`verify:package:static` and `verify:package:smoke`. The latter verified the
bundled renderer/engine, encrypted credential persistence across restart,
an authenticated localhost model request, deletion of the synthetic test
credential, live permission changes, security checks, and temporary-process
cleanup. The formal app then reconnected (08:00:34 UTC) and restored the affected
conversation; accessibility inspection confirmed DeepSeek Flash, an enabled
`Do anything` composer, and no active-turn Stop control. Signing checks and the
full smoke suite remain enabled.

The affected business task had temporarily disabled a generation guard before
stalling; restoring that task's intended business-code state is a separate
recovery concern and must not be inferred from the tool's running indicator.
