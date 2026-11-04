# Teams T0 — 2026-10-29

T0 passed. The reviewed Firestore prototype now enforces trusted conversation
membership and complete message-mutation catch-up. Real Electron main and browser
SDK listeners work; both pinned native SDK size probes build successfully.
T1–T5 functionality is still pending.

Base: `claude/teams`, fast-forwarded to Bot acceptance record `492a62e97` (source
base `82e718fbf`). The changes were made only in the Teams worktree. The owning
contract is [docs/TEAMS.md](../../docs/TEAMS.md).

## Changes

- Conversation creation is a trusted transaction that derives exact read ACLs,
  verifies current members/Bot policy and includes Bot owners. Clients cannot
  create or rewrite those ACLs. Reusing a conversation ID with a different
  request conflicts.
- Bot DM sends recheck the allowlist. Own-member server-time heartbeats work;
  role/generation changes remain denied. A deleted team also denies access.
- Message creates, edits and recalls each commit a message, immutable change
  receipt and conversation cursor atomically. Creation order/read watermarks stay
  separate from change order. A rules-validated SHA-256 body digest plus immutable
  message metadata preserves retry evidence after later edits/recalls.
- `(default)` has a separate rules file. The named service databases remain
  deny-all. Unimplemented invites, tasks, App grants/opens and epoch paths remain
  closed to clients.
- The contract specifies private memory/home isolation, authenticated triggers,
  durable membership epochs, verifiable human provenance, safe crash recovery
  and stable Local App revocation principals before T3–T5 implementation.

## Validation

| Check | Result |
| --- | --- |
| Official rules-unit-testing + real client transaction | 15 passed, no skips |
| Desktop/Web SDK authentication, listeners, account mismatch and cleanup | 6 passed, no skips |
| Real Electron 43.1.1 main, Node 24.18.0 | Custom-token sign-in and a live Firestore change passed |
| Real browser through Playwright | Bundled browser SDK auth and old-message recall catch-up passed |
| Backend unit suite | 23 passed; 22 environment-dependent tests skipped in this invocation |
| Web unit suite | 231 passed; 1 emulator-only test skipped and passed in the dedicated run |
| Desktop main/Web type checks | Passed |
| Bridge client/Web builds | Passed |
| Independent final code review | No remaining actionable T0 regression found |

The recorded Node test runner was 25.2.1; the actual Electron check exercised its
own Node 24.18.0 runtime. The Web unit suite used Python 3.12.14 and the Bot
worktree's matching React test dependencies. Desktop type checking used a freshly
built Teams-worktree bridge client rather than the primary checkout's stale dist.
Dependency links and builds stayed in this worktree; shared source directories
were not modified.

`firebase-tools` does not initialize multiple databases' rules in the Firestore
emulator. The runner therefore configures only `(default)`. Tests separately
compile each configured deny-all file in an isolated test project and assert the
production `firebase.json` mapping. This is not a production multi-database deploy.

## Native dependency footprint

Both minimal baselines include the same existing Firebase products. The second
variant adds Firestore, using the product's pinned SDK versions.

| Probe | Baseline | With Firestore | Increase |
| --- | ---: | ---: | ---: |
| Android Debug APK, unminified, BoM 35.0.2 / Firestore 27.0.1 | 16.91 MiB | 26.21 MiB | 9.30 MiB |
| Android uncompressed APK entries | 16.76 MiB | 26.49 MiB | 9.73 MiB |
| iOS unsigned arm64 Release app, Firebase 12.19.2 | 5.44 MiB | 22.34 MiB | 16.90 MiB |

These are measured dependency probes, not full-product release sizes. Product
shrinking, other transitive dependencies, signing and packaging can change the
increment. Full iOS/Android products, signed installers and devices were not
built/installed for this T0. Retained artifacts and SHA-256 values are in
[sizes.json](sizes.json); [probe sources and commands](probes/README.md) reproduce
the comparison. No Rust or FFI regeneration was needed.

## Reproduction and evidence

Prepare the repository's normal Electron/Web/bridge-client dependencies and run:

```sh
npm --prefix apps/firebase ci
npm --prefix packages/bridge-client run build
npm --prefix apps/firebase run test:teams
```

The runner requires `firebase` CLI and Java 21, uses only the local
`demo-lingxi-teams-t0` project, ports 8085/9098 and a scratch configuration, and
stops the emulators afterward. Optional `TEAM_FIRESTORE_PORT`/`TEAM_AUTH_PORT`
override those two ports. At least 2 GiB free space is required.

To repeat actual Electron/browser checks, set `TEAM_SPIKE_COMMAND` to
`node <absolute-repository-path>/apps/firebase/scripts/teams-runtime-spikes.mjs`
when running `test:teams`. Open the printed loopback browser URL within two
minutes, using a real browser or Playwright. The process records the browser
result and shuts down after completion. It uses synthetic emulator identities,
an isolated temporary Electron user-data directory and an explicit local-only CSP.

[results.json](results.json) records source hashes and final log hashes.
[logs/](logs/) retains gzip-compressed passing logs. [browser-result.json](browser-result.json)
is the browser's observed result. Earlier failed diagnostic runs remain only in
the task's temporary output and are not acceptance evidence.

Next: implement T1 membership/invite/session HTTP APIs, durable epochs and four
client member/deep-link screens. The tested trusted conversation constructor is
ready for the authenticated API; it is not yet exported as an HTTP endpoint.
Production IAM/custom-token signing and a real second-account flow still need
their phase acceptance. No push or Firebase deploy was performed.
