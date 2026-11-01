import { spawn, type ChildProcessWithoutNullStreams, type SpawnOptionsWithoutStdio } from 'node:child_process';
import { Buffer } from 'node:buffer';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const CREDENTIAL_BROKER_BIN = process.platform === 'win32'
  ? 'lingxi-credential-client.exe'
  : 'lingxi-credential-client';
const CREDENTIAL_BROKER_PROTOCOL_VERSION = 1;
const MAX_REQUEST_BYTES = 128 * 1024;
const MAX_RESPONSE_BYTES = 128 * 1024;
const MAX_STDERR_BYTES = 8 * 1024;
// Includes native cold service readiness (60s), one operation (15s), and
// helper signature/install overhead. This remains a hard process deadline.
const DEFAULT_TIMEOUT_MS = 90_000;

export interface CredentialBrokerHealth {
  protocolVersion: number;
  buildVersion: string;
}

export interface CredentialBrokerStatus {
  providerId: string;
  configured: boolean;
}

export interface CredentialBrokerPreview extends CredentialBrokerStatus {
  maskedValue?: string;
}

export interface PluginSecretRef {
  pluginId: string;
  key: string;
}

export interface PluginSecretPreview extends PluginSecretRef {
  configured: boolean;
  maskedValue?: string;
}

export interface PluginSecretDeletionSummary {
  deletedCount: number;
  failedCount: number;
  aborted: boolean;
}

export interface ProviderCredentialBroker {
  readonly credentialChannel?: 'development' | 'production';
  health(): Promise<CredentialBrokerHealth>;
  listStatus(providerIds: readonly string[]): Promise<CredentialBrokerStatus[]>;
  preview(providerId: string): Promise<CredentialBrokerPreview>;
  resolve(providerId: string): Promise<string | undefined>;
  set(providerId: string, secret: string): Promise<CredentialBrokerPreview>;
  delete(providerId: string): Promise<void>;
  listPluginSecrets(): Promise<PluginSecretRef[]>;
  previewPluginSecret(pluginId: string, key: string): Promise<PluginSecretPreview>;
  resolvePluginSecret(pluginId: string, key: string): Promise<string | undefined>;
  setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretPreview>;
  deletePluginSecret(pluginId: string, key: string): Promise<void>;
}


type BrokerRequest = {
  op: 'health' | 'store' | 'retrieve' | 'contains' | 'delete' | 'list' | 'preview';
  service?: string;
  account?: string;
  payload?: string;
};

type BrokerResponse = {
  ok: boolean;
  present?: boolean;
  payload?: string;
  accounts?: string[];
  protocol_version?: number;
  build_version?: string;
  error_kind?: string;
  error?: string;
};

interface CredentialBrokerTransport {
  request(input: BrokerRequest): Promise<BrokerResponse>;
}

class UnavailableCredentialBrokerTransport implements CredentialBrokerTransport {
  constructor(private readonly message = 'macOS credential broker signing configuration is unavailable; use an Apple Development signed package with a valid provisioning profile') {}

  async request(): Promise<BrokerResponse> {
    throw new Error(this.message);
  }
}

interface CreateCredentialBrokerClientOptions {
  binaryPath?: string;
  isPackaged?: boolean;
  platform?: NodeJS.Platform;
  resourcesPath?: string;
  timeoutMs?: number;
  spawnProcess?: SpawnProcess;
  transport?: CredentialBrokerTransport;
  channel?: 'development' | 'production';
}

interface SpawnProcess {
  (bin: string, args: readonly string[], options: SpawnOptionsWithoutStdio): ChildProcessWithoutNullStreams;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function validateProviderId(providerId: string): string {
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(providerId)) {
    throw new Error('invalid provider id');
  }
  return providerId;
}

export function validatePluginSecretPluginId(pluginId: string): string {
  if (!/^[A-Za-z0-9][A-Za-z0-9._@-]{0,127}$/.test(pluginId)) {
    throw new Error('invalid plugin id');
  }
  return pluginId;
}

