# Remote Control deep review — 2026-10-25

Review baseline: `ead2237d592fefdb8574f1c1bd86ad095bd6674e`. Integration retains subsequent main commits through `64085d939`, including Desktop acceptance and mobile workspace file browsing changes.

The review covers Desktop connection authorization, session attachments and leases, signaling publication, Web account/route ownership and device keys, Rust RPC/session/connection lifecycles, the Android/iOS FFI connection owner, and native image preparation. Independent reviewers reproduced the findings and cross-reviewed the fixes. Existing account, signing, routing, and APNs configuration are outside this patch.

## Confirmed findings and fixes

| Area | Trigger and consequence | Correction and evidence |
| --- | --- | --- |
| Host authorization | A queued notification runs after revocation/close, or a new command arrives while the final denial is being sent. It can recreate gateway state or continue application traffic. | Close application traffic synchronously; recheck ownership inside the deferred dispatch. Tests exercise actual simulated WebRTC connections through revoke and stop. |
| Pending attachments | Detach, sharing withdrawal/re-enable, or gateway disposal happens while a runtime opens. The obsolete attach later establishes a subscription and retains its lease. | Per-session pending-attach token, cancellation on detach/unshare/close, and final ownership check before installation. |
| Attachment capacity | Concurrent distinct attaches all pass the pre-await limit check. | Count pending reservations together with established attachments; nine concurrent requests cannot exceed the eight-session viewer cap. |
| Runtime lifetime | Disposed runtime observation closures remain held by the gateway. | Unsubscribe and remove the closure when the runtime is disposed; regression verifies no retained observation. |
| Shared-session list | Sharing is withdrawn while asynchronous title reads are pending. | Revalidate the enabled session set after title I/O, before exposing the list. |
| Host presence | A refused heartbeat or already-started fallback publication completes after stop; a failed startup delete releases ordering before its sibling finishes. | Generation and publication ordering prevent obsolete writes or deletes from reviving a stopped host or wiping a newer run. Startup cleanup waits for both deletions to settle before reporting failure. Queue ownership includes the database backend: fresh wrappers share ordering, while independent databases cannot block each other. Regressions cover stop, replacement during delayed writes/deletes, partial cleanup failure and backend isolation. |
| History pagination | An old history page resolves after reset/reconnect and contaminates a new transcript base, or a stale failure clears a newer loading state. | Web history generations fence both result and cleanup; Rust already guards the cursor/base and retains its regression. |
| Buffered events | More than 5,000 live events arrive during snapshot/attach. Dropped records are silently skipped once the cursor advances. | Web and Rust track the dropped-through sequence and request a fresh base if the completed snapshot does not cover it. Sparse sequence numbers remain valid. |
| Composer draft | A turn begins on another device after images were selected, then the browser sends a text follow-up. | Preserve the unsent image draft while sending text during a running turn. Tests cover asynchronous image preparation too. |
| Route ownership | React reuses account/session state across route changes; a new host route can briefly receive the prior host's live client before effects replace it. | Key account/session subtrees and synchronously fence the host hook by account, database, and host identity. |
| Question answers | A question is literally named `__proto__`. Assignment to an ordinary object loses the answer. | Construct own answer properties with `Object.fromEntries`; serialization regression verifies the exact key. |
| Repair lifecycle | Identity reset completes after the viewer is stopped; stale repair reconnects or repeated clicks rotate a new identity. | Single-flight repair, revoked-state gate, and generation-checked retry in Web/Rust; FFI inactive repair also cannot outlive disconnect. |
| Device-key reset | Another reader reloads a key during deletion, creation finishes after reset, or a stale second tab deletes another tab's replacement identity. | Wait for outstanding creation; block readers during reset; compare-and-delete the revoked public key atomically in IndexedDB. Shared-store two-instance tests cover sequential and simultaneous repair. |
| Rust fragmented RPC | Timeout or cancellation interrupts a split message under backpressure. The receiver keeps a partial frame and the next message corrupts the protocol. | Close the damaged transport when an incomplete split send is dropped. Mutex-queued and response-only timeouts do not close a healthy transport. |
| Native connection ownership | Concurrent connection construction and disconnect install a stale active connection; queued Rust dial work can start after stop. | Make generation/active installation atomic; fence queued attempt registration and final publication. Deterministic delayed-operation regressions cover both FFI consumers. |
| Early pairing approval | Approval arrives before the pending initialize response is consumed. The Rust awaiting-approval flag can stay set after Ready and suppress live notifications. | Clear the approval gate when the handshake completes; a scripted tunnel regression verifies a post-Ready notification. |
| Android screen cleanup | A prior overlay cleanup suspends, a new screen connects, then the old cleanup disconnects the new computer. | Serialize screen connection/watch/cleanup operations and fence cleanup by screen generation and client binding. The deferred-cleanup regression fails on the prior implementation. |
| Android photos | BitmapFactory pixels are re-encoded without applying camera EXIF orientation; URI/decode errors can escape the picker coroutine. | Use a bounded software ImageDecoder path and map I/O, permission, and invalid-image failures to the existing rejected-image UI. Samsung headless tests verify all eight rotation/reflection orientations, dimensions and quadrant colors; no app installation or account access. |

