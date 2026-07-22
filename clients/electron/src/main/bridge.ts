import { spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { IpcMainInvokeEvent, WebContents } from 'electron';
import {
  BridgeClient,
  type ClientEvent,
  type PermissionRequest,
} from '@lingxi/bridge-client';

import { buildBridgeArguments, buildBridgeEnvironment, buildCredentialEnvelope, diagnosticEvent, DiagnosticBuffer, sanitizeDiagnostic } from './host-utils.js';
import {
  assertCommandAllowedDuringTurn,
  validateClientCommand,
  validateBridgeLockfile,
  validateOptionalTurnId,
  validatePermissionResponse,
  validatePrompt,
  validateRequestId,
} from './validation.js';

export const CH_SEND_PROMPT = 'lingxi:sendPrompt';
export const CH_APPROVE = 'lingxi:approve';
export const CH_DENY = 'lingxi:deny';
export const CH_CANCEL = 'lingxi:cancel';
export const CH_COMMAND = 'lingxi:command';
export const CH_CONNECTION_STATE = 'lingxi:connectionState';
export const CH_EVENT = 'lingxi:event';
export const CH_PERMISSION = 'lingxi:permission';
export const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';

export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'restarting' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

export interface BridgeLaunchConfig {
  workspace: string;
  apiKey?: string;
  providerCredentials?: Record<string, string>;
  trusted: boolean;
  model?: string;
  apiBaseUrl?: string;
}

export interface BridgeRuntimeVersions {
  serverName: string;
  serverProtocol: string;
  clientProtocol: string;
}

export interface BridgeManagerOptions {
  /** Development-only binary override. Ignored in packaged builds. */
  serverBin?: string;
  /** Parent for owner-only, per-launch discovery directories. */
  bridgeRoot?: string;
  lockfileTimeoutMs?: number;
  stopTimeoutMs?: number;
  isPackaged?: boolean;
  resourcesPath?: string;
  launchConfig: () => BridgeLaunchConfig | Promise<BridgeLaunchConfig>;
  /** Synchronous trust snapshot used by privileged IPC checks. */
  accessState?: () => { workspace?: string; trusted: boolean };
  diagnostics?: DiagnosticBuffer;
  onModelChanged?: (model: string) => void;
}

const SERVER_BIN_NAME = process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
const MAX_PENDING_PERMISSIONS = 1_000;
const require = createRequire(import.meta.url);
const electronModule = require('electron');
const ipcMain = (typeof electronModule === 'string' ? undefined : electronModule.ipcMain) ?? {
  handle: () => { throw new Error('ipcMain is unavailable outside Electron'); },
  removeHandler: () => undefined,
};

function moduleDir(): string {
  try {
    return dirname(fileURLToPath(import.meta.url));
  } catch {
    return process.cwd();
  }
}

function findWorkspaceServerBin(start: string): string | undefined {
  let dir = start;
  for (;;) {
    for (const profile of ['debug', 'release']) {
      const candidate = join(dir, 'lingxi-code', 'target', profile, SERVER_BIN_NAME);
      if (existsSync(candidate)) return candidate;
    }
    const parent = dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** Packaged builds use only process.resourcesPath; overrides/search are development-only. */
export function resolveServerBin(opts: Omit<BridgeManagerOptions, 'launchConfig'> = {}): string {
  if (opts.isPackaged) {
    const resourcesPath = opts.resourcesPath ?? process.resourcesPath;
    const packagedBinary = join(resourcesPath, 'bin', SERVER_BIN_NAME);
    if (existsSync(packagedBinary)) return packagedBinary;
    throw new Error(`packaged bridge-server is missing from application resources`);
  }
  const explicit = opts.serverBin ?? process.env['LINGXI_BRIDGE_SERVER_BIN'];
  if (explicit) return explicit;
  const found = findWorkspaceServerBin(moduleDir());
  if (found) return found;
  throw new Error('bridge-server binary not found; build it or set LINGXI_BRIDGE_SERVER_BIN in development');
}

function lockfiles(dir: string): string[] {
  try {
    return readdirSync(dir).filter((name) => name.endsWith('.lock') && !name.startsWith('.'));
  } catch {
    return [];
  }
}

function urlOrigin(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'file:' ? 'file://' : url.origin;
  } catch {
    return undefined;
  }
}

function connectionDiagnostic(state: ConnectionState, generation: number): string {
  return diagnosticEvent('connection_state', { generation, state });
}

function childExitDiagnostic(code: number | null, signal: NodeJS.Signals | null, generation: number): string {
  return diagnosticEvent('child_exit', {
    clean: signal === null && code === 0,
    code,
    generation,
    signal,
  });
}

function bridgeVersionDiagnostic(server: string, serverProtocol: string, clientProtocol: string): string {
  return diagnosticEvent('bridge_handshake', {
    clientProtocol,
    server,
    serverProtocol,
  });
}

function ignorableLockfileError(input: unknown): boolean {
  if (!(input instanceof Error)) return false;
  return (input as NodeJS.ErrnoException).code === 'ENOENT'
    || input instanceof SyntaxError
    || /bridge lockfile|JSON/.test(input.message);
}

export class BridgeManager {
  private child: ChildProcess | null = null;
  private client: BridgeClient | null = null;
  private state: ConnectionState = { status: 'idle' };
  private disposed = false;
  private ipcRegistered = false;
  private restartChain: Promise<void> = Promise.resolve();
  private generation = 0;
  private launchDir: string | null = null;
  private activeWorkspace: string | undefined;
  private activeWorkspaceTrusted = false;
  private activeCredentialAvailable = false;
  private activeTurn = false;
  private lastRuntimeVersions: BridgeRuntimeVersions | undefined;
  private readonly pendingPermissionIds = new Set<number>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly diagnostics: DiagnosticBuffer;

  constructor(private readonly opts: BridgeManagerOptions) {
    this.diagnostics = opts.diagnostics ?? new DiagnosticBuffer();
  }

  get connectionState(): ConnectionState {
    return this.state;
  }

  get turnActive(): boolean {
    return this.activeTurn;
  }

  get runtimeVersions(): BridgeRuntimeVersions | undefined {
    return this.lastRuntimeVersions ? { ...this.lastRuntimeVersions } : undefined;
  }

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const origin = urlOrigin(rendererUrl);
    if (!origin) throw new Error('invalid renderer URL');
    const origins = this.targets.get(webContents) ?? new Set<string>();
    origins.add(origin);
    this.targets.set(webContents, origins);
    webContents.once('destroyed', () => this.targets.delete(webContents));
  }

  async start(): Promise<void> {
    if (this.disposed) throw new Error('BridgeManager is disposed');
    if (this.child || this.client) throw new Error('BridgeManager already started');
    this.registerIpc();
    try {
      await this.startInternal();
    } catch (error) {
      // launchConfig runs before the child lifecycle begins. Surface failures
      // such as an unreadable Keychain credential through the same renderer
      // connection state as spawn/protocol failures.
      if (this.state.status !== 'error') this.fail(error);
      throw error;
    }
  }

  restart(): Promise<void> {
    this.registerIpc();
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) return;
      this.setState({ status: 'restarting' });
      await this.stopBridge();
      await this.startInternal();
    });
    return this.restartChain;
  }

  private async startInternal(): Promise<void> {
    const launch = await this.opts.launchConfig();
    this.activeWorkspace = launch.workspace;
    this.activeWorkspaceTrusted = launch.trusted;
    this.activeCredentialAvailable = Boolean(launch.apiKey) || Object.keys(launch.providerCredentials ?? {}).length > 0;
    const bridgeDir = this.createLaunchDirectory();
    const generation = ++this.generation;

    this.setState({ status: 'spawning' });
    let child: ChildProcess;
    try {
      child = this.spawnServer(launch, bridgeDir);
    } catch (error) {
      this.fail(error);
      this.removeLaunchDirectory();
      throw error;
    }
    this.child = child;
    this.captureLogs(child, [launch.apiKey, ...Object.values(launch.providerCredentials ?? {})].filter((value): value is string => Boolean(value)));

    child.once('exit', (code, signal) => {
      this.diagnostics.add('info', 'host', childExitDiagnostic(code, signal, generation));
      if (generation !== this.generation) return;
      this.child = null;
      if (!this.disposed) {
        this.setState({ status: 'disconnected', reason: `bridge-server exited (code=${code ?? 'null'}, signal=${signal ?? 'null'})` });
      }
    });
    child.once('error', (error) => {
      if (generation === this.generation && !this.disposed) this.fail(`failed to spawn bridge-server: ${error.message}`);
    });

    try {
      const lockfilePath = await this.waitForLockfile(bridgeDir, launch, generation);
      this.setState({ status: 'connecting' });
      const client = new BridgeClient({ lockfilePath, clientName: 'lingxi-electron/0.1.0' });
      this.client = client;
      this.wireClient(client, generation);
      const hello = await client.connect();
      if (generation !== this.generation) return;
      this.lastRuntimeVersions = {
        serverName: hello.server_name,
        serverProtocol: hello.protocol_version,
        clientProtocol: hello.capabilities.client_protocol_version,
      };
      this.diagnostics.add(
        'info',
        'bridge',
        bridgeVersionDiagnostic(hello.server_name, hello.protocol_version, hello.capabilities.client_protocol_version),
      );
      this.setState({ status: 'connected' });
    } catch (error) {
      if (generation === this.generation) {
        this.fail(error);
        await this.stopBridge();
      }
      throw error;
    }
  }

  private createLaunchDirectory(): string {
    const root = this.opts.bridgeRoot ?? join(tmpdir(), 'lingxi-electron-bridge');
    mkdirSync(root, { recursive: true, mode: 0o700 });
    chmodSync(root, 0o700);
    const directory = mkdtempSync(join(root, 'launch-'));
    chmodSync(directory, 0o700);
    this.launchDir = directory;
    return directory;
  }

  private spawnServer(launch: BridgeLaunchConfig, bridgeDir: string): ChildProcess {
    const bin = resolveServerBin(this.opts);
    const args = buildBridgeArguments({
      workspace: launch.workspace,
      bridgeDir,
      model: launch.model,
      hasApiKey: Boolean(launch.apiKey),
      hasCredentialStdin: Object.keys(launch.providerCredentials ?? {}).length > 0,
      trusted: launch.trusted,
      packagedCredentialBoundary: Boolean(this.opts.isPackaged),
    });

    const child = spawn(bin, args, {
      cwd: launch.workspace,
      env: buildBridgeEnvironment(process.env, launch.apiBaseUrl),
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      detached: process.platform !== 'win32',
    });
    const providerCredentials = launch.providerCredentials ?? {};
    if (Object.keys(providerCredentials).length > 0) {
      child.stdin?.end(buildCredentialEnvelope(launch));
    } else if (launch.apiKey) child.stdin?.end(`${launch.apiKey}\n`);
    else child.stdin?.end();
    return child;
  }

  private captureLogs(child: ChildProcess, secrets: readonly string[] = []): void {
    const attach = (stream: NodeJS.ReadableStream | null, level: 'info' | 'error'): void => {
      if (!stream) return;
      let buffered = '';
      const flush = (): void => {
        const text = sanitizeDiagnostic(buffered, secrets);
        buffered = '';
        if (text) this.diagnostics.add(level, 'bridge', text, secrets);
      };
      stream.on('data', (chunk: Buffer | string) => {
        buffered += chunk.toString();
        const lines = buffered.split(/\r?\n/);
        buffered = lines.pop() ?? '';
        for (const line of lines) {
          const text = sanitizeDiagnostic(line, secrets);
          if (text) this.diagnostics.add(level, 'bridge', text, secrets);
        }
        if (buffered.length > 8_000) flush();
      });
      stream.on('end', flush);
    };
    attach(child.stdout, 'info');
    attach(child.stderr, 'error');
  }

  private waitForLockfile(dir: string, launch: BridgeLaunchConfig, generation: number): Promise<string> {
    const timeoutMs = this.opts.lockfileTimeoutMs ?? 15_000;
    const deadline = Date.now() + timeoutMs;
    return new Promise<string>((resolve, reject) => {
      const tick = (): void => {
        if (this.disposed || generation !== this.generation) return reject(new Error('bridge launch superseded'));
        if (!this.child) return reject(new Error('bridge-server exited before publishing a lockfile'));
        for (const name of lockfiles(dir).sort((left, right) => left.localeCompare(right))) {
          const path = join(dir, name);
          try {
            const metadata = lstatSync(path);
            if (!metadata.isFile()) throw new Error('bridge lockfile is not a regular file');
            if (metadata.size > 64 * 1024) throw new Error('bridge lockfile is too large');
            if (process.platform !== 'win32' && (metadata.mode & 0o077) !== 0) {
              throw new Error('bridge lockfile permissions are not owner-only');
            }
            if (process.getuid && metadata.uid !== process.getuid()) throw new Error('bridge lockfile owner mismatch');
            validateBridgeLockfile(JSON.parse(readFileSync(path, 'utf8')), this.child.pid!, launch.workspace);
            return resolve(path);
          } catch (error) {
            const failure = error instanceof Error ? error : new Error(String(error));
            if (!ignorableLockfileError(failure)) return reject(failure);
            this.diagnostics.add('warn', 'host', diagnosticEvent('lockfile_ignored', { file: path, reason: failure.message }));
          }
        }
        if (Date.now() >= deadline) return reject(new Error(`timed out waiting for bridge lockfile`));
        setTimeout(tick, 50);
      };
      tick();
    });
  }

  private wireClient(client: BridgeClient, generation: number): void {
    client.on('event', (event: ClientEvent) => {
      if (event.type === 'turn_started') this.activeTurn = true;
      if (event.type === 'turn_ended' || event.type === 'session_ended' || event.type === 'error') {
        this.activeTurn = false;
      }
      if (event.type === 'model_changed') {
        try { this.opts.onModelChanged?.(event.model); }
        catch (error) { this.diagnostics.add('warn', 'host', error); }
      }
      this.broadcast(CH_EVENT, event);
    });
    client.on('permission', (request: PermissionRequest) => {
      if (Number.isSafeInteger(request.request_id) && request.request_id >= 0) {
        if (!this.pendingPermissionIds.has(request.request_id) && this.pendingPermissionIds.size >= MAX_PENDING_PERMISSIONS) {
          this.diagnostics.add('warn', 'bridge', 'permission request limit reached');
          return;
        }
        this.pendingPermissionIds.add(request.request_id);
        this.broadcast(CH_PERMISSION, request);
      }
    });
    client.on('close', (code, reason) => {
      if (generation === this.generation && !this.disposed) this.setState({ status: 'disconnected', reason: reason || `ws closed (code=${code})` });
    });
    client.on('error', (error) => {
      if (generation === this.generation && !this.disposed) this.fail(error);
    });
  }

  private assertSender(event: IpcMainInvokeEvent): void {
    const origins = this.targets.get(event.sender);
    const senderFrame = event.senderFrame;
    if (!origins || !senderFrame || senderFrame !== event.sender.mainFrame) throw new Error('unauthorized IPC sender');
    const origin = urlOrigin(senderFrame.url);
    if (!origin || !origins.has(origin)) throw new Error('unauthorized IPC origin');
  }

  private registerIpc(): void {
    if (this.ipcRegistered) return;
    this.ipcRegistered = true;
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, text: unknown) => {
      this.assertSender(event);
      this.requirePromptClient().sendPrompt(validatePrompt(text));
    });
    ipcMain.handle(CH_APPROVE, (event: IpcMainInvokeEvent, requestId: unknown, response: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      const permissionResponse = validatePermissionResponse(response);
      if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
      this.requireClient().approvePermission(id, permissionResponse);
      this.pendingPermissionIds.delete(id);
    });
    ipcMain.handle(CH_DENY, (event: IpcMainInvokeEvent, requestId: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
      this.requireClient().denyPermission(id);
      this.pendingPermissionIds.delete(id);
    });
    ipcMain.handle(CH_CANCEL, (event: IpcMainInvokeEvent, turnId: unknown) => {
      this.assertSender(event);
      this.requireClient().cancel(validateOptionalTurnId(turnId));
    });
    ipcMain.handle(CH_COMMAND, (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      const validated = validateClientCommand(command, this.activeWorkspace);
      assertCommandAllowedDuringTurn(validated, this.activeTurn);
      this.requireClient().sendCommand(validated);
    });
    ipcMain.handle(CH_CONNECTION_STATE, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.state;
    });
  }

  private unregisterIpc(): void {
    if (!this.ipcRegistered) return;
    for (const channel of [CH_SEND_PROMPT, CH_APPROVE, CH_DENY, CH_CANCEL, CH_COMMAND, CH_CONNECTION_STATE]) {
      ipcMain.removeHandler(channel);
    }
    this.ipcRegistered = false;
  }

  private requireClient(): BridgeClient {
    this.refreshAccessState();
    if (!this.activeWorkspaceTrusted) throw new Error('workspace trust is required before using the engine');
    if (!this.client) throw new Error(`bridge client not connected (state=${this.state.status})`);
    return this.client;
  }

  private requirePromptClient(): BridgeClient {
    const client = this.requireClient();
    this.refreshAccessState();
    if (!this.activeCredentialAvailable) throw new Error('provider credential is required before sending a prompt');
    return client;
  }

  private refreshAccessState(): void {
    const workspace = this.activeWorkspace;
    if (!workspace) {
      this.activeWorkspaceTrusted = false;
      this.activeCredentialAvailable = false;
      return;
    }
    if (this.opts.accessState) {
      const snapshot = this.opts.accessState();
      const matchesWorkspace = snapshot.workspace === workspace;
      this.activeWorkspaceTrusted = matchesWorkspace && snapshot.trusted;
      if (!matchesWorkspace) this.activeCredentialAvailable = false;
      return;
    }
    const launch = this.opts.launchConfig();
    if (launch instanceof Promise) return;
    const matchesWorkspace = launch.workspace === workspace;
    this.activeWorkspaceTrusted = matchesWorkspace && launch.trusted;
    this.activeCredentialAvailable = matchesWorkspace
      && (Boolean(launch.apiKey) || Object.keys(launch.providerCredentials ?? {}).length > 0);
  }

  private broadcast(channel: string, payload: unknown): void {
    for (const webContents of this.targets.keys()) {
      if (webContents.isDestroyed()) this.targets.delete(webContents);
      else webContents.send(channel, payload);
    }
  }

  private setState(next: ConnectionState): void {
    this.state = next;
    this.diagnostics.add('info', 'host', connectionDiagnostic(next, this.generation));
    this.broadcast(CH_STATE_CHANGED, next);
  }

  private fail(error: unknown): void {
    const message = sanitizeDiagnostic(error);
    this.diagnostics.add('error', 'host', message);
    this.setState({ status: 'error', message });
  }

  private async stopBridge(): Promise<void> {
    ++this.generation;
    this.pendingPermissionIds.clear();
    this.activeWorkspace = undefined;
    this.activeWorkspaceTrusted = false;
    this.activeCredentialAvailable = false;
    this.activeTurn = false;
    const client = this.client;
    this.client = null;
    if (client) {
      try {
        client.removeAllListeners();
        client.close();
      } catch {
        // A half-open socket may throw while closing.
      }
    }

    const child = this.child;
    this.child = null;
    if (child && child.exitCode === null && child.signalCode === null) await new Promise<void>((resolve) => {
      let settled = false;
      let timer: NodeJS.Timeout | undefined;
      const finish = (): void => {
        if (settled) return;
        settled = true;
        if (timer) clearTimeout(timer);
        resolve();
      };
      child.once('exit', finish);
      try { this.signalChildTree(child, 'SIGINT'); } catch { finish(); return; }
      timer = setTimeout(() => {
        try { this.signalChildTree(child, 'SIGKILL'); } catch { /* already gone */ }
        finish();
      }, this.opts.stopTimeoutMs ?? 2_000);
      timer.unref();
    });
    this.removeLaunchDirectory();
  }

  private signalChildTree(child: ChildProcess, signal: NodeJS.Signals): void {
    if (process.platform !== 'win32' && child.pid) process.kill(-child.pid, signal);
    else child.kill(signal);
  }

  private removeLaunchDirectory(): void {
    const launchDir = this.launchDir;
    this.launchDir = null;
    if (launchDir) {
      try { rmSync(launchDir, { recursive: true, force: true }); } catch (error) { this.diagnostics.add('warn', 'host', error); }
    }
  }

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    await this.restartChain.catch(() => undefined);
    await this.stopBridge();
    this.unregisterIpc();
    this.targets.clear();
  }
}