function validatePluginSecretRef(pluginId: string, key: string): PluginSecretRef {
  validatePluginSecretPluginId(pluginId);
  if (!/^[A-Za-z_][A-Za-z0-9_.-]{0,127}$/.test(key)) {
    throw new Error('invalid plugin secret key');
  }
  return { pluginId, key };
}

function pluginSecretAccount(pluginId: string, key: string): string {
  const validated = validatePluginSecretRef(pluginId, key);
  return `${encodeURIComponent(validated.pluginId)}/${encodeURIComponent(validated.key)}`;
}

function parsePluginSecretAccount(account: string): PluginSecretRef | undefined {
  const separator = account.indexOf('/');
  if (separator <= 0 || separator === account.length - 1 || account.indexOf('/', separator + 1) !== -1) {
    return undefined;
  }
  try {
    return validatePluginSecretRef(
      decodeURIComponent(account.slice(0, separator)),
      decodeURIComponent(account.slice(separator + 1)),
    );
  } catch {
    return undefined;
  }
}

/** Resolve all broker-owned plugin secrets for the one-shot engine envelope. */
export async function resolveSessionLaunchPluginSecrets(
  credentialBroker?: ProviderCredentialBroker,
): Promise<Record<string, Record<string, string>>> {
  const result: Record<string, Record<string, string>> = Object.create(null);
  if (!credentialBroker) return result;
  const refs = await credentialBroker.listPluginSecrets();
  if (refs.length > 256) throw new Error('credential broker contains too many plugin secrets');
  const entries = await Promise.all(refs.map(async ({ pluginId, key }) => ({
    pluginId,
    key,
    value: await credentialBroker.resolvePluginSecret(pluginId, key),
  })));
  for (const { pluginId, key, value } of entries) {
    if (value === undefined) continue;
    (result[pluginId] ??= Object.create(null))[key] = value;
  }
  return result;
}

/** Delete every durable broker key for one plugin, including keys absent from its current schema. */
export async function deletePluginSecretsForPlugin(
  pluginId: string,
  broker?: Pick<ProviderCredentialBroker, 'listPluginSecrets' | 'deletePluginSecret'>,
  canDelete?: () => Promise<boolean>,
): Promise<PluginSecretDeletionSummary> {
  validatePluginSecretPluginId(pluginId);
  if (!broker) throw new Error('secure plugin credential broker is unavailable');

  const refs = await broker.listPluginSecrets();
  if (refs.length > 256) throw new Error('credential broker contains too many plugin secrets');
  const keys = new Set<string>();
  for (const ref of refs) {
    const validated = validatePluginSecretRef(ref.pluginId, ref.key);
    if (validated.pluginId === pluginId) keys.add(validated.key);
  }

  let deletedCount = 0;
  let failedCount = 0;
  if (canDelete && !(await canDelete())) return { deletedCount, failedCount, aborted: true };
  for (const key of keys) {
    if (canDelete && !(await canDelete())) return { deletedCount, failedCount, aborted: true };
    try {
      await broker.deletePluginSecret(pluginId, key);
      deletedCount += 1;
    } catch {
      // Do not return broker diagnostics or credential material to the renderer.
      failedCount += 1;
    }
  }
  return { deletedCount, failedCount, aborted: false };
}

function truncateStderr(stderr: string): string {
  const trimmed = stderr.trim();
  if (!trimmed) return '';
  return trimmed.length <= 512 ? trimmed : `${trimmed.slice(0, 512)}…`;
}

function readStringField(value: Record<string, unknown>, field: string): string {
  const candidate = value[field];
  if (typeof candidate !== 'string' || candidate.length === 0) {
    throw new Error(`credential broker response missing ${field}`);
  }
  return candidate;
}

