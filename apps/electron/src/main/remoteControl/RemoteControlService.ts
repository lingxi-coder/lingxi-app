/**
 * Remote Control on the desktop host: owns the presence record and inbox
 * stream, the viewer connections, the hidden WebRTC window and the approved
 * device list, and runs only while it should: an account is signed in, the
 * local `remoteControlEnabled` setting is on (or the unpackaged
 * `LINGXI_REMOTE_CONTROL=1` switch is set) and the managed policy allows it.
 * Sign-in changes, the setting and the policy are all folded into one
 * reconciliation, so starting, stopping and switching accounts are the same
 * step however they were triggered.
 */

import { CLIENT_PROTOCOL_MAJOR, systemClock, type Clock, type RemoteHostInfo, type Rtdb } from '@lingxi/bridge-client/remote';

import type { ApprovalDialog } from './approvalDialog.js';
import type { RemoteControlConfig } from './config.js';
import { HostConnection, type RemoteConnectionStatus } from './connection.js';
import type { DeviceRegistry, RegisteredDevice } from './deviceRegistry.js';
import { describeError, noDiagnostics, type RemoteDiagnostics } from './diagnostics.js';
import type { RemoteSessionGateway } from './gateway.js';
import type { HostIdentity } from './hostIdentity.js';
import type { PeerHost } from './peerHost.js';
import { HostSignaling, type HostInfo, type SignalingState } from './signalingHost.js';

export type { RemoteConnectionStatus } from './connection.js';

export type RemoteControlState = 'off' | 'starting' | 'online' | 'error';

export interface RemoteControlStatus {
  state: RemoteControlState;
  /** This computer's id, once the host key is loaded. */
  host_id?: string;
  /** What went wrong, when `state` is `error`. */
  error?: string;
  /** Connections that were accepted and have a transport; a pending one shows `approved: false`. */
  connections: RemoteConnectionStatus[];
}

/** The slice of `AccountService` the host follows. */
export interface RemoteControlAccount {
  snapshot(): { user: { uid: string } | null };
  onChanged(callback: () => void): () => void;
  /** The Firebase ID token; `forceRefresh` bypasses the cached one. */
  getIdToken(forceRefresh?: boolean): Promise<string>;
  /** Resolves once a saved session has been restored, so that `snapshot()` is final. */
  ready?(): Promise<void>;
}

/** The slice of `SettingsStore` the host uses. */
export interface RemoteControlSettings {
  getRemoteControlEnabled(): boolean;
  setRemoteControlEnabled(enabled: boolean): void;
}

export interface RemoteControlServiceOptions {
  account: RemoteControlAccount;
  settings: RemoteControlSettings;
  /** Managed-policy veto; false stops the service and refuses offers. Call {@link RemoteControlService.refresh} when it changes. */
  policyAllows: () => boolean;
  gateway: RemoteSessionGateway;
  config: Pick<RemoteControlConfig, 'forceTunnel' | 'forceEnabled' | 'stunUrls'>;
  /** Loads or creates this computer's host key. Called once, when the service first starts. */
  identity: () => Promise<HostIdentity>;
  /** The Realtime Database for the signed-in account; `getIdToken` is the service's own provider. */
  createRtdb: (getIdToken: (forceRefresh?: boolean) => Promise<string>) => Rtdb;
  /** Shared backend identity across service instances; defaults to this service. */
  publicationDomain?: object | string;
  registry: DeviceRegistry;
  approval: Pick<ApprovalDialog, 'request' | 'dispose'>;
  peers: Pick<PeerHost, 'answer' | 'link' | 'shutdown' | 'dispose'>;
  /** What viewers are told about this computer; read at every start. */
  host: () => Pick<HostInfo, 'name' | 'platform' | 'app_version'>;
  clock?: Clock;
  diagnostics?: RemoteDiagnostics;
}

// Retrying a start that failed (typically: offline), doubling up to a minute.
const RETRY_MIN_MS = 2_000;
const RETRY_MAX_MS = 60_000;
// `dispose()` returns after this long whatever is still shutting down.
const DISPOSE_BUDGET_MS = 2_000;

interface Run {
  uid: string;
  identity: HostIdentity;
  signaling: HostSignaling;
}

export class RemoteControlService {
  private readonly account: RemoteControlAccount;
  private readonly clock: Clock;
  private readonly diagnostics: RemoteDiagnostics;
  private readonly connections = new Map<string, HostConnection>();
  private readonly listeners = new Set<(status: RemoteControlStatus) => void>();
  private readonly unsubscribe: () => void;
  private identity: Promise<HostIdentity> | undefined;
  private lastToken: string | undefined;
  private run: Run | undefined;
  private state: RemoteControlState = 'off';
  private error: string | undefined;
  private lastEmitted = '';
  private pass: Promise<void> = Promise.resolve();
  private retryTimer: unknown;
  private retryDelay = RETRY_MIN_MS;
  private started = false;
  private disposed = false;
  private disposing: Promise<void> | undefined;

