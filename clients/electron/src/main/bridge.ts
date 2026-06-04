/**
 * Main-process bridge manager (M10 A1 — C2).
 *
 * Owns the lifecycle that turns the Electron shell into a real client of the
 * Rust `bridge-server`:
 *
 *   1. Spawn the built `bridge-server` binary as a child process. Its path is
 *      resolved from `opts.serverBin` → `LINGXI_BRIDGE_SERVER_BIN` → a path
 *      derived RELATIVE to the repo (`<repoRoot>/lingxi-code/target/{debug,
 *      release}/bridge-server`, first existing); a clear, actionable error is
 *      thrown if none resolve.
 *      `ANTHROPIC_API_KEY` / `LINGXI_API_BASE_URL` pass through from the
 *      environment; `--cwd` / `--model` come from env overrides (the key is
 *      NEVER read into a string we log — it rides inherited `env` untouched).
 *   2. Wait for the F2-04 discovery lockfile to appear under `~/.claude/bridge`
 *      (`<port>.lock`). We snapshot the pre-existing lockfiles first and accept
 *      the first NEW one the child writes, so a stale lockfile from a previous
 *      run never wins the race.
 *   3. Connect a {@link BridgeClient} (from `@lingxi/bridge-client`) through that
 *      lockfile — the token in its body is the WS-upgrade auth.
 *   4. Wire IPC: the renderer drives turns / permissions through
 *      `ipcMain.handle(...)`, and every inbound {@link ClientEvent} is forwarded
 *      to the renderer via `webContents.send('lingxi:event', evt)`.
 *   5. {@link BridgeManager.dispose} kills the child, closes the socket, and
 *      removes every IPC handler + listener — called on app quit / all-windows-
 *      closed so no orphan process or dangling handler survives.
 *
 * The renderer surface (channel names) is mirrored 1:1 in `src/preload/index.ts`.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ipcMain, type WebContents } from 'electron';
import {
  BridgeClient,
  defaultBridgeDir,
  type ClientEvent,
  type PermissionRequest,
  type PermissionResponseDto,
} from '@lingxi/bridge-client';

// ── IPC channel names (mirror these in the preload) ──────────────────────────

/** Renderer → main: submit a user prompt to drive a turn. */
export const CH_SEND_PROMPT = 'lingxi:sendPrompt';
/** Renderer → main: approve a parked permission request. */
export const CH_APPROVE = 'lingxi:approve';
/** Renderer → main: deny a parked permission request. */
export const CH_DENY = 'lingxi:deny';
/** Renderer → main: cancel the in-flight turn. */
export const CH_CANCEL = 'lingxi:cancel';
/** Renderer → main (invoke): read the current connection state synchronously. */
export const CH_CONNECTION_STATE = 'lingxi:connectionState';
/** Main → renderer (send): one inbound {@link ClientEvent} from the engine. */
export const CH_EVENT = 'lingxi:event';
/** Main → renderer (send): one inbound {@link PermissionRequest} from the engine. */
export const CH_PERMISSION = 'lingxi:permission';
/** Main → renderer (send): a {@link ConnectionState} transition. */
export const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';

/** Coarse lifecycle of the bridge connection, surfaced to the renderer. */
export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

/** Tunables for {@link BridgeManager} (all optional — env-resolved by default). */
export interface BridgeManagerOptions {
  /** Override the bridge-server binary path (else `LINGXI_BRIDGE_SERVER_BIN` / default). */
  serverBin?: string;
  /** Override the engine working directory (else `LINGXI_CWD` / `process.cwd()`). */
  cwd?: string;
  /** Override the default model id (else `LINGXI_MODEL`, omitted if unset). */
  model?: string;
  /** Directory to watch for the discovery lockfile (else `~/.claude/bridge`). */
  bridgeDir?: string;
  /** How long to wait for the child to publish its lockfile (default 15_000ms). */
  lockfileTimeoutMs?: number;
}

/** Binary name we look for under the workspace `lingxi-code/target/{profile}`. */
const SERVER_BIN_NAME = 'bridge-server';

/**
 * Walk upward from `start` looking for a built `bridge-server` under
 * `<dir>/lingxi-code/target/{debug,release}/bridge-server`, returning the first
 * existing path. This anchors the binary RELATIVE to the repo (no absolute
 * author path) and works from both the bundled `out/main` and the `src/main`
 * source tree, since both live under the repo root.
 */
function findWorkspaceServerBin(start: string): string | undefined {
  let dir = start;
  // Bound the walk at the filesystem root (dirname is idempotent there).
  for (;;) {
    for (const profile of ['debug', 'release']) {
      const candidate = join(dir, 'lingxi-code', 'target', profile, SERVER_BIN_NAME);
      if (existsSync(candidate)) {
        return candidate;
      }
    }
    const parent = dirname(dir);
    if (parent === dir) {
      return undefined;
    }
    dir = parent;
  }
}