export function configuredCredentialProviderIds(settings: unknown): string[] {
  const config = isRecord(settings) && isRecord(settings.effective_json) ? settings.effective_json : settings;
  return isRecord(config) && isRecord(config.providers) ? Object.keys(config.providers) : [];
}

class ChildProcessCredentialBrokerTransport implements CredentialBrokerTransport {
  private readonly spawnProcess: SpawnProcess;
  private readonly timeoutMs: number;

  constructor(
    private readonly binaryPath: string,
    options: {
      spawnProcess?: SpawnProcess;
      timeoutMs?: number;
    } = {},
  ) {
    this.spawnProcess = options.spawnProcess ?? spawn;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  }

  async request(input: BrokerRequest): Promise<BrokerResponse> {
    const payload = JSON.stringify(input);
    if (Buffer.byteLength(payload) > MAX_REQUEST_BYTES) {
      throw new Error('credential broker request too large');
    }
    return new Promise((resolve, reject) => {
      const child = this.spawnProcess(this.binaryPath, [], {
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
      });
      let settled = false;
      const stdout: Buffer[] = [];
      let stdoutBytes = 0;
      const stderr: Buffer[] = [];
      let stderrBytes = 0;
      const finish = (action: () => void): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        action();
      };
      const timer = setTimeout(() => {
        child.kill('SIGKILL');
        finish(() => reject(new Error('credential broker timed out')));
      }, this.timeoutMs);
      timer.unref();
      child.stdout.on('data', (chunk: Buffer | string) => {
        if (settled) return;
        const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        stdoutBytes += bytes.length;
        if (stdoutBytes > MAX_RESPONSE_BYTES) {
          child.kill('SIGKILL');
          finish(() => reject(new Error('credential broker response too large')));
          return;
        }
        stdout.push(bytes);
      });
      child.stderr.on('data', (chunk: Buffer | string) => {
        if (settled || stderrBytes >= MAX_STDERR_BYTES) return;
        const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        const fragment = bytes.subarray(0, MAX_STDERR_BYTES - stderrBytes);
        stderr.push(fragment);
        stderrBytes += fragment.length;
      });
      child.once('error', (error) => finish(() => reject(error instanceof Error ? error : new Error(String(error)))));
      child.stdin.once('error', (error) => finish(() => reject(error instanceof Error ? error : new Error(String(error)))));
      child.once('close', (code, signal) => {
        if (settled) return;
        if (code !== 0) {
          const detail = truncateStderr(Buffer.concat(stderr, stderrBytes).toString('utf8'));
          finish(() => reject(new Error(
            `credential broker exited with ${signal ?? `code ${String(code)}`}${detail ? `: ${detail}` : ''}`,
          )));
          return;
        }
        try {
          const parsed = JSON.parse(Buffer.concat(stdout, stdoutBytes).toString('utf8')) as BrokerResponse;
          if (!isRecord(parsed) || typeof parsed.ok !== 'boolean') {
            throw new Error('credential broker returned an invalid response envelope');
          }
          if (!parsed.ok) {
            const error = typeof parsed.error === 'string' && parsed.error.length > 0
              ? parsed.error
              : 'credential broker request failed';
            finish(() => reject(new Error(error)));
            return;
          }
          finish(() => resolve(parsed));
        } catch (error) {
          finish(() => reject(error instanceof Error ? error : new Error(String(error))));
        }
      });
      child.stdin.end(payload);
    });
  }
}

class CredentialBrokerClient implements ProviderCredentialBroker {
  private healthPromise: Promise<CredentialBrokerHealth> | null = null;

  constructor(
    private readonly transport: CredentialBrokerTransport,
    private readonly channel: 'development' | 'production',
  ) {}

  get credentialChannel(): 'development' | 'production' { return this.channel; }

  health(): Promise<CredentialBrokerHealth> {
    this.healthPromise ??= this.loadHealth().catch((error) => {
      this.healthPromise = null;
      throw error;
    });
    return this.healthPromise;
  }

