# Brand workspace storage

Private product state and agent working files use separate sibling directories.
For LingXi, the private configuration home is `~/.lingxi` and the workspace root
is `~/.lingxi-workspaces`. Other brands derive the workspace name from their
configuration directory name by appending `-workspaces`.

On Desktop, `~` is the operating-system user's home. An explicit configuration
home also determines its workspace sibling: `/custom/.atlas` uses
`/custom/.atlas-workspaces`. The native `product-paths` response (version 2)
provides both roots; Electron does not guess the brand or accept version 1.
On mobile, `~` is the selected Linux account's home, backed by
`<managedRoot>/persistent/<linuxUser>`.

| Content | Location relative to workspace root |
| --- | --- |
| No-project Chat | `chat/` |
| No-project Code | `code/` |
| Mobile managed project sources | `projects/` |
| Local App authoring sources | `apps/<source-name>/` |
| Local App Chat tool files | `app-agents/<appId>/chats/<chatId>/` |
| Local App independent task files | `app-agents/<appId>/tasks/<taskStorageId>/` |
| Mobile scheduler controller | `scheduled/` |
| Generated cron run files | `cron/<sha256(taskId)>/runs/<date>/<title>-<runId>/` |
| Cron task memory | `cron/<sha256(taskId)>/memory.md` |

Each generated cron run contains `work/` for intermediate files and `outputs/`
for deliverables. Selected existing project or session tasks retain their exact
authorized cwd. Their scheduler ledger remains with its controller. Mobile's
private `automations/sessions/<sessionId>.json` maps a generated session back to
its cron workspace, and is resolved against the workspace root.

Configuration, provider credentials, global memory, conversation ownership,
transcripts, Local App native authorization receipts and the installed App
library remain private. Installed immutable releases, application business data
and application process state retain their library locations under
`local-apps/v2/`. Agent tools use the workspace directories above as cwd, never
the private transcript or authorization directory.

Owned directory chains reject symlinks. Mobile host/guest mappings must be exact
and authorized, and the configuration tree cannot be granted as a workspace.
Explicit user-selected project directories are unchanged.

This is the only current layout. Runtime code does not probe old workspace roots,
decode the old product-paths response or migrate old data on startup. A one-time
operator move of selected local sources and session ownership is separate from
the shipped runtime.
