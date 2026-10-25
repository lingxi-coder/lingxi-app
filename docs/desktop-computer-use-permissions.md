# Desktop Computer Use and Browser permissions

This change connects the settings pages to Electron main, the Computer Use
execution path, and a LingXi Chrome native-messaging extension. Claude Code
2.1.310 is the pinned permission-behavior reference. LingXi retains its combined
`computer` and `browser` tool adapters; their schemas and browser transport are
not Anthropic's separate MCP interfaces.

## Device authority and revocation

Electron main owns `computer-use/policy.json` under the application's user-data
directory. It writes a bounded, owner-only regular file through atomic rename.
A separate `execution-policy.json` binds the exact source bytes and the owning
Desktop process ID. Runtime checks reject a stale source or an exited owner.
Renderer code receives policy values through authenticated application IPC;
it cannot choose the path or supply browser transport credentials.

Settings mutations carry the currently displayed revision. Stale, concurrent,
malformed and failed writes do not replace the active policy. Unreadable policy
files fail closed and are preserved for recovery. A latched authority failure
stops existing sidecars even when Side Chats are active; restoring old file
bytes cannot keep an old runtime alive. Restart Desktop after repairing it. The main process resolves
application identities through NSWorkspace before storing them.

The Any App switch is a master enable switch. It does not grant every app.
Applications still need a session approval or an explicit permanent grant.
Browser/trading apps are read-only through native Computer Use; terminal/IDE
apps have click access; other apps have full access unless policy denies them.
Clipboard read, clipboard write and system shortcuts require separate session
grants. Native browser input uses the separately authorized Browser integration.

Removing an always-allowed app changes it to denied. “Ask again” removes that
deny decision and requires a fresh approval. Any device-policy revision clears
old session grants, flags and observations. Re-enabling control cannot revive
the previous session's grants. Permanent grants are then re-evaluated against
current app policy. Disconnecting Browser also changes the device revision so
pending conversation approval cards cannot outlive the connection.

The runtime checks policy at entry, before and after awaited preparation,
before effects, at every batch step, and before returning a cached native
frame. Long holds poll the policy while waiting, so revocation releases inputs
even if the host cancellation transport is unavailable. Host notifications also
deny pending approvals and interrupt a turn that used Computer Use or Browser.
Cleanup remains possible after control is disabled.

The native macOS backend uses system-wide Accessibility hit testing for input
targets, including their process identity. Current-cursor operations pin the
checked coordinate. Screenshots compose authorized window captures on an opaque
canvas and mask other windows; the reported filtering capability is `mask`,
not native compositor exclusion. Locked display operations are denied. Locked
background app control and the Excel add-in are unavailable and remain disabled
in settings.

## Browser execution

The main process starts an authenticated loopback service. Independent agent
and native-host credentials are written only to owner-only descriptors. Browser
HTTP endpoints reject web Origin requests; the Chrome extension talks through
the native-messaging relay, rather than reading model credentials.

The runtime's Browser adapter derives session identity from trusted invocation
state and a per-tool-instance identity. Website approval uses the existing
trusted conversation card. Its single-use nonce is bound to the complete
request, scope, session and policy revision, and is not returned to the model.
Generic tool bypass modes cannot bypass this exchange.

The Chrome extension owns a tab group per runtime session. It does not expose
tabs in other groups. Actual heartbeat/handshake determines connection status;
installing a native-host manifest alone does not mean the extension is connected.
Changing profiles or removing the connection clears grants and pending work.
Removing it also rotates the native credential, preventing an old relay from
reauthorizing itself after reinstallation.

Browser settings enforce independent browsing, history and download permissions.
Website exceptions match exact HTTP(S) origins, including scheme and port;
subdomains and other ports have independent defaults. Document navigation is
intercepted so redirects to unapproved origins are rejected. Queued and active
operations have cancellation and revision checks. Host request timeouts cancel
the owning extension session, preventing delayed CDP work from resuming input.
Revocation waits for pending debugger setup and teardown; nested out-of-process
frames receive navigation guards before they resume.

For the development distribution, choose **Set up extension**, open
`chrome://extensions`, enable Developer mode, and load the shipped
`browser-extension` directory unpacked. See the extension's
[setup and supported operations](../apps/electron/browser-extension/README.md).
No extension is silently installed in an existing browser profile.

Supported-operation limits are explicit:

- Downloads require an approved same-origin tab, reject redirects, are limited
  to 8 MiB, and save only fully materialized content through Chrome's Save As UI.
  These settings govern LingXi-requested downloads, not autonomous page blob/data
  downloads across the browser.
- CDP access is restricted to the approved target. It cannot change the
  navigation guard, create arbitrary targets, read profile-wide cookies or
  override download policy.
- Site tools require the browser's WebMCP API and a tool registered by the
  approved page. An absent API returns an unavailable error.

These are LingXi integration boundaries, not evidence that every setting in the
reference screenshot exists in Claude Code CLI or has identical wire bytes.