  async listStatus(providerIds: readonly string[]): Promise<CredentialBrokerStatus[]> {
    await this.health();
    const ids = providerIds.map((providerId) => validateProviderId(providerId));
    const response = await this.requestChecked({ op: 'list', service: this.providerServiceName() });
    const configured = new Set(readAccounts(response));
    return ids.map((providerId) => ({ providerId, configured: configured.has(providerId) }));
  }

  async preview(providerId: string): Promise<CredentialBrokerPreview> {
    await this.health();
    const id = validateProviderId(providerId);
    const response = await this.requestChecked({
      op: 'preview',
      service: this.providerServiceName(),
      account: id,
    });
    const maskedValue = readMaskedPreview(response);
    return {
      providerId: id,
      configured: response.present === true,
      ...(maskedValue ? { maskedValue } : {}),
    };
  }

  async resolve(providerId: string): Promise<string | undefined> {
    await this.health();
    const response = await this.requestChecked({
      op: 'retrieve',
      service: this.providerServiceName(),
      account: validateProviderId(providerId),
    });
    if (response.present !== true) return undefined;
    const secret = response.payload;
    if (typeof secret !== 'string' || !secret || secret.length > 16_384 || secret.includes('\0')) {
      throw new Error('credential broker returned an invalid secret payload');
    }
    return secret;
  }

  async set(providerId: string, secret: string): Promise<CredentialBrokerPreview> {
    const id = validateProviderId(providerId);
    if (!secret || secret.length > 16_384 || secret.includes('\0')) {
      throw new Error('invalid provider credential');
    }
    await this.health();
    await this.requestChecked({
      op: 'store',
      service: this.providerServiceName(),
      account: id,
      payload: secret,
    });
    return this.preview(id);
  }

  async delete(providerId: string): Promise<void> {
    await this.health();
    await this.requestChecked({
      op: 'delete',
      service: this.providerServiceName(),
      account: validateProviderId(providerId),
    });
  }

  async listPluginSecrets(): Promise<PluginSecretRef[]> {
    await this.health();
    const response = await this.requestChecked({ op: 'list', service: this.pluginSecretServiceName() });
    return readAccounts(response).flatMap((account) => {
      const parsed = parsePluginSecretAccount(account);
      return parsed ? [parsed] : [];
    });
  }

  async previewPluginSecret(pluginId: string, key: string): Promise<PluginSecretPreview> {
    await this.health();
    const ref = validatePluginSecretRef(pluginId, key);
    const response = await this.requestChecked({
      op: 'preview',
      service: this.pluginSecretServiceName(),
      account: pluginSecretAccount(pluginId, key),
    });
    const maskedValue = readMaskedPreview(response);
    return {
      ...ref,
      configured: response.present === true,
      ...(maskedValue ? { maskedValue } : {}),
    };
  }

  async resolvePluginSecret(pluginId: string, key: string): Promise<string | undefined> {
    await this.health();
    const response = await this.requestChecked({
      op: 'retrieve',
      service: this.pluginSecretServiceName(),
      account: pluginSecretAccount(pluginId, key),
    });
    if (response.present !== true) return undefined;
    const secret = response.payload;
    if (typeof secret !== 'string' || !secret || secret.length > 16_384 || secret.includes('\0')) {
      throw new Error('credential broker returned an invalid plugin secret payload');
    }
    return secret;
  }

  async setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretPreview> {
    if (!secret || secret.length > 16_384 || secret.includes('\0')) {
      throw new Error('invalid plugin secret');
    }
    await this.health();
    await this.requestChecked({
      op: 'store',
      service: this.pluginSecretServiceName(),
      account: pluginSecretAccount(pluginId, key),
      payload: secret,
    });
    return this.previewPluginSecret(pluginId, key);
  }

