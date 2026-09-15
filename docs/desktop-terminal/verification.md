# Desktop bottom terminal

The terminal is a Desktop-only native PTY surface. Open it from the chat header or with Ctrl+backquote. Each conversation has independent tabs; hiding the panel or switching chats leaves the processes running. Closing a tab terminates its shell and jobs. Restarting the application does not rerun commands.

## Implementation

- `clients/electron/src/renderer/components/TerminalPanel.tsx` provides lazy-loaded xterm.js surfaces, theme matching, resizing, keyboard/IME handling, and per-conversation panel state. Terminal surfaces remain mounted while hidden.
- `clients/electron/src/main/terminal.ts` manages a dedicated sidecar and bounded output snapshots. `terminal-delivery.ts` batches acknowledged output and pauses upstream reads when a renderer falls behind; live ANSI bytes are never discarded. Navigation/crash tears down obsolete acknowledgement ownership.
- The preload exposes only scoped terminal operations. Host validation requires a registered main-frame sender, a known project/session, and an attached terminal. A terminal needs no model connection. Existing pre-message session UUIDs stay stable; fallback drafts migrate without restarting the shell.
- `lingxi-code/apps/bridge-server/src/desktop_terminal.rs` handles the independent `--desktop-terminal` JSONL mode using `platform-pty`. It starts login shells, decodes split UTF-8, resizes PTYs and shuts down on EOF/SIGTERM. POSIX session membership catches jobs reparented after their original shell exits. Windows uses the existing PTY Job Object.
- Credentials are excluded from the sidecar/shell environment. Output is not sent to the model or stored in the conversation. Raw retained history is bounded to 1 Mi UTF-16 code units; live xterm scrollback is 5,000 lines. Full terminal emulation state is retained while switching/hiding, not serialized across a renderer/application restart.

## Validation

- Terminal manager, host boundary and lossless delivery unit tests pass.
- Real Electron renderer tests cover tabs, hide/resume, conversation focus isolation, Unicode composition, Ctrl+C/Tab, resize, theme, close failure and restart, and draft migration. Visual comparison score: 93/100. Screenshots: `/tmp/lingxi-terminal-ui/light.png` and `/tmp/lingxi-terminal-ui/dark.png`.
- Seven Rust terminal tests pass, including Unicode decoding, malformed input limits, actual PTY resize/interrupt, EOF, separate process-group cleanup and reparented nohup cleanup.
- Full Desktop test run: 991 passed, 3 failed, 1 skipped (native integration requires an explicit binary). Existing unrelated failures: conversation `/loop` folded-ID expectations; a ProviderEditorFields tooltip guard; settings navigation count (17 versus expected 18).
- TypeScript main/preload and renderer type checks pass.

Final release checks:

- All 18 terminal/host/delivery/renderer targeted tests passed.
- Both native integration tests passed against the release sidecar: real vim Unicode editing, native Tab completion, Ctrl+C, resize, 40,000 output lines, independent sessions, draft migration, 34 restart cycles, orphan cleanup, and SIGTERM with stdin open and stdout backpressured.
- All 9 `platform-pty` tests passed with the repository Rust toolchain.
- `package:mac:flare -- --launch` passed inside-out signing, static package validation and runtime smoke checks. The packaged UI executed Unicode shell commands in the correct working directory, kept running while hidden, restored the panel, and closed the terminal. Existing credential persistence/restart and authenticated localhost model checks also passed.
- Final packaged screenshot score: 95/100. A native tab-strip scrollbar artifact found during production screenshot review was fixed and verified in a second signed package.
- Packaged Desktop launched successfully. Evidence logs: `/tmp/lingxi-terminal-package3.log`, `/tmp/lingxi-terminal-release-integration.log`, `/tmp/lingxi-terminal-pty-tests-final.log`, `/tmp/lingxi-terminal-targeted-final.log`.
- An earlier build exhausted local disk. Only rebuildable Rust incremental caches and this task's accidentally duplicated toolchain artifacts were removed; successful checks above ran after recovery.

## Platform limits

Runtime validation is on macOS ARM64. Linux and Windows branches require their respective hosts for native validation. No remote SSH management or process restoration after application exit is included.
