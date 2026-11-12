# Desktop cold-session recovery diagnostics

## Reproduced false timeout

The old packaged smoke waited up to 20 seconds for
`document.body.innerText` to contain the saved assistant reply. Transcript rows
use `content-visibility: auto`. Chromium can skip their descendants outside the
paint viewport, leaving the reply out of `innerText` even though both the
conversation state and the assistant row's `textContent` are complete. Waiting
longer does not make an offscreen row paint.

The real Electron regression in `apps/electron/test/cold-restore-interaction.test.mjs`
mounts the production `useBridge` and `Stage`, imports the production stylesheet,
and replays the saved two-message conversation. It covers successful IPC, a
rejected first request, and a stalled first request. It then hides the native
window and reloads without changing the messages. The old condition
times out at its original 20-second deadline while both saved items and the
assistant DOM text remain present. A normal second launch activates the existing
native window and makes the same reply appear in `innerText` again. CDP focus
alone leaves a hidden native window hidden.

Commit `7d26fb58856ecb3523efff152e322c37de172ed3` changed the smoke's text
lookup. The stronger check now requires an assistant row in the active Stage,
checks its session key where provided, and separately brings the actual window
forward and verifies painted text. It does not extend the 20-second deadline.

This establishes a reproducible false-timeout mechanism. The original one-off
failure saved only `innerText` and the host bootstrap, not renderer state or DOM
content, so that historical occurrence cannot be uniquely attributed to this
mechanism. The unchanged original signed package passed a subsequent run with
50 reloads/runtime restarts, then another 50-cycle run alternating 25 genuinely
cold owners and 25 cached-runtime restarts. Those passes alone do not prove
recovery races cannot happen.

## Following a recovery across processes

The ordinary Desktop diagnostics now include `session_recovery` events:

| Phase | What it establishes |
| --- | --- |
| `native_base_cached` | Runtime generation, sequence, base event type and saved-message count |
| `native_replay_snapshot` | Which cached bases the main process returned to the window |
| `preload_subscribe`, `preload_request`, `preload_retry`, `preload_received`, `preload_failed`, `preload_dispose` | Document/subscription identity, attempt, request duration and returned-event count |
| `renderer_base` | The renderer received the base for this session and sequence |
| `renderer_connection`, `renderer_bootstrap` | Connection transitions and accepted bootstrap revision |
| `renderer_visibility` | Document visibility and focus, including hidden windows that retain focus |
| `renderer_commit` | Committed item count, previous count, session key and connection |
| `stage_commit` | Presented item and row counts |

Connection diagnostics also carry session and runtime identities. Correlate by
session, native generation/sequence, renderer document/subscription and time.
An item-count drop can be an intentional restart or clear; it is not by itself
a recovery error. A final failed preload request is a warning.

Renderer samples accept only an allowlist of phases, enums, bounded identifiers
and nonnegative counters. Unknown fields, free-form strings and paths are
rejected before logging. Samples do not include transcript content, tool
arguments, credentials or error bodies. The IPC uses the existing registered
main-frame ownership check, caps each window at 1,200 samples per minute, and is
removed when its manager is disposed. Diagnostic observer failures cannot stop
replay delivery.

## Reproduction and package acceptance

From `apps/electron`, run the repository's TypeScript/CSS test loader for
`test/preload-event-replay.test.ts`, `test/session-recovery-diagnostics.test.ts`
and `test/cold-restore-interaction.test.mjs`. The Electron test needs a local
fixture HTTP port and the macOS window service. It uses an isolated profile.

Always build with `npm run package:mac:flare`. The stock static and full package
smokes remain required. Set `LINGXI_COLD_RELOAD_CYCLES=50` to extend the default
10 cycles; supported values are 10 through 100. Cycles alternate closing the
runtime with `/clear` and reopening the preserved saved conversation, versus
restarting the cached runtime. Every cycle then reloads the renderer. The
disposable smoke profile is the only profile modified by this test.

With `LINGXI_LOCAL_APP_EVIDENCE` set, `cold-open-cycles.json` records each
cycle's recovery type, elapsed time, session/item/row identity, DOM/paint
checks, viewport geometry and recovery trace. On failure,
`cold-open-failure.json` additionally captures bootstrap and replay information
for the generated fixture conversation. State restoration and foreground
painting have separate assertions, so a missing transcript cannot pass merely
because the runtime connected or an unrelated page contains the same text.

During signed-package verification of the diagnostics build, 33 of 50 cycle
samples had `document.visibilityState === 'hidden'` while
`document.hasFocus() === true`. All 50 had two recovered items and two Stage
rows, including the complete assistant DOM text. The original CDP-only final
paint check timed out after 20 seconds. This is a captured instance of the
false-timeout mechanism, with native replay, renderer commits and DOM evidence.

The foreground check now uses the product's ordinary single-instance second
launch to show/focus its existing native window, verifies that the temporary
activation process exits while the original process remains alive, and requires
actual document visibility before checking painted text. It does not substitute
focus emulation for visibility. Recovery samples also record visibility and
focus on document/window transitions and message commits.
