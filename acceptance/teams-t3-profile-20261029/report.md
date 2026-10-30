# Teams T3 private-profile checkpoint — 2026-10-29

**The macOS private-profile prerequisite is implemented locally. T3 remains
incomplete and disabled.** The companion Harness branch is `codex/teams-runtime`,
based on quota commit `6ac2f6f4e617a21a71b57e479bf2b7e195adcaea`. The new local
commit is `dfab0eb36bb20196534509298d88654fce008554`; exact check results are recorded in
`results.json`. The product's
Harness pin remains `eed08ca06c35371c1eb7ea5b02ebc10bcae63a12`; no product entry
point creates a Team runtime. No push, merge, deployment or IAM change occurred.

## Implemented boundary

`TeamInvocationProfile` is an in-memory host capability. It hashes the complete
owner/Bot/Team/conversation/original-human/membership-generation principal into a
private namespace. It validates canonical 0700 directories, bounded immutable
0400 persona/Skill snapshots and per-invocation transcripts/output roots. Clones
share a revocable tool lease, finite output quota and profile-owned policy cache.
Config reapplication discards owner callbacks, folders, settings, resume data,
MCP, plugins, hooks, skills, native tools, startup helpers and watchers.

Only the selected provider configuration is retained. Team bootstrap uses memory
credentials and refuses ordinary persistent/shared host sessions. Ambient
OAuth/PAT capture and generic environment-key fallback are disabled, including
the public headless service path. An explicitly supplied shared stack remains a
trusted host responsibility and must contain only the selected credential.

Automatic-memory retrieval and session-memory extraction use explicit private
config-home paths, independent of HOME/LINGXI_CONFIG_DIR. The main prompt uses
the immutable snapshot and does not discover owner/ancestor instructions.
No real model memory retrieval or extraction was exercised.

The actual Bash tool uses a fixed executable, no login/startup files, a cleared
environment and private HOME/TMPDIR/PATH. A dedicated macOS sandbox wrapper
excludes general desktop owner-home/global-temp write grants. Only workspace/tmp
are mutable. The live sandbox toggle, explicit override and forged Git pointers
cannot widen the grant. Network/system credential IPC, background handoff, host
Git snapshots and unconfined cwd readback are withheld; `cd` does not persist.

All Bash commands reach the host permission adapter, including read-only `pwd`.
Managed allow rules and sandbox auto-allow cannot grant execution for the owner.
An unanswered adapter request is denied. The test subsequently calls the actual
registered Bash tool directly to simulate a separate owner approval; no Team
member supplies permission in that fixture. No question sender is inherited;
the test uses an explicit synthetic sender when checking questionnaire registration.
The authenticated owner/human brokers themselves are still unimplemented.

Host metadata probes also need confinement: a model-controlled `.git` can point
outside the scope and execute `core.fsmonitor` during an apparently read-only
Git status. Team boot skips subagent Git probing; the execution-only orchestrator
gate disables prompt/environment Git, transcript branch and server-classifier
Git probes. Private shell/OS prompt facts do not spawn host commands.

Private foreground completion/cancellation kills same-group descendants. This
is **not** reliable termination of descendants that detach with setsid, nor a
completed five-minute invocation supervisor.

API contract: companion `docs/TEAM_INVOCATION_PROFILE.md`.

## Verification

All provider data and repositories were synthetic. No Firebase account, saved
browser session, real provider credential or live model was used. The fixed SDK
is `ff71b2ec1139bc67cc8719d78b1735aca967bbe8`; Cargo ran offline and locked.

| Check | Result |
| --- | --- |
| Real Desktop Team profile integration | 13 pass |
| POSIX runner modules | 31 pass |
| Full provider-config library | 97 pass |
| Full tool-api library | 303 pass |
| Full tool-shell library | 258 pass |
| Orchestrator Git-focused library cases | 11 pass |
| Orchestrator environment-ingress library cases | 8 pass |
| Total selected tests | **721 pass**, zero failed/ignored |
| Harness Desktop-feature library check | Pass |
| Harness Mobile-feature library check on macOS | Pass; no native package |
| Changed Rust formatting and staged diff checks | Pass, 25 Rust files |