## Validation

Final results (zero failures):

| Validation | Result |
| --- | --- |
| Web complete test suite | 162 passed, zero skipped |
| Web types and production build | Passed |
| Desktop Remote Control suite, including real Electron dual-direction 2 MiB | 322 passed; 3 Firebase-emulator-only tests skipped |
| Desktop Node/renderer types | Passed |
| Rust client default suite | 114 passed, zero ignored |
| iOS FFI remote suite | 23 passed |
| Android FFI remote suite | 23 passed |
| Android Remote Control JVM suite, including Kotlin/Java production compilation | 42 passed |
| Samsung headless production image decoder | All 8 EXIF orientations, bounded dimensions, quadrant colors and non-enlargement passed |

Relevant regression entry points:

- Electron: `test/remote-connection.test.ts`, `remote-gateway.test.ts`, `remote-signaling-host.test.ts`, and the real `remote-rtc-electron.test.mjs`.
- Web: `test/remoteReviewRegressions.test.ts`, `remoteDevice.test.ts`, and `viewerConnection.test.ts`.
- Rust: `remote-control-client` connection/RPC/session tests and both FFI `remote::tests` suites.
- Android: Remote Control JVM tests and `RemoteImageDecoderInstrumentedTest` (also runnable headlessly on a device).

The Java headless device test executes the production image decoder and synthetic color fixtures. It does not replace a full installed-app camera/picker test. FFI public APIs and generated binding contracts are unchanged. The Android JVM run reused existing native artifacts and excluded native verification; new Android JNI libraries, iOS XCFrameworks and complete signed installers were not produced in this review. Earlier signed-package, phone and browser acceptance remains evidence for its recorded package; this review does not claim configured APNs delivery.

The final integration retained all review edits byte-for-byte while taking newer main commits. Desktop and Web checks cover their unchanged reviewed source; Android Remote Control tests were rerun after integrating the mobile workspace changes. The review and tests did not change VPN, proxy, routing, or application accounts.

## Final re-review

Independent re-review covered the final Host publication queue and failure cleanup, Web and Rust lifecycle fixes, and Android changes. A further read-only iOS pass covered Remote Control managers and screens, the root presentation boundary, camera draft handling, capture wrappers and FFI shutdown ownership. It found no additional provable defect; this iOS pass did not run a new native build or device test.

All 18 finding categories above are repaired. Final checks total 686 passed tests and 3 Firebase-emulator-only skips, plus the Samsung decoder checks. No remaining confirmed actionable finding was found in the reviewed Remote Control paths. This is the outcome of the reviewed scope and available validation, not a guarantee that every possible defect has been eliminated.