  constructor(private readonly options: RemoteControlServiceOptions) {
    this.account = options.account;
    this.clock = options.clock ?? systemClock;
    this.diagnostics = options.diagnostics ?? noDiagnostics;
    this.unsubscribe = this.account.onChanged(() => { if (this.started) void this.reconcile(); });
  }

  /** Begin following the account and the setting. Resolves once the first decision has been carried out. */
  async start(): Promise<void> {
    if (this.started || this.disposed) return;
    this.started = true;
    try { await this.account.ready?.(); } catch { /* The snapshot then says signed out. */ }
    await this.reconcile();
  }

  status(): RemoteControlStatus {
    const status: RemoteControlStatus = {
      state: this.state,
      connections: [...this.connections.values()].flatMap((connection) => connection.status() ?? []),
    };
    if (this.run !== undefined) status.host_id = this.run.identity.hostId;
    if (this.state === 'error' && this.error !== undefined) status.error = this.error;
    return status;
  }

  /** Be told whenever the status changes; returns the unsubscribe function. */
  onStatus(callback: (status: RemoteControlStatus) => void): () => void {
    this.listeners.add(callback);
    return () => { this.listeners.delete(callback); };
  }

  /** Persist the setting and apply it. Resolves once the service has started or stopped accordingly. */
  async setEnabled(enabled: boolean): Promise<void> {
    this.options.settings.setRemoteControlEnabled(enabled);
    await this.reconcile();
  }

  /** The approved devices of the signed-in account. */
  listDevices(): RegisteredDevice[] {
    const uid = this.account.snapshot().user?.uid;
    return uid === undefined ? [] : this.options.registry.list(uid);
  }

  /** Forget a device and end its connections. Throws if the revocation could not be saved (it still holds until restart). */
  async revokeDevice(deviceId: string): Promise<void> {
    const uid = this.account.snapshot().user?.uid;
    if (uid === undefined) return;
    let failure: unknown;
    try {
      this.options.registry.revoke(uid, deviceId);
    } catch (error) {
      failure = error;
    }
    await Promise.all([...this.connections.values()]
      .filter((connection) => connection.deviceId === deviceId)
      .map((connection) => connection.disconnect('revoked', 'revoked')));
    if (failure !== undefined) throw failure;
  }

  /** After a wake from sleep or a network change: retry now, and reopen the stream. */
  reconnect(): void {
    if (this.disposed) return;
    this.retryDelay = RETRY_MIN_MS;
    this.clock.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    if (this.run !== undefined) this.run.signaling.reconnect();
    else void this.reconcile();
  }

  /** Re-evaluate whether the service should run, e.g. after the policy changed. */
  refresh(): void {
    if (this.started && !this.disposed) void this.reconcile();
  }