/** Absolute path of this module's directory (works for both ESM bundle + source). */
function moduleDir(): string {
  try {
    return dirname(fileURLToPath(import.meta.url));
  } catch {
    // Extremely defensive: if `import.meta.url` is unavailable, fall back to cwd.
    return process.cwd();
  }
}

/**
 * Resolve the bridge-server binary path:
 *   1. an explicit `opts.serverBin` override, else
 *   2. the `LINGXI_BRIDGE_SERVER_BIN` environment variable, else
 *   3. a path derived RELATIVE to the repo by walking up from this module to a
 *      built `lingxi-code/target/{debug,release}/bridge-server`.
 *
 * Throws an actionable error when none resolve, telling the user to build the
 * binary or set the env var — no absolute author paths are ever baked in.
 */
export function resolveServerBin(opts: BridgeManagerOptions = {}): string {
  const explicit = opts.serverBin ?? process.env['LINGXI_BRIDGE_SERVER_BIN'];
  if (explicit) {
    return explicit;
  }
  const found = findWorkspaceServerBin(moduleDir());
  if (found) {
    return found;
  }
  throw new Error(
    `bridge-server binary not found. Build it from the cargo workspace ` +
      `("cd lingxi-code && cargo build -p bridge-server --bin bridge-server", ` +
      `which emits lingxi-code/target/debug/bridge-server), or point ` +
      `LINGXI_BRIDGE_SERVER_BIN at a prebuilt binary.`,
  );
}

/** Lockfile filenames currently in `dir` (so we can tell which one the child adds). */
function snapshotLockfiles(dir: string): Set<string> {
  try {
    return new Set(readdirSync(dir).filter((n) => n.endsWith('.lock') && !n.startsWith('.')));
  } catch {
    return new Set();
  }
}

/**
 * Manages one `bridge-server` child + its {@link BridgeClient} and the IPC seam
 * between the renderer and that client. Construct once in the main process and
 * call {@link start}; call {@link dispose} on shutdown.
 */
export class BridgeManager {
  private readonly opts: BridgeManagerOptions;
  private child: ChildProcess | null = null;
  private client: BridgeClient | null = null;
  private state: ConnectionState = { status: 'idle' };
  private disposed = false;
  private ipcRegistered = false;

  /** Renderer targets to forward inbound frames + state changes to. */
  private readonly targets = new Set<WebContents>();

  constructor(opts: BridgeManagerOptions = {}) {
    this.opts = opts;
  }

  /** The latest connection state (also pushed to the renderer on every change). */
  get connectionState(): ConnectionState {
    return this.state;
  }

  /**
   * Register a renderer's {@link WebContents} to receive forwarded events. A
   * destroyed `WebContents` is auto-removed. Safe to call before {@link start}.
   */
  registerWindow(wc: WebContents): void {
    this.targets.add(wc);
    wc.once('destroyed', () => this.targets.delete(wc));
  }

  /**
   * Spawn the server, wait for its lockfile, connect the client, and wire IPC.
   * Resolves once connected; on any failure it records an `error` state, tears
   * the child down, and rejects (the caller may surface this to the renderer).
   */
  async start(): Promise<void> {
    if (this.child || this.client) {
      throw new Error('BridgeManager already started');
    }
    this.registerIpc();

    const bridgeDir = this.opts.bridgeDir ?? defaultBridgeDir();
    const preexisting = snapshotLockfiles(bridgeDir);

    this.setState({ status: 'spawning' });
    let child: ChildProcess;
    try {
      // Resolving the binary path can throw (e.g. it isn't built / no env var);
      // surface that as a clean `error` state rather than a stuck `spawning`.
      child = this.spawnServer();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.setState({ status: 'error', message });
      throw err;
    }
    this.child = child;

    child.once('exit', (code, signal) => {
      this.child = null;
      if (!this.disposed) {
        this.setState({
          status: 'disconnected',
          reason: `bridge-server exited (code=${code ?? 'null'}, signal=${signal ?? 'null'})`,
        });
      }
    });
    child.once('error', (err) => {
      if (!this.disposed) {
        this.setState({ status: 'error', message: `failed to spawn bridge-server: ${err.message}` });
      }
    });

    try {
      const lockfilePath = await this.waitForLockfile(bridgeDir, preexisting);
      this.setState({ status: 'connecting' });

      const client = new BridgeClient({ lockfilePath, clientName: 'lingxi-electron/0.1.0' });
      this.client = client;
      this.wireClient(client);

      await client.connect();
      this.setState({ status: 'connected' });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.setState({ status: 'error', message });
      this.teardownChildAndClient();
      throw err;
    }
  }

  // ── Spawn ───────────────────────────────────────────────────────────────────

