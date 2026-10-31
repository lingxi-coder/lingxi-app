# Teams T3 dedicated-process prerequisite — 2026-10-29

**T3 stays incomplete and disabled.** Companion Harness commit
`43031991ba7246b2f1b0f38e05632009334d3aaa`, parent `dfab0eb3`, adds a dedicated
macOS worker for one host-admitted turn. Product source still pins
`eed08ca06c35371c1eb7ea5b02ebc10bcae63a12`; it does not launch this worker.
The companion is local/unpublished. No push, merge, deployment or IAM change.

## Implemented behavior

- A fresh executable process receives only a selected static Anthropic API key,
  model, private principal, bounded human text and immutable persona over an
  anonymous pipe. No key enters argv, environment or credential/settings files.
  The eight-key environment fixes private HOME/config/tmp/cwd, PATH, locale and
  CoreFoundation home/encoding; ambient credentials, headers, metadata, proxy
  variables and cloud-provider switches are excluded.
- The worker closes inherited non-stdio descriptors before building Tokio or
  Harness, including descriptors without CLOEXEC. The only input is a strict,
  versioned bootstrap bounded to 256 KiB. EOF or additional input bytes cancel
  work; no approval, question, settings or second-start command exists.
- A synced create-new invocation tombstone precedes spawning. Concurrent starts,
  spawn failure, cancellation, timeout and result loss cannot automatically
  reset quota or relaunch the same invocation. Backend permit authentication,
  stable identity mapping and cross-host fencing remain host prerequisites.
- The parent captures the backend lease into a monotonic deadline, capped at
  five minutes; child build/model/drain, pipe I/O and child waiting share it.
  Cancellation, future drop and expiry kill the worker group. The child also
  supervises its lease and parent connection. All tool requests are denied;
  AskUserQuestion has no sender. No model-selected command executes here.
- Terminal text uses the last accepted assistant message and typed completed-turn
  error facts. Retracted attempts, intermediate preambles, synthetic API errors,
  empty replies and malformed natural stops cannot become a successful Bot reply.
  Text/overflow is bounded to 8192 UTF-8 bytes, with a conservative cumulative
  received-text cap across retries. Provider error bodies are withheld.
- A real valid-text fixture exposed an existing shared recovery bug:
  `pumped_has_visible_text` accepted only plain Text. It now uses the core's
  visible-text rule, covering exact UTF-16 and native text with preserved unknown
  fields while still excluding thinking, whitespace and unknown protocols.

API and detailed trust/termination limits are recorded in the companion's
`docs/TEAM_INVOCATION_PROCESS.md`.

## Final verification

| Check | Result |
| --- | --- |
| Dedicated worker + pinned SDK over actual synthetic HTTP/SSE | 15 pass |
| Existing private Desktop profile/macOS Bash boundary | 13 pass |
| Shared visible-text guard | 7 pass, 2146 filtered |
| Existing turn recovery | 39 pass, 2114 filtered |
| Desktop-feature and Mobile-feature library checks on macOS | pass |
| Normal Desktop-feature standalone worker build | pass |
| Formatting of eight changed Rust files and diff checks | pass |
| Independent static review of all ten changed files | APPROVE for disabled prerequisite |

**74 distinct selected tests pass, zero failures/ignores.** These are not the
entire repository suite. Earlier 721 profile/shared and 1440 model-layer totals
are historical; they were not rerun in full. Review performed no tests.

The 15-case suite first passed through Cargo. It then ran again directly against
the worker built with the normal Desktop dependency features, without dev-test
feature unification. The final worker SHA-256 was identical before and after:
`7e959bdc930a9396ae1df696ef51bf4ed530bac6e02a4b81226001b46d162807`.
This is an unsigned local debug executable, not a signed product package.

The tests verify actual x-api-key/model/output-cap/request bytes and private
prompt content, denial of a valid streamed Bash request, shared dispatch-budget
exhaustion, accepted/retracted/failed text, and real provider-connection closure
on expiry/cancellation/drop/parent-EOF. A plain-spawn positive control retains an
inheritable socket; the real worker closes it. Protocol failures require their
specific exit category, so an earlier environment refusal cannot falsely pass
a malformed-frame test. Deadlines use short finite leases (including live
10-second leases) and reject expired or beyond-five-minute values; no real model
was left running for five minutes.

`results.json` seals ten source-file hashes, normal-worker/test-binary hashes,
commands and nine compressed logs with raw/compressed hashes. Build outputs and
raw logs remain in this companion's ignored target directory.

## Validation history and limits

The tool's outer sandbox initially refused localhost binding (EPERM); that
historical failure log is retained and is not counted as passing acceptance.
Final local fixtures used the approved execution context. The initial strict
six-key worker also refused macOS pre-main environment initialization; private
CoreFoundation home and fixed UTF-8 encoding now make startup deterministic.
[Apple's initialization source](https://github.com/apple-oss-distributions/CF/blob/main/CFRuntime.c)
explains the early encoding setup. The parent lease uses a dedicated reader
thread because [Tokio stdin reads cannot be cancelled](https://docs.rs/tokio/1.52.3/tokio/io/struct.Stdin.html).

This does not authenticate `startBot`, connect the backend to the worker, provide
owner-only approvals or original-human questions, persist final results, retry
`finishBot`, poll revocation, implement recovery UI or deliver the four-client
T3 feature. Other model protocols/OAuth/cloud/subscription credentials are not
admitted. Literal loopback HTTP is supported for local gateways; HTTPS/live
provider behavior was not exercised. Saved test accounts/sessions were untouched.

Group killing is not reliable detached-process containment. Current workers
deny every tool, so no detached model-selected tool child is admitted. Before
allowing tools, implement and validate detached termination, authenticated
brokers and independent deadline/revocation supervision. Reaping may take up to
two seconds after kill; scheduler/OS/filesystem stalls are not hard real-time
guarantees. CPU/memory/disk, total-input-token and billing ceilings remain open.

No real provider/Firebase/browser/device flow was used. Mobile-feature checking
on macOS is not native iOS/Android compilation, signing or physical acceptance.
Existing caches, other worktrees and source were preserved; no build-cache
cleanup occurred. Disk checks stayed above the 2 GiB floor.