  async deletePluginSecret(pluginId: string, key: string): Promise<void> {
    await this.health();
    await this.requestChecked({
      op: 'delete',
      service: this.pluginSecretServiceName(),
      account: pluginSecretAccount(pluginId, key),
    });
  }

  private async loadHealth(): Promise<CredentialBrokerHealth> {
    const result = await this.requestChecked({ op: 'health' });
    return {
      protocolVersion: CREDENTIAL_BROKER_PROTOCOL_VERSION,
      buildVersion: readStringField(result, 'build_version'),
    };
  }

  private async requestChecked(input: BrokerRequest): Promise<BrokerResponse> {
    const result = await this.transport.request(input);
    const protocolVersion = result.protocol_version;
    if (typeof protocolVersion !== 'number') {
      throw new Error('credential broker response missing protocol_version');
    }
    if (protocolVersion !== CREDENTIAL_BROKER_PROTOCOL_VERSION) {
      throw new Error(
        `credential broker protocol mismatch (expected ${CREDENTIAL_BROKER_PROTOCOL_VERSION}, got ${protocolVersion}); upgrade LingXi Desktop and the bundled credential broker together`,
      );
    }
    return result;
  }

  private providerServiceName(): string {
    return this.channel === 'production'
      ? 'com.lingxi.provider-credentials.v1'
      : 'com.lingxi.provider-credentials.v1.development';
  }

  private pluginSecretServiceName(): string {
    return this.channel === 'production'
      ? 'com.lingxi.plugin-secrets.v1'
      : 'com.lingxi.plugin-secrets.v1.development';
  }
}

function readMaskedPreview(response: BrokerResponse): string | undefined {
  if (response.present !== true) return undefined;
  const value = response.payload;
  if (typeof value !== 'string'
    || !value.startsWith('••••')
    || Array.from(value.slice(4)).length > 4) {
    throw new Error('credential broker returned an invalid masked preview');
  }
  return value;
}

function readAccounts(response: BrokerResponse): string[] {
  if (!Array.isArray(response.accounts) || response.accounts.some((value) => typeof value !== 'string')) {
    throw new Error('credential broker returned an invalid account list');
  }
  return response.accounts;
}

export function createMacCredentialBrokerClient(
  options: CreateCredentialBrokerClientOptions = {},
): ProviderCredentialBroker | undefined {
  let channel = options.channel ?? ((options.isPackaged ?? false) ? 'production' : 'development');
  if (options.transport) return new CredentialBrokerClient(options.transport, channel);
  const platform = options.platform ?? process.platform;
  if (platform !== 'darwin') return undefined;
  if (!(options.isPackaged ?? false)) {
    return new CredentialBrokerClient(new UnavailableCredentialBrokerTransport(), channel);
  }
  if (!options.channel) {
    try {
      const manifest = JSON.parse(readFileSync(
        join(options.resourcesPath ?? process.resourcesPath, 'credential-broker', 'broker-manifest.json'),
        'utf8',
      )) as { channel?: unknown; protocol_version?: unknown };
      if ((manifest.channel !== 'development' && manifest.channel !== 'production')
        || manifest.protocol_version !== CREDENTIAL_BROKER_PROTOCOL_VERSION) {
        throw new Error('invalid manifest values');
      }
      channel = manifest.channel;
    } catch (error) {
      return new CredentialBrokerClient(new UnavailableCredentialBrokerTransport(
        `macOS credential broker manifest is unavailable or invalid: ${error instanceof Error ? error.message : String(error)}`,
      ), channel);
    }
  }
  const binaryPath = options.binaryPath
    ?? join(options.resourcesPath ?? process.resourcesPath, 'credential-broker', 'bin', CREDENTIAL_BROKER_BIN);
  return new CredentialBrokerClient(new ChildProcessCredentialBrokerTransport(binaryPath, {
    timeoutMs: options.timeoutMs,
    spawnProcess: options.spawnProcess,
  }), channel);
}
