import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

import type { AudioOpDto, AudioResultDto } from '@lingxi/bridge-client';

import type { DiagnosticBuffer } from '../host-utils.js';
import type { MicrophonePermissionStatus } from '../../shared/microphoneAccess.js';
import { offlineVoiceModelById } from '../../shared/voiceModelCatalog.js';
import {
  defaultNativeAudioSnapshot,
  nativeAudioResponseToEngineResult,
  validateNativeAudioCommand,
  validateNativeAudioEngineRequest,
  validateNativeAudioEngineResponse,
  validateNativeAudioEvent,
  validateNativeAudioResponse,
  type NativeAudioCommand,
  type NativeAudioEngineResponse,
  type NativeAudioErrorCode,
  type NativeAudioEvent,
  type NativeAudioHelperCommandEnvelope,
  type NativeAudioHelperEnvelope,
  type NativeAudioOwner,
  type NativeAudioResponse,
  type NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';

const AUDIO_HELPER_APP = 'LingXiAudioHelper.app';
const AUDIO_HELPER_BIN = 'LingXiAudioHelper';
const AUDIO_HELPER_ENV = 'LINGXI_AUDIO_HELPER_BIN';
const REQUEST_TIMEOUT_MS = 5 * 60_000;

type HelperProcess = Pick<ChildProcessWithoutNullStreams, 'pid' | 'stdin' | 'stdout' | 'stderr' | 'kill' | 'once'>;
type SpawnHelper = (path: string, args: string[], env: NodeJS.ProcessEnv) => HelperProcess;
type AudioPermission = 'microphone' | 'speech';
type LaunchPermissionHelper = (appPath: string, permissions: AudioPermission[]) => Promise<void>;
type VerifyPackagedHelper = (appPath: string) => void;
type RequestMicrophoneAccess = () => Promise<MicrophonePermissionStatus>;

interface PendingCommandRequest {
  kind: 'command';
  owner: NativeAudioOwner | null;
  resolve: (response: NativeAudioResponse) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface PendingEngineRequest {
  kind: 'engine';
  owner: NativeAudioOwner;
  op: AudioOpDto;
  resolve: (response: AudioResultDto) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

type PendingRequest = PendingCommandRequest | PendingEngineRequest;

export interface NativeAudioManagerOptions {
  isPackaged: boolean;
  resourcesPath: string;
  userDataPath: string;
  diagnostics: DiagnosticBuffer;
  spawnHelper?: SpawnHelper;
  launchPermissionHelper?: LaunchPermissionHelper;
  verifyPackagedHelper?: VerifyPackagedHelper;
  requestMicrophoneAccess?: RequestMicrophoneAccess;
  helperPath?: string;
}

function launchPermissionHelper(appPath: string, permissions: AudioPermission[]): Promise<void> {
  return new Promise((resolveLaunch, rejectLaunch) => {
    const child = spawn('/usr/bin/open', [
      '-W',
      '-n',
      appPath,
      '--args',
      '--request-permissions',
      permissions.join(','),
    ], { stdio: 'ignore' });
    const timer = setTimeout(() => {
      child.kill();
      rejectLaunch(new Error('timed out waiting for the macOS audio permission prompt'));
    }, REQUEST_TIMEOUT_MS);
    child.once('error', (error) => {
      clearTimeout(timer);
      rejectLaunch(error);
    });
    child.once('exit', (code, signal) => {
      clearTimeout(timer);
      if (code === 0) {
        resolveLaunch();
        return;
      }
      rejectLaunch(new Error(`audio permission helper exited (code=${String(code)} signal=${String(signal)})`));
    });
  });
}

function cloneOwner(owner: NativeAudioOwner | null): NativeAudioOwner | null {
  return owner ? { ...owner } : null;
}

function cloneSnapshot(snapshot: NativeAudioSnapshot): NativeAudioSnapshot {
  return {
    helper: { ...snapshot.helper },
    permissions: { ...snapshot.permissions },
    owner: cloneOwner(snapshot.owner),
    activity: snapshot.activity,
    ...(snapshot.localeTag === undefined ? {} : { localeTag: snapshot.localeTag }),
    ...(snapshot.recognizerAvailable === undefined ? {} : { recognizerAvailable: snapshot.recognizerAvailable }),
    ...(snapshot.recognition ? { recognition: { ...snapshot.recognition } } : {}),
    ...(snapshot.playback ? { playback: { ...snapshot.playback } } : {}),
    voices: snapshot.voices.map((voice) => ({ ...voice })),
    models: snapshot.models.map((model) => ({ ...model })),
  };
}

function sameOwner(left: NativeAudioOwner | null, right: NativeAudioOwner | null): boolean {
  return Boolean(left && right && left.kind === right.kind && left.id === right.id);
}

function opClaimsOwner(op: AudioOpDto): boolean {
  return op.type === 'start_recording' || op.type === 'transcribe' || op.type === 'synthesize';
}

function opReleasesOwner(op: AudioOpDto, result: AudioResultDto): boolean {
  if (op.type === 'start_recording') return result.type === 'failed';
  return op.type === 'stop_recording' || op.type === 'transcribe' || op.type === 'synthesize';
}

function ownerFromCommand(command: NativeAudioCommand): NativeAudioOwner | null {
  switch (command.type) {
    case 'start_listening':
    case 'finish_listening':
    case 'cancel':
    case 'speak':
    case 'stop_speaking':
      return command.owner;
    default:
      return null;
  }
}

function commandClaimsOwner(command: NativeAudioCommand): boolean {
  return command.type === 'start_listening' || command.type === 'speak';
}

function commandReleasesOwner(command: NativeAudioCommand, response: NativeAudioResponse): boolean {
  if (command.type === 'cancel' || command.type === 'finish_listening' || command.type === 'stop_speaking') return true;
  if (response.type === 'error') return true;
  return false;
}

function commandActivity(command: NativeAudioCommand): NativeAudioSnapshot['activity'] {
  return command.type === 'speak' ? 'speaking' : 'listening';
}

function commandModelId(command: NativeAudioCommand): string | null {
  switch (command.type) {
    case 'install_model':
    case 'cancel_model':
    case 'remove_model':
      return command.modelId;
    default:
      return null;
  }
}

export function resolveNativeAudioHelperPath(opts: Pick<NativeAudioManagerOptions, 'isPackaged' | 'resourcesPath' | 'helperPath'>): string | null {
  if (opts.helperPath) return resolve(opts.helperPath);
  if (!opts.isPackaged) {
    const explicit = process.env[AUDIO_HELPER_ENV]?.trim();
    if (!explicit) return null;
    const resolved = resolve(explicit);
    return existsSync(resolved) ? resolved : null;
  }
  const packaged = join(opts.resourcesPath, AUDIO_HELPER_APP, 'Contents', 'MacOS', AUDIO_HELPER_BIN);
  return existsSync(packaged) ? packaged : null;
}

export class NativeAudioManager {
  private readonly storageRoot: string;
  private readonly spawnHelper: SpawnHelper;
  private readonly launchPermissionHelper: LaunchPermissionHelper;
  private readonly subscribers = new Set<(event: NativeAudioEvent) => void>();
  private readonly pending = new Map<string, PendingRequest>();
  private readonly helperPath: string | null;
  private helper: HelperProcess | null = null;
  private helperStartup: Promise<void> | null = null;
  private permissionResolution: Promise<NativeAudioSnapshot> | null = null;
  private helperStdoutBuffer = '';
  private snapshot = defaultNativeAudioSnapshot();
  private reservedOwner: NativeAudioOwner | null = null;

  constructor(private readonly opts: NativeAudioManagerOptions) {
    this.storageRoot = join(opts.userDataPath, 'voice-models');
    this.spawnHelper = opts.spawnHelper ?? ((path, args, env) => spawn(path, args, {
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
    }) as ChildProcessWithoutNullStreams);
    this.launchPermissionHelper = opts.launchPermissionHelper ?? launchPermissionHelper;
    this.helperPath = resolveNativeAudioHelperPath(opts);
    if (!this.helperPath) {
      this.snapshot = {
        ...this.snapshot,
        helper: {
          state: 'stopped',
          message: opts.isPackaged
            ? 'the packaged native audio helper is missing from application resources'
            : `set ${AUDIO_HELPER_ENV} to the built helper binary to enable native audio in development`,
        },
      };
    }
  }

  getSnapshot(): NativeAudioSnapshot {
    return cloneSnapshot(this.snapshot);
  }

  onEvent(callback: (event: NativeAudioEvent) => void): () => void {
    this.subscribers.add(callback);
    return () => this.subscribers.delete(callback);
  }

  async request(value: unknown): Promise<NativeAudioResponse> {
    const command = validateNativeAudioCommand(value);
    if (command.type === 'get_snapshot' && !this.helperPath) {
      return { type: 'snapshot', snapshot: this.getSnapshot() };
    }
    const modelId = commandModelId(command);
    if (modelId && !offlineVoiceModelById(modelId)) {
      return this.errorResponse('invalid-request', `unknown model id: ${modelId}`);
    }
    const owner = ownerFromCommand(command);
    if (owner && this.ownerBusy(owner)) {
      return this.errorResponse('busy', 'another audio operation is already active on this device');
    }
    if (!this.helperPath) return this.unavailableResponse();
    if (commandClaimsOwner(command) && owner) this.reservedOwner = { ...owner };
    try {
      await this.ensureHelper();
      if (command.type === 'request_authorization' && this.opts.isPackaged) {
        const snapshot = await this.resolvePackagedPermissions(command.permissions);
        return { type: 'authorization', snapshot };
      }
      if (command.type === 'start_listening' && this.opts.isPackaged) {
        await this.resolvePackagedPermissions([
          'microphone',
          ...(command.recognitionMode === 'automatic' ? ['speech' as const] : []),
        ]);
      }
      const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'command', command }, owner);
      const normalized = this.normalizeCommandResponse(command, response);
      return normalized;
    } catch (error) {
      this.opts.diagnostics.add('error', 'host', `native audio request failed: ${String(error)}`);
      return this.errorResponse('native-error', error instanceof Error ? error.message : String(error));
    } finally {
      if (owner && sameOwner(this.reservedOwner, owner)) this.reservedOwner = null;
    }
  }

  async executeEngineRequest(sessionId: unknown, opValue: unknown): Promise<AudioResultDto> {
    const { sessionId: validatedSessionId, op } = validateNativeAudioEngineRequest(sessionId, opValue);
    const owner = { kind: 'engine', id: validatedSessionId } satisfies NativeAudioOwner;
    if (op.type === 'is_recording' && this.snapshot.owner && !sameOwner(this.snapshot.owner, owner)) {
      return { type: 'recording_state', recording: false };
    }
    if (opClaimsOwner(op) && this.ownerBusy(owner)) {
      return { type: 'failed', kind: 'busy', message: 'another audio operation is already active on this device' };
    }
    if (!this.helperPath) {
      return { type: 'failed', kind: 'unavailable', message: this.snapshot.helper.message ?? 'native audio helper is unavailable' };
    }
    if (opClaimsOwner(op)) this.reservedOwner = { ...owner };
    try {
      await this.ensureHelper();
      if (op.type === 'start_recording' && this.opts.isPackaged) {
        await this.resolvePackagedPermissions(['microphone']);
      }
      const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'engine_request', owner, op }, owner);
      if (response.type === 'engine_result') {
        this.applySnapshot(response.snapshot);
        if (opReleasesOwner(op, response.result) && sameOwner(this.snapshot.owner, owner)) {
          this.releaseOwner();
        }
        return response.result;
      }
      const result = nativeAudioResponseToEngineResult(response);
      if (opReleasesOwner(op, result) && sameOwner(this.snapshot.owner, owner)) this.releaseOwner();
      return result;
    } catch (error) {
      this.opts.diagnostics.add('error', 'host', `native engine audio request failed: ${String(error)}`);
      return { type: 'failed', kind: 'other', message: error instanceof Error ? error.message : String(error) };
    } finally {
      if (sameOwner(this.reservedOwner, owner)) this.reservedOwner = null;
    }
  }

  async suspend(reason: string): Promise<void> {
    if (!this.helper) {
      this.releaseOwner();
      return;
    }
    const owner = this.snapshot.owner;
    if (!owner) return;
    const response = await this.request({ type: 'cancel', owner });
    if (response.type === 'error') {
      this.opts.diagnostics.add('warn', 'host', `native audio suspend (${reason}) failed: ${response.error.message}`);
      this.releaseOwner();
    }
  }

  async dispose(): Promise<void> {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new Error('native audio helper disposed'));
      this.pending.delete(id);
    }
    this.helper?.kill();
    this.helper = null;
    this.helperStartup = null;
    this.helperStdoutBuffer = '';
    this.releaseOwner();
    this.setHelperState('stopped', 'native audio helper stopped');
  }

  private unavailableResponse(): NativeAudioResponse {
    return this.errorResponse('unavailable', this.snapshot.helper.message ?? 'native audio helper is unavailable');
  }

  private errorResponse(code: NativeAudioErrorCode, message: string): NativeAudioResponse {
    return {
      type: 'error',
      snapshot: this.getSnapshot(),
      error: { code, message },
    };
  }

  private ownerBusy(owner: NativeAudioOwner): boolean {
    const active = this.snapshot.owner ?? this.reservedOwner;
    return Boolean(active && !sameOwner(active, owner));
  }

  private normalizeCommandResponse(command: NativeAudioCommand, response: NativeAudioResponse | NativeAudioEngineResponse): NativeAudioResponse {
    if (response.type === 'engine_result') {
      this.applySnapshot(response.snapshot);
      return response.result.type === 'failed'
        ? {
            type: 'error',
            snapshot: this.getSnapshot(),
            error: { code: 'native-error', message: response.result.message },
          }
        : { type: 'snapshot', snapshot: this.getSnapshot() };
    }
    this.applyResponse(command, response);
    return response;
  }

  private applyResponse(command: NativeAudioCommand, response: NativeAudioResponse): void {
    this.applySnapshot(response.snapshot);
    const owner = ownerFromCommand(command);
    if (owner && commandClaimsOwner(command) && response.type !== 'error' && !this.snapshot.owner) {
      this.snapshot = {
        ...this.snapshot,
        owner: { ...owner },
        activity: commandActivity(command),
      };
      this.emit({
        type: 'owner_changed',
        snapshot: this.getSnapshot(),
        owner: cloneOwner(this.snapshot.owner),
      });
    }
    if (owner && commandReleasesOwner(command, response) && sameOwner(this.snapshot.owner, owner)) {
      this.releaseOwner();
    }
  }

  private emit(event: NativeAudioEvent): void {
    for (const subscriber of this.subscribers) {
      try {
        subscriber(event);
      } catch (error) {
        this.opts.diagnostics.add('warn', 'host', `native audio event subscriber failed: ${String(error)}`);
      }
    }
  }

  private applySnapshot(snapshot: NativeAudioSnapshot): void {
    this.snapshot = cloneSnapshot(snapshot);
  }

  private releaseOwner(): void {
    if (!this.snapshot.owner && this.snapshot.activity === 'idle') return;
    this.snapshot = {
      ...this.snapshot,
      owner: null,
      activity: 'idle',
    };
    const cloned = this.getSnapshot();
    this.emit({ type: 'owner_changed', snapshot: cloned, owner: null });
    this.emit({ type: 'snapshot_changed', snapshot: cloned });
  }

  private setHelperState(state: NativeAudioSnapshot['helper']['state'], message?: string): void {
    this.snapshot = {
      ...this.snapshot,
      helper: {
        state,
        ...(message ? { message } : {}),
      },
    };
    const snapshot = this.getSnapshot();
    this.emit({ type: 'helper_state', snapshot, state, ...(message ? { message } : {}) });
    this.emit({ type: 'snapshot_changed', snapshot });
  }

  private async ensureHelper(): Promise<void> {
    if (this.helper) return;
    if (this.helperStartup) return this.helperStartup;
    if (!this.helperPath) throw new Error(this.snapshot.helper.message ?? 'native audio helper is unavailable');
    this.helperStartup = this.startHelper();
    try {
      await this.helperStartup;
    } finally {
      this.helperStartup = null;
    }
  }

  private helperAppPath(): string {
    if (!this.helperPath) throw new Error('native audio helper is unavailable');
    return resolve(this.helperPath, '..', '..', '..');
  }

  private async refreshHelperSnapshot(): Promise<NativeAudioSnapshot> {
    const response = await this.sendHelperEnvelope({
      id: randomUUID(),
      kind: 'command',
      command: { type: 'get_snapshot' },
    }, null);
    const normalized = this.normalizeCommandResponse({ type: 'get_snapshot' }, response);
    if (normalized.type === 'error') throw new Error(normalized.error.message);
    return this.getSnapshot();
  }

  private async resolvePackagedPermissions(permissions: AudioPermission[]): Promise<NativeAudioSnapshot> {
    if (this.permissionResolution) {
      await this.permissionResolution;
      return this.resolvePackagedPermissions(permissions);
    }
    const resolution = this.performPackagedPermissionResolution(permissions);
    this.permissionResolution = resolution;
    try {
      return await resolution;
    } finally {
      if (this.permissionResolution === resolution) this.permissionResolution = null;
    }
  }

  private async performPackagedPermissionResolution(permissions: AudioPermission[]): Promise<NativeAudioSnapshot> {
    if (permissions.includes('microphone')) {
      if (!this.opts.requestMicrophoneAccess) {
        throw new Error('outer application microphone authorization is unavailable');
      }
      await this.opts.requestMicrophoneAccess();
    }
    const before = await this.refreshHelperSnapshot();
    const unresolved = permissions.filter((permission, index) => {
      if (permissions.indexOf(permission) !== index) return false;
      return permission === 'speech' && before.permissions.speech === 'not_determined';
    });
    if (unresolved.length === 0) return before;
    await this.launchPermissionHelper(this.helperAppPath(), unresolved);
    return this.refreshHelperSnapshot();
  }

  private async startHelper(): Promise<void> {
    this.setHelperState('starting');
    this.verifyHelper();
    const env = {
      PATH: process.env['PATH'],
      TMPDIR: process.env['TMPDIR'],
      TMP: process.env['TMP'],
      TEMP: process.env['TEMP'],
      LANG: process.env['LANG'],
      LC_ALL: process.env['LC_ALL'],
      LINGXI_AUDIO_MODELS_ROOT: this.storageRoot,
    };
    const child = this.spawnHelper(this.helperPath!, ['--jsonl'], env);
    if (!child.stdin || !child.stdout || !child.stderr) {
      throw new Error('native audio helper stdio is unavailable');
    }
    this.helper = child;
    child.stdout.on('data', (chunk: Buffer | string) => {
      this.helperStdoutBuffer += chunk.toString();
      this.drainStdout();
    });
    child.stderr.on('data', (chunk: Buffer | string) => {
      this.opts.diagnostics.add('warn', 'host', `audio-helper: ${chunk.toString().trim()}`);
    });
    child.once('exit', (code, signal) => {
      const detail = `native audio helper exited (code=${String(code)} signal=${String(signal)})`;
      this.helper = null;
      this.helperStdoutBuffer = '';
      this.failPending(detail);
      this.releaseOwner();
      this.setHelperState(code === 0 ? 'stopped' : 'failed', detail);
    });
    this.setHelperState('running');
  }

  private verifyHelper(): void {
    if (!this.helperPath) return;
    if (!this.opts.isPackaged) return;
    const helperAppPath = this.helperAppPath();
    if (this.opts.verifyPackagedHelper) {
      this.opts.verifyPackagedHelper(helperAppPath);
      return;
    }
    const verification = spawnSync('/usr/bin/codesign', ['--verify', '--strict', helperAppPath], {
      encoding: 'utf8',
    });
    if (verification.status !== 0) throw new Error('packaged native audio helper signature is invalid');
    // `codesign -d` writes signing details to stderr by design.
    const description = spawnSync('/usr/bin/codesign', ['-dv', helperAppPath], { encoding: 'utf8' });
    if (description.status !== 0) throw new Error('packaged native audio helper signature cannot be inspected');
    const detail = `${description.stdout ?? ''}\n${description.stderr ?? ''}`;
    if (!/TeamIdentifier=AZ4AX7J833\b/.test(detail)) {
      throw new Error('packaged native audio helper is signed by the wrong Apple team');
    }
    if (!/Identifier=com\.lingxi\.code\.audio-helper(?:\.development)?\b/.test(detail)) {
      throw new Error('packaged native audio helper has an unexpected bundle identifier');
    }
  }

  private drainStdout(): void {
    for (;;) {
      const newline = this.helperStdoutBuffer.indexOf('\n');
      if (newline < 0) return;
      const line = this.helperStdoutBuffer.slice(0, newline).trim();
      this.helperStdoutBuffer = this.helperStdoutBuffer.slice(newline + 1);
      if (!line) continue;
      let payload: unknown;
      try {
        payload = JSON.parse(line);
      } catch (error) {
        this.opts.diagnostics.add('warn', 'host', `native audio helper emitted invalid JSON: ${String(error)}`);
        continue;
      }
      this.handleHelperEnvelope(payload);
    }
  }

  private handleHelperEnvelope(value: unknown): void {
    const envelope = value as NativeAudioHelperEnvelope;
    if (envelope.type === 'event') {
      const event = validateNativeAudioEvent(envelope.event);
      if (event.type !== 'input_level') this.applySnapshot(event.snapshot);
      this.emit(event);
      return;
    }
    if (typeof envelope.id !== 'string') return;
    const pending = this.pending.get(envelope.id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(envelope.id);
    if (envelope.type === 'error') {
      const response = this.errorResponse('native-error', envelope.error.message);
      if (pending.kind === 'command') pending.resolve(response);
      else pending.resolve({ type: 'failed', kind: 'other', message: envelope.error.message });
      return;
    }
    try {
      if ((envelope.result as { type?: string }).type === 'engine_result') {
        const response = validateNativeAudioEngineResponse(envelope.result);
        this.applySnapshot(response.snapshot);
        if (pending.kind === 'command') pending.resolve(this.normalizeCommandResponse({ type: 'get_snapshot' }, response));
        else pending.resolve(response.result);
      } else {
        const response = validateNativeAudioResponse(envelope.result);
        if (pending.kind === 'command') pending.resolve(response);
        else pending.resolve(nativeAudioResponseToEngineResult(response));
      }
    } catch (error) {
      pending.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private failPending(message: string): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.reject(new Error(message));
    }
  }

  private sendHelperEnvelope(
    envelope: NativeAudioHelperCommandEnvelope,
    owner: NativeAudioOwner | null,
  ): Promise<NativeAudioResponse | NativeAudioEngineResponse> {
    if (!this.helper?.stdin?.writable) throw new Error('native audio helper is not writable');
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(envelope.id);
        reject(new Error(`native audio helper timed out waiting for ${envelope.kind}`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(envelope.id, owner && envelope.kind === 'engine_request'
        ? {
            kind: 'engine',
            owner,
            op: envelope.op,
            resolve: (result) => resolve({ type: 'engine_result', snapshot: this.getSnapshot(), result }),
            reject,
            timer,
          }
        : {
            kind: 'command',
            owner,
            resolve,
            reject,
            timer,
          });
      try {
        this.helper!.stdin!.write(`${JSON.stringify(envelope)}\n`);
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(envelope.id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
}
