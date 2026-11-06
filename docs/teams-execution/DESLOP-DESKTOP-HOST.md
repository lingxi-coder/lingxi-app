# Desktop host cleanup receipt

Scope: the current M0/M1 changes in `apps/electron/src/main/teams/{connector,privateStore,process,service}.ts` and `harness-runtime/crates/runtime/src/desktop/team_connector/{mod,host}.rs`. Other agents' work was excluded.

Behavior lock: 22 host regression tests passed before the cleanup edit. Two additional restart/ACK regressions and two busy-Chat source-watermark regressions subsequently passed, bringing the current host suite to 26 tests.

The explicit cleanup plan was to delete unused notification wiring, inspect duplication and authorization boundaries, then rerun focused checks. The only cleanup edit removed the unused `notifyHostChanged` member from the connector interface and service. The private synchronous constructor I/O and asynchronous operation I/O remain distinct; publication and running-task authorization remain separately scoped. No dependency, broad formatting, or speculative abstraction was introduced.

Checks:

- 26 host regression tests passed after the final lifecycle and source-watermark fixes.
- 23 personal Bot content tests passed after removing the retired `teamInstructions` collector; persona, memory and ordinary Skill behavior remain covered.
- Desktop Node and renderer typechecks passed.
- `git diff --check` passed for the changed host files.
- No package lint command is defined. No independent static-security scanner was run.
- Retired execution action/key/pending protocol searches found no live references in the owned Desktop host/preload/renderer bridge paths.

The complete actual private host/worker fixture passed with the final Desktop binaries: 200 initial source rows, a full 450-row increment, prior assistant retention, reopened cursors at revisions 1 and 2, and exact result recovery without additional model dispatch. The fixture made two synthetic-provider model calls and six authorization callbacks. The two complete debug-worker runs took 25,876 ms and 141,771 ms respectively, each within the actual 300,000 ms task deadline. These timings include local process/session/durability work and are not production or pure inference latency.

The binary hashes matched both the runtime agent's announced default build and the fixture's before/after measurements:

- Host: `d260a1888ece9e6cadf0ffebd54bae7ebf73a23a149af0f6558b4f0db16c819b`.
- Worker: `2979258ac744520ea991a81646513e3acd21b8a6acf86094888c8c6b2de8d481`.

Passing receipt: `/private/var/folders/2c/gch8wj4s0d52rygkyd6b9f_h0000gn/T/shared-bot-v3-integration-f9r9WX/receipt.json`. Full stdout: `/private/tmp/teams-host-runtime-final-2979258a.log`. The earlier passing `shared-bot-v3-integration-kIeIa8` receipt and `/private/tmp/teams-host-runtime-final.log` remain preserved for the previous worker hash. Earlier failures, including the `shared-bot-v3-integration-nz9t0X` fixture, also remain preserved. The fixture's previous artificial 90-second deadline was corrected to the specified five minutes before final verification.

This is synthetic-provider acceptance of the actual isolated processes. Signed-package, production-identity, real-provider business acceptance, and physical-device gates remain separate. Runtime pin and package cutover remain integration-owned.