  private spawnServer(): ChildProcess {
    const bin = resolveServerBin(this.opts);
    const cwd = this.opts.cwd ?? process.env['LINGXI_CWD'] ?? process.cwd();
    const model = this.opts.model ?? process.env['LINGXI_MODEL'];

    const args: string[] = ['--cwd', cwd];
    if (model) {
      args.push('--model', model);
    }

    // Inherit the full environment so `ANTHROPIC_API_KEY` / `LINGXI_API_BASE_URL`
    // pass through untouched — the key is never copied into a local string here.
    return spawn(bin, args, {
      cwd,
      env: process.env,
      stdio: ['ignore', 'inherit', 'inherit'],
    });
  }

  // ── Lockfile wait ─────────────────────────────────────────────────────────

  /**
   * Poll `dir` for the first `<port>.lock` that was NOT present in `preexisting`,
   * returning its absolute path. Rejects on timeout or if the child exits first.
   */
  private waitForLockfile(dir: string, preexisting: Set<string>): Promise<string> {
    const timeoutMs = this.opts.lockfileTimeoutMs ?? 15_000;
    const deadline = Date.now() + timeoutMs;
    const pollMs = 50;

    return new Promise<string>((resolve, reject) => {
      const tick = (): void => {
        if (this.disposed) {
          reject(new Error('bridge manager disposed before lockfile appeared'));
          return;
        }
        if (!this.child) {
          reject(new Error('bridge-server exited before publishing a lockfile'));
          return;
        }
        const current = snapshotLockfiles(dir);
        for (const name of current) {
          if (!preexisting.has(name)) {
            resolve(join(dir, name));
            return;
          }
        }
        if (Date.now() >= deadline) {
          reject(new Error(`timed out after ${timeoutMs}ms waiting for bridge lockfile in ${dir}`));
          return;
        }
        setTimeout(tick, pollMs);
      };
      tick();
    });
  }

  // ── Client wiring ───────────────────────────────────────────────────────────

  private wireClient(client: BridgeClient): void {
    client.on('event', (evt: ClientEvent) => {
      this.broadcast(CH_EVENT, evt);
    });
    client.on('permission', (req: PermissionRequest) => {
      this.broadcast(CH_PERMISSION, req);
    });
    client.on('close', (code: number, reason: string) => {
      if (!this.disposed) {
        this.setState({ status: 'disconnected', reason: reason || `ws closed (code=${code})` });
      }
    });
    client.on('error', (err: Error) => {
      if (!this.disposed) {
        this.setState({ status: 'error', message: err.message });
      }
    });
  }

  // ── IPC ───────────────────────────────────────────────────────────────────

  private registerIpc(): void {
    if (this.ipcRegistered) {
      return;
    }
    this.ipcRegistered = true;

    ipcMain.handle(CH_SEND_PROMPT, (_e, text: string) => {
      this.requireClient().sendPrompt(text);
    });
    ipcMain.handle(CH_APPROVE, (_e, requestId: number, response?: PermissionResponseDto) => {
      this.requireClient().approvePermission(requestId, response ?? { type: 'allow_once' });
    });
    ipcMain.handle(CH_DENY, (_e, requestId: number) => {
      this.requireClient().denyPermission(requestId);
    });
    ipcMain.handle(CH_CANCEL, (_e, turnId?: number) => {
      this.requireClient().cancel(turnId);
    });
    ipcMain.handle(CH_CONNECTION_STATE, () => this.state);
  }

  private unregisterIpc(): void {
    if (!this.ipcRegistered) {
      return;
    }
    ipcMain.removeHandler(CH_SEND_PROMPT);
    ipcMain.removeHandler(CH_APPROVE);
    ipcMain.removeHandler(CH_DENY);
    ipcMain.removeHandler(CH_CANCEL);
    ipcMain.removeHandler(CH_CONNECTION_STATE);
    this.ipcRegistered = false;
  }

  private requireClient(): BridgeClient {
    if (!this.client) {
      throw new Error(`bridge client not connected (state=${this.state.status})`);
    }
    return this.client;
  }

  // ── Renderer fan-out ─────────────────────────────────────────────────────────

  private broadcast(channel: string, payload: unknown): void {
    for (const wc of this.targets) {
      if (wc.isDestroyed()) {
        this.targets.delete(wc);
        continue;
      }
      wc.send(channel, payload);
    }
  }

  private setState(next: ConnectionState): void {
    this.state = next;
    this.broadcast(CH_STATE_CHANGED, next);
  }

  // ── Teardown ─────────────────────────────────────────────────────────────────

  private teardownChildAndClient(): void {
    if (this.client) {
      try {
        this.client.removeAllListeners();
        this.client.close();
      } catch {
        // closing a half-open socket can throw — ignore on teardown.
      }
      this.client = null;
    }
    if (this.child) {
      this.child.removeAllListeners();
      try {
        this.child.kill();
      } catch {
        // already-dead child — ignore.
      }
      this.child = null;
    }
  }

  /**
   * Kill the child, close the socket, and remove every IPC handler + listener.
   * Idempotent — safe to call on both `window-all-closed` and `will-quit`.
   */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.teardownChildAndClient();
    this.unregisterIpc();
    this.targets.clear();
  }
}