  /** Stop everything; returns within about two seconds whatever is still shutting down. */
  dispose(): Promise<void> {
    if (this.disposing !== undefined) return this.disposing;
    this.disposed = true;
    this.unsubscribe();
    this.clock.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.options.approval.dispose();
    const shutdown = this.reconcile().finally(() => this.options.peers.dispose());
    let timer: unknown;
    const budget = new Promise<void>((resolve) => { timer = this.clock.setTimeout(resolve, DISPOSE_BUDGET_MS); });
    this.disposing = Promise.race([shutdown, budget]).finally(() => this.clock.clearTimeout(timer));
    return this.disposing;
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Starting and stopping
  // ───────────────────────────────────────────────────────────────────────────

  /** The account to run for, or `undefined` when the service should be off. */
  private wantedUid(): string | undefined {
    if (this.disposed) return undefined;
    const uid = this.account.snapshot().user?.uid;
    if (uid === undefined) return undefined;
    const { settings, config, policyAllows } = this.options;
    if (!settings.getRemoteControlEnabled() && !config.forceEnabled) return undefined;
    return policyAllows() ? uid : undefined;
  }

  /** Bring the running state in line with what is wanted. Passes are queued, so they never overlap. */
  private reconcile(): Promise<void> {
    this.pass = this.pass.then(() => this.apply()).catch((error: unknown) => {
      this.diagnostics('error', `remote control failed to reconcile (${describeError(error)})`);
    });
    return this.pass;
  }

  private async apply(): Promise<void> {
    const wanted = this.wantedUid();
    if (this.run !== undefined && this.run.uid === wanted) return;
    // A failed start is already waiting for its retry; account events in between do not hurry it.
    if (this.run === undefined && wanted !== undefined && this.retryTimer !== undefined) return;
    if (this.run !== undefined) await this.stopRun();
    if (wanted === undefined) {
      this.clock.clearTimeout(this.retryTimer);
      this.retryTimer = undefined;
      this.setState('off');
      return;
    }
    await this.startRun(wanted);
  }

  private async startRun(uid: string): Promise<void> {
    this.setState('starting');
    let identity: HostIdentity;
    try {
      identity = await (this.identity ??= this.options.identity());
    } catch (error) {
      this.identity = undefined;
      this.fail(`the host key could not be loaded (${describeError(error)})`);
      this.scheduleRetry();
      return;
    }
    if (this.disposed) return;
    const rtdb = this.options.createRtdb((forceRefresh) => this.idToken(forceRefresh));
    const info: HostInfo = { ...this.options.host(), client_protocol_version: CLIENT_PROTOCOL_MAJOR };
    const signaling: HostSignaling = new HostSignaling({
      rtdb,
      publicationDomain: this.options.publicationDomain ?? this,
      uid,
      identity,
      info,
      clock: this.clock,
      diagnostics: this.diagnostics,
      onOffer: (connId, record) => this.accept(run, rtdb, connId, record),
      onFrame: (connId, seq, value) => this.connections.get(connId)?.deliverFrame(seq, value),
      onState: (state, error) => this.signalingState(run, state, error),
    });
    const run: Run = { uid, identity, signaling };
    this.run = run;
    // Not awaited: stopping must stay possible while a slow start is still talking to the database.
    signaling.start().then(
      () => { this.retryDelay = RETRY_MIN_MS; },
      (error: unknown) => {
        if (this.run !== run) return;
        this.run = undefined;
        this.fail(`could not reach the Realtime Database (${describeError(error)})`);
        this.scheduleRetry();
      },
    );
    this.emit();
  }

  private async stopRun(): Promise<void> {
    const run = this.run;
    if (run === undefined) return;
    this.run = undefined;
    for (const connection of [...this.connections.values()]) connection.close('service_stopped');
    this.connections.clear();
    const stopped = run.signaling.stop();
    this.options.peers.shutdown();
    await stopped;
    this.lastToken = undefined;
  }

  private fail(message: string): void {
    this.diagnostics('error', message);
    this.setState('error', message);
  }

  private scheduleRetry(): void {
    this.clock.clearTimeout(this.retryTimer);
    const delay = this.retryDelay;
    this.retryDelay = Math.min(RETRY_MAX_MS, delay * 2);
    this.retryTimer = this.clock.setTimeout(() => {
      this.retryTimer = undefined;
      void this.reconcile();
    }, delay);
  }

  private signalingState(run: Run, state: SignalingState, error: string | undefined): void {
    if (this.run !== run) return;
    if (state === 'online') this.setState('online');
    else if (state === 'error') this.setState('error', error);
    else if (state === 'starting') this.setState('starting');
  }

  /** The account's ID token. While stopping after a sign-out the account has none left; the last one lets the goodbye through. */
  private async idToken(forceRefresh?: boolean): Promise<string> {
    try {
      const token = await this.account.getIdToken(forceRefresh);
      this.lastToken = token;
      return token;
    } catch (error) {
      if (this.run === undefined && this.lastToken !== undefined) return this.lastToken;
      throw error;
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Connections
  // ───────────────────────────────────────────────────────────────────────────

  private accept(run: Run, rtdb: Rtdb, connId: string, record: unknown): void {
    if (this.run !== run || this.connections.has(connId)) return;
    const connection: HostConnection = new HostConnection({
      uid: run.uid,
      identity: run.identity,
      hostInfo: this.hostInfo(),
      connId,
      rtdb,
      signaling: run.signaling,
      peers: this.options.peers,
      registry: this.options.registry,
      approval: this.options.approval,
      gateway: this.options.gateway,
      config: this.options.config,
      enabled: () => this.wantedUid() === run.uid,
      acceptedCount: () => [...this.connections.values()].filter((other) => other !== connection && other.isAccepted).length,
      clock: this.clock,
      diagnostics: this.diagnostics,
      onChange: () => this.emit(),
      onClosed: (closed) => { if (this.connections.get(closed.connId) === closed) this.connections.delete(closed.connId); },
    });
    this.connections.set(connId, connection);
    connection.handleOffer(record);
  }

  private hostInfo(): RemoteHostInfo {
    const { name, platform, app_version } = this.options.host();
    return { name, platform, app_version };
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Status
  // ───────────────────────────────────────────────────────────────────────────

  private setState(state: RemoteControlState, error?: string): void {
    this.state = state;
    this.error = error;
    this.emit();
  }

  private emit(): void {
    const status = this.status();
    const serialized = JSON.stringify(status);
    if (serialized === this.lastEmitted) return;
    this.lastEmitted = serialized;
    for (const listener of [...this.listeners]) {
      try { listener(status); } catch { /* One listener cannot break the others. */ }
    }
  }
}