## Validation and delivery boundaries

The hash-pinned reference oracle and generated fixture live in Harness:
`docs/parity/computer-use-2.1.310.md` and
`crates/tools/computer-use/fixtures/claude-code-2.1.310.json`.
The reference replay has 1,573 cases; the Rust differential tests currently
consume all 694 classification and 22 key-policy cases. These counts must not
be presented as full native/browser protocol parity.

Focused tests cover policy persistence, stale writes, subset approvals,
revocation, batch and held-input cleanup, target changes, cursor pinning, TCC,
locked screens, media delivery, exact origins, redirects, browser identity and
queued cancellation. The Electron interaction fixture uses a controlled host
and checks confirmed-state rendering, error recovery and focus. It is not a
real OS or Chrome acceptance test.

Cross-repository validation uses an isolated product snapshot with only the
feature's source overlays. The canonical checkout's unrelated staged/unstaged
files are preserved. The App pins Harness commit
`e19b7ebcffd5c2122f312f7ec6e778a23680fdb6`.

Final focused validation includes 151 Rust Computer Use library tests, 43
Electron policy/Browser/extension/interaction tests, two forced-sidecar-stop
regressions, and TypeScript checks. The 1,573-case pinned oracle replay passes.
The broader Electron suite has unrelated existing failures; it is not reported
as passing.

The opt-in `apps/electron/scripts/browser-real-acceptance.mjs` ran against a
fresh Chrome for Testing profile and the real native relay. Its 11 checks cover
handshake, approval before effects, navigation, screenshots with 2× CSS mapping,
click/type DOM changes, site denial, redirect denial, nested cross-site iframe
navigation denial, revocation and removal. The compact sanitized receipt is
[evidence/browser-real-2026-10-24.json](desktop-computer-use/evidence/browser-real-2026-10-24.json).
Its source hashes bind the implementation under test. Signed Desktop package
and OS Accessibility/TCC end-to-end acceptance remain separate, unrun gates.

## Signed native acceptance follow-up (2026-10-25)

Real packaging exposed two integration gaps that mocked tests did not cover:
workspace environment composition discarded the Desktop authority paths, and
Desktop tools had no generic preauthorization before their dedicated resolver.
App `37c7c0a7f` now passes the two reserved paths through a separate host-owned
field, overriding inherited/workspace copies immediately before spawn. Its
regression records actual child environments through startup and two restarts.
Harness `e19b7ebc` adds exact `computer`/`browser` generic allow rules only for
Transport sessions with the dedicated resolver and corresponding host/backend
integration. Explicit ask/deny, managed-only rules and dispatcher ceilings
remain in effect. Four real policy-gate regressions and independent review pass.

The Flare-signed environment-fix candidate passed full package smoke and 13
native checks using an explicit generic allow rule in its disposable profile:
authorized/denied application requests, device master switch, actual click and
typing effects, Retina screenshot coordinates, an independent unauthorized
visible window masked across its whole sampled content region, permanent-grant
removal, ask reset, active-batch revocation and no revival on re-enable.
Candidate receipts are deliberately marked as candidate evidence. Existing
system grants were used; this is not a fresh TCC grant or OS-revocation pass.

The opt-in `computer-use-packaged-acceptance.mjs` drives the signed application,
real bundled sidecar and native macOS backend through public IPC and a local
synthetic vision/tool provider. It never writes TCC databases or grants system
permissions. Its private status/control files coordinate operator actions for
Accessibility and Screen Recording roundtrips. `--fixture-root` must be a new
non-temporary workspace directory: this macOS does not resolve `/tmp` bundles
through LaunchServices. Each fixture has a unique bundle ID, and remains on
disk as a diagnostic artifact after its process is stopped and registration is
removed. `--explicit-computer-rule` is for candidate isolation only and must be
omitted for final default-pipeline acceptance.

Final package and system roundtrip results will be recorded separately after
the new pinned build and operator steps complete. The development acceptance
build uses release optimization with LTO disabled due to constrained disk;
it is not default thin-LTO, Developer ID notarization or distribution acceptance.

Normal LaunchServices launch was necessary for the OS tests: directly spawning
the executable made Codex the TCC responsible application. The workflow now
checks this run's bounded system attribution events and requires the signed
LingXi application as the responsible process before any OS toggle. The
updated e19 package passes the default generic/dedicated pipeline without a
persisted explicit computer allow rule. With that verified responsibility,
Accessibility removal blocked actual clicking and typing, but its original
prompt-returned status did not expose the missing permission. This is recorded
as an incomplete roundtrip, not acceptance. Harness `366a23e6` separates live
queries, revokes grants/frames/held inputs on known OS loss, and uses a bounded
AX message to verify effective accessibility; unavailable/busy queries reject
only the current operation. The complete Computer Use regression has 161
passing tests. The new signed-package roundtrip remains pending.
