# LingXi Code Desktop — Internal Beta Tester Guide

Supported build: macOS on Apple Silicon (`arm64`). This Beta is for approved
internal testers only. It is ad-hoc signed and is not notarized for public
distribution.

## Install and verify

Keep the ZIP, its `.sha256` file, and the previous Beta ZIP together.

```bash
shasum -a 256 -c LingXi-Code-0.1.0-mac-arm64.zip.sha256
```

The result must say `OK`. Unzip the archive, quit any running LingXi Code, and
move `LingXi Code.app` to `/Applications`.

Verified artifact for this Beta run (2026-07-21):

- ZIP SHA-256: `c4e85b5c79890b3df3ce2c78396d3334296c223fa99c4ec19773e80062ee5f5d`
- bundled `bridge-server` SHA-256: `1d70c1d6bd597baa3b581d7117657e3f40a687673ba26a2a6d6eb7825c9735e4`

The exact artifact passed the static package audit and packaged smoke gate.

On first launch after transferring the ZIP, macOS may block an ad-hoc signed
build. In Finder, Control-click the app, choose **Open**, then confirm **Open**.
If macOS still blocks it, use **System Settings → Privacy & Security → Open
Anyway**. Do not disable Gatekeeper globally.

## First-run setup

1. Choose a workspace using the native folder picker.
2. Review the repository before trusting it. Trust enables project hooks, MCP
   servers, agents, plugins, and local settings. A native warning requires a
   second confirmation. Changes to executable project settings revoke trust.
3. Choose a provider and enter its API key. Each value is masked, encrypted by
   macOS secure storage, and never displayed again. The picker follows the
   CLI/TUI provider set; OAuth/device sign-in is currently labeled as a
   CLI/TUI-only flow.
4. Wait for **Engine ready**. Use **Settings & diagnostics → Restart engine** if
   startup was interrupted.

Provider keys are stored as per-provider generic-password items in the macOS
Keychain. Existing Beta installs with a legacy Safe Storage blob migrate it on
the first successful launch and remove the old encrypted file. If macOS asks
for the login-keychain password during that one-time migration, approve the
item; subsequent launches read the Keychain item through `/usr/bin/security`
without starting an Electron credential helper or prompting again. If the
item is unavailable, re-enter the affected provider key in Settings.

Send remains disabled until the workspace is trusted, at least one provider is
connected, and the bundled engine is connected.

## Sessions and models

- Click **+** beside Sessions for a clean engine-issued session.
- Select a saved session to restore its ordered conversation and tool history.
- Use the Model menu only when no turn is active. The menu contains the live
  engine model catalog; the confirmed model is retained for future launches.
- Session files remain in the normal LingXi local session store and survive app
  updates or reinstalls.

## Run and cancel

Type a request and press Enter. Use Shift+Enter for a newline. Streaming answer,
thinking, tools, results, usage, and errors come directly from the local engine.
Press the red stop button to cancel an active turn. Workspace, model, trust, and
credential changes are blocked until the active turn is cancelled.

## Permission requests

Every parked request shows the tool and a bounded command/path preview:

- **Deny** rejects this request.
- **Allow once** approves only this request.
- **Allow matching actions** creates a narrowed workspace rule when supported.

Escape denies. Tab and Shift+Tab stay within the dialog. Disconnecting the
engine fails parked permissions closed.

## Background tasks

Open the stacked-tasks button in the top bar. Task rows, state, output, and stop
actions are supplied by the current engine; an empty panel means no task was
reported. Truncated output is labelled with its total line count.

## Diagnose and recover

Open **Settings & diagnostics** to inspect connection state and sanitized recent
host/bridge events. The report includes app, Electron, bridge/protocol, OS,
architecture, workspace trust, child exit, and recovery errors. It excludes
prompts, tool payloads, credential plaintext, auth headers, and bridge tokens.
The packaged log is bounded at `~/Library/Application Support/lingxi-code-desktop/logs/desktop.jsonl`;
development builds may use a different Electron profile directory.

- **Restart engine** performs a controlled bridge restart.
- **Refresh all** reloads live sessions, models, tasks, and diagnostics.
- **Copy report** copies a sanitized JSON report.
- **Export JSON…** uses a native save dialog and writes an owner-only report.

When filing an internal issue, attach the exported report and the artifact
SHA-256. Never paste an API key into the issue.

## Update and rollback

1. Verify the new ZIP checksum.
2. Quit LingXi Code completely.
3. Replace `/Applications/LingXi Code.app` with the new verified app.
4. Launch and confirm the expected workspace, trust state, sessions, model, and
   **Engine ready** state.

To roll back, repeat those steps with the retained previous verified ZIP. User
data and `~/.lingxi` sessions are not removed by replacing the application.

## Uninstall

Quit the app, move `/Applications/LingXi Code.app` to Trash, then optionally
remove its user-data directory from `~/Library/Application Support`. Removing
`~/.lingxi` also deletes LingXi sessions and configuration, so keep it unless
you intentionally want to erase that history.

## Known limitations

- Apple Silicon macOS only.
- Manual updates; no auto-update service.
- No cloud account/billing, Chat/Cowork, voice, attachments, diff/plan editor,
  interactive shell, connectors, Windows, or Linux release build.
- Ad-hoc internal signature; no Developer ID notarization.
- One active workspace/engine at a time.
