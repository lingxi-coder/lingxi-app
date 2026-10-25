# Local App implementation and acceptance checkpoint

Publication checkpoint: 2026-10-25. The unified Local App implementation and
AG-UI integration are merged for source delivery. Full cross-platform acceptance
is **not complete**.

## Source and distribution

- Local App identity, immutable releases, expected-base publish transactions,
  data directories, execution consent and runtime ownership are independent of
  ordinary development sessions.
- Mobile pages use the system-resolved external browser. Desktop retains its
  application windows. Agent execution, credentials and approvals stay in the
  trusted native host; public Agent messages use the pinned official AG-UI 1.0
  contract.
- Source-release market validation includes release-local Agent Skills. Skill
  IDs are unique, paths must be `skills/<id>/SKILL.md`, and instruction files
  must be in the explicit public package allowlist. Packages cannot serialize
  local grants.
- Product Cargo dependencies resolve to Harness
  `9c2de9653d00306e92eebb67e8732737b42e59ea` and Mobile Linux Runtime
  `bfd17e346f7ebbb2bcd5206f3c05f56b83a1858b`. The Harness merge retains the
  existing main Accessibility diagnostics alongside the Local App changes. The
  SDK merge includes both branded Project mounts and the previously validated
  Android host-death / iOS root-directory, process-exit, truncate and covered-path
  preservation fixes. Native patch files match the original validated revision;
  combined Swift mount planning and transition behavior has separate regression
  coverage. These simulator/unit results do not replace physical service tests.

## Validation for this source merge

| Check | Result |
| --- | --- |
| Harness `local-app`, no default features, library tests | 83 passed |
| Product Agent SDK, source-chat navigation, AG-UI fixture and package-smoke tests | 47 passed, no skips |
| SDK merged Android/API/core/iOS Rust suites | 129 passed |
| SDK merged NativeSupport Swift simulator unit tests | 40 passed |
| SDK merged guest-user/rootfs and native patch script tests | 9 + 7 passed |
| Market complete unit/domain suite | 56 passed; 4 Firebase emulator tests not run |
| Market TypeScript and production build | Passed |
| Final product Cargo locked dependency metadata | Verified: 80 Harness packages and 5 SDK packages |
| Product Desktop node/web TypeScript and Play source-channel gate | Passed |

The market checks exercised source upload, review and compatible distribution
with Agent Skills using the production domain code and memory-backed test
storage. They do not establish a live market deployment.

## Package and device evidence boundary

Only one active flavor per platform is in scope: Android PlayDebug, independent
iOS FullDebug, and Desktop Flare development signing. Play does not gain Linux
execution capabilities to make a service fixture pass.

The Flare archive with SHA-256
`60432ad9a97aac011fb69fa5d00a6f0577723a50c7635810224cf70f8a988283`
passed signing/static verification and the official packaged runtime smoke.
The smoke script was corrected to click the current app-scoped **Modify**
action and fail if the button is unavailable. Its prior stale label silently
skipped that click. The passing rerun verified independent application windows,
durable storage, correct source-session/transcript/draft restoration, terminal,
Git and credential persistence. The signed archive itself was unchanged by the
test correction. Its source graph predates this final main integration; this is
not a claim that the final merge was rebuilt or device-tested.

Remaining acceptance includes the refreshed PlayDebug File/image model error,
FullDebug physical source/Linux/browser/background expiry recovery, current
Agent live-provider checks, actual test-account sign-out independence, and
remaining market/platform end-to-end scenarios. Earlier package/device results
remain historical evidence and must not be relabelled as results for a newer
source graph.

Detailed local logs and immutable artifact receipts are retained under
`acceptance/mobile-local-app-20261023` and
`acceptance/mobile-app-agent-20261023` in the multi-checkout workspace. Those
local evidence directories are not shipped application data or uploaded source
packages.
