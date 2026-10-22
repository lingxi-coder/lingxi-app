# Desktop Local Apps: upstream references

The running application is an Electron-owned service. Harness supplies the model tool and checks publication reads; application runtime and library operations share the main-process service.

| Upstream | Pinned source | Adoption |
| --- | --- | --- |
| Dyad | [`c3765d152eaeff325a3ebef8626bdc44fd773aec`](https://github.com/dyad-sh/dyad/tree/c3765d152eaeff325a3ebef8626bdc44fd773aec) | `upstream/dyadInvocation.ts` is the upstream invocation module with its clock interface made local. Its cancellation tombstones prevent canceled queued starts from becoming live. Apache-2.0 terms are retained in `upstream/DYAD-LICENSE`. |
| Dyad | `src/ipc/services/app_runtime_service.ts`, `app_operation_coordinator.ts` | The runtime adapts the transport-neutral start/restart/stop seam, serialized per-app operations, readiness cleanup, output captured by invocation, and deletion after draining active work. Database, account and cloud adapters are not imported. |
| Pinokio / pinokiod | [`add4a674ad1ba95bb62ec427fce519fe5bf0e9bf`](https://github.com/pinokiocomputer/pinokiod/tree/add4a674ad1ba95bb62ec427fce519fe5bf0e9bf), `kernel/shell.js`, `kernel/shells.js` | The manifest adopts explicit command/cwd/env/venv steps, separate installation and persistent services, readiness output, and app-owned process groups. The shell implementation is original; the package's Conda/UI/server framework is not embedded. |

Runtime tests cover cancellation before admission, duplicate starts, interrupted install, readiness failure, stopped port release, source preservation and the upstream identity primitives. Electron integration tests cover actual independent windows and the authenticated model-tool endpoint.

Desktop publication produces an immutable source revision. Install/build runs in a separate revision workspace; durable app data and browser storage outlive revisions. No `node_modules`, virtual environments, `.git`, `.env` credentials or source symlinks are imported.

Creation and repairs use the existing Bash permission path. Runtime commands subsequently execute under the user's app-level native-execution grant. This is native account execution, not a process filesystem sandbox. The renderer remains sandboxed and receives only the narrow `window.localApp` bridge.