The Mobile-feature library check is recorded separately in `results.json`; a
macOS-host feature check does not establish iOS/Android cross-compilation,
signed packages, native UI or device acceptance.

- Team profile integration uses the real Desktop assembly, registered Bash and
  macOS sandbox. It covers principal separation, immutable snapshots, input byte
  limits, path/glob rejection, revoked captured authority, discarded owner
  capabilities, selected credentials, no background/unsandboxed fallback,
  explicit memory roots and rendered prompt isolation, file read/write and
  soft/hard-link denial, loopback network denial, environment/startup-script
  removal, host approval and headless service reapplication.
- A valid external Git repository/fsmonitor has a positive control proving that
  ordinary host Git runs its marker. Team build, prompt and transcript append
  must then leave the marker absent and omit external commit/branch data.
- POSIX runner regressions include normal completion and cancellation with a
  child ready-handshake, then prove the owned child no longer exists.
- Credential regression first proves the ambient synthetic key works on the
  ordinary provider, then proves Team-style fallback suppression rejects it
  while retaining an explicitly selected memory credential. Existing tool API,
  shell and Side Chat regressions are also included.

Final counts, command results, source hashes and compressed logs are in
`results.json` and `logs/`. Independent final static review covered 27 files and
approved delivery **only as a disabled prerequisite**, with no unresolved
CRITICAL/HIGH findings; the reviewer did not run tests.

The initial restricted run correctly failed because the outer execution sandbox
could not apply a nested macOS sandbox (`sandbox_apply: Operation not permitted`).
The actual OS acceptance run used the authorized elevated tool context so the
Team sandbox could start. Assertions check successful allowed operations plus
OS permission-denial/status metadata, not merely a generic tool error flag.

The preceding quota report's command `cargo check -p harness-runtime --lib`
checked default engine features. It did not compile Desktop assembly. That
historical scope is corrected in its report; this checkpoint explicitly checks
the Desktop feature. Prior 1,440 model-layer quota tests are historical evidence,
not a fresh result in this checkpoint.

## Remaining gates

1. Consume a backend one-shot start and launch a fresh invocation process with
   a clean environment, selected credentials and a narrow authenticated transport.
   Native SDK process-environment readers still exist; current credential tests
   do not establish full provider-environment isolation or actual request privacy.
2. Install owner-only approvals and a separate original-human question broker,
   rechecking exact UID/generation, question identity, membership and revocation
   after waits. Generic owner/remote-control IPC cannot answer human questions.
3. Enforce the total five-minute deadline across model/tools/approval waits and
   reliably terminate detached descendants. Revocation of tool/quota authority
   alone does not stop already-admitted work. CPU/memory/disk ceilings are open.
4. Integrate durable queue/start/result/retry/recovery. An ambiguous restart must
   not create a fresh quota or automatically replay a previously started tool.
5. Deliver Desktop/Web/iOS/Android trigger, question and recovery UI together;
   validate real model/host crashes, offline/rejoin/revocation and complete signed
   products/devices. Linux/Windows profiles currently reject launch; they have
   no runtime acceptance in this checkpoint.

Defaults are 12 turns, 32,768 human-input UTF-8 bytes, 4,096 output tokens/request
and 24 physical dispatches; hard host ceilings are 20/32,768/4,096/32. Snapshot
text is at most 65,536 bytes. This is not a total-input/token/monetary ceiling.

Only the Teams companion/product worktrees were edited. Build logs/targets are
retained; no cache cleanup occurred. Disk checks stayed above the 2 GiB stop
line. Tests' temporary subprocesses and loopback listener finished; no persistent
browser, emulator or server was started. Private account/session files remain
untouched with their existing reuse authorization.
