# Mobile directory projects and Linux user storage

Android and iOS use the shared Harness directory/session lifecycle. Native project
records group imported sources and conversation metadata; the Linux cwd identifies
the transcript bucket. Selecting another directory keeps the selected session UUID
and moves its transcript and sidecars through the shared session transaction.
`/add-dir` records the same one-hop directory association used on Desktop.

## Storage

`mobile_user_directories(managed_root)` is the shared path authority. It uses the
runtime account declared by `mobile_linux_api::guest_paths` and the product's
`branding::DOT_DIR`; native clients do not invent a second account or config root.
For the current LingXi account the paths are:

| Data | Linux path | Native backing |
| --- | --- | --- |
| Account HOME | `/home/lingxi` | `<managed_root>/persistent/lingxi` |
| Default cwd | `~/lingxi` | `<HOME>/lingxi` |
| Project sources and manifests | `~/lingxi/projects` | `<HOME>/lingxi/projects` |
| Scheduled conversation cwd | `~/lingxi/scheduled` | `<HOME>/lingxi/scheduled` |
| Configuration and transcripts | `~/.lingxi` | `<HOME>/.lingxi` |
| Reserved LocalApp source directory | `~/.lingxi/.apps` | `<HOME>/.lingxi/.apps` |

The `projects` mount is stable for the account. A conversation's selected cwd does
not replace that mount. Both FFI constructors reject another host root or mount
identity. Workspaces must resolve to an existing directory inside HOME; canonical
containment checks reject an escaping symlink. User storage preparation rejects
symlinks in the owned directory chain.

Native project records persist guest `directory`, `activeDirectory`, and linked
`additionalDirectories`. iOS normalizes the native managed-root container alias before deriving account
paths. Native paths are resolved when accessing files, so an iOS
container's changing host prefix is not session identity. Source mirroring always
uses the original source workspace, even after the active cwd changes. A missing
active folder remains visible as a project and fails explicitly when opened; it
is not silently recreated as an empty workspace.

Old app-root projects, sandbox transcript catalogs, workspace UUID preferences,
and migration fallbacks are not read by this layout. Removing those readers does
not delete data from a running installation.

## Session lifecycle

The directory picker captures the source and session UUID. The shared runtime
rejects a stale selection and serializes New, Resume, directory changes, and state
snapshots through the same lifecycle lease. Native restoration additionally checks
the recorded workspace before adopting the last active session.

Session discovery has explicit Current, Repository, and All scopes. Linux Git
worktrees are discovered by guest Git. Resume uses the exact discovered transcript
path rather than searching another directory for a matching UUID. A missing or
escaping recorded cwd is rejected before any active owner changes. Empty mobile
sessions record cwd atomically in their anchor and preserve the original root
when moved before their first prompt. Session catalog rows carry the loader's
lossless workspace hint; native indexes retain confirmed rows from directories
not covered by the current workspace listing. A deleted linked directory does
not block persistence of a still-valid current workspace. Working-directory
changes refresh the skill/custom-agent catalog, configured MCP servers, and session
cron file. Checkpoint and parked-agent persistence retain their originating owner
when a different session becomes active.

Engine-level settings that require reconnection retain the existing mobile
reconnect behavior. This change does not make every Desktop host service a mobile
hot-reload service.

## Verification boundaries

The regression suites exercise real filesystem writes, transcript moves and cold
handle reconstruction, fixed mount validation, stale pickers, linked directories,
skill documents after `/cd`, cron rebinding, and native project persistence.

Simulator native-support builds contain the SDK's simulator stubs. They verify
compilation and FFI integration, not a running iSH guest. Android JVM tests likewise
do not replace a real PRoot launch. Physical Android/iOS runtime and UI acceptance
must be reported separately from these source and build checks.
