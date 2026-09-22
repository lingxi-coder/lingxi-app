import { providerById } from '../shared/providers.js';
import { spawn, type ChildProcessWithoutNullStreams, type SpawnOptionsWithoutStdio } from 'node:child_process';
import { Buffer } from 'node:buffer';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { CODEX_PROVIDER_ID, parseCodexSession, type CodexOAuthSession } from './codex-auth.js';

const CREDENTIAL_BROKER_BIN = process.platform === 'win32'
  ? 'lingxi-credential-client.exe'
  : 'lingxi-credential-client';
const CREDENTIAL_BROKER_PROTOCOL_VERSION = 1;
const MAX_REQUEST_BYTES = 128 * 1024;
const MAX_RESPONSE_BYTES = 128 * 1024;
const MAX_STDERR_BYTES = 8 * 1024;
const DEFAULT_TIMEOUT_MS = 20_000;

export const providerEnvironmentVariables: Readonly<Record<string, string>> = {
  anthropic: 'ANTHROPIC_API_KEY',
  openai: 'OPENAI_API_KEY',
  deepseek: 'DEEPSEEK_API_KEY',
  kimi: 'MOONSHOT_API_KEY',
  'kimi-code': 'KIMI_API_KEY',
  gemini: 'GEMINI_API_KEY',
  openrouter: 'OPENROUTER_API_KEY',
  zai: 'ZAI_API_KEY',
  'glm-coding': 'GLM_API_KEY',
  'github-copilot': 'GITHUB_TOKEN',
};

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

export interface ProviderCredentialBroker {
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

type ProviderCredentialResolver = Pick<ProviderCredentialBroker, 'resolve'>;

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

function validatePluginSecretRef(pluginId: string, key: string): PluginSecretRef {
  if (!/^[A-Za-z0-9][A-Za-z0-9._@-]{0,127}$/.test(pluginId)) {
    throw new Error('invalid plugin id');
  }
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
  if (!credentialBroker) return {};
  const refs = await credentialBroker.listPluginSecrets();
  if (refs.length > 256) throw new Error('credential broker contains too many plugin secrets');
  const entries = await Promise.all(refs.map(async ({ pluginId, key }) => ({
    pluginId,
    key,
    value: await credentialBroker.resolvePluginSecret(pluginId, key),
  })));
  const result: Record<string, Record<string, string>> = {};
  for (const { pluginId, key, value } of entries) {
    if (value === undefined) continue;
    (result[pluginId] ??= {})[key] = value;
  }
  return result;
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

export function resolveProviderIdForModel(model: string | null | undefined): string | undefined {
  if (!model) return undefined;
  const separator = model.indexOf('/');
  if (separator > 0 && separator < model.length - 1) {
    const prefix = model.slice(0, separator);
    if (prefix === 'builtin' || prefix === 'claude') return 'anthropic';
    return /^[a-z0-9][a-z0-9._-]{0,63}$/.test(prefix) ? prefix : undefined;
  }
  return model.startsWith('claude') ? 'anthropic' : undefined;
}

/** Mirror registered custom model/alias resolution; decrypt only the selected route's keys. */
export function resolveModelCredentialProviderIds(model: string, settings: unknown): string[] {
  const config = isRecord(settings) ? settings : {};
  const providers = isRecord(config.providers) ? config.providers : {};
  const routing = isRecord(config.routing) ? config.routing : {};
  const rows: Array<{ provider: string; model: string; aliases: string[] }> = [];
  for (const [provider, profile] of Object.entries(providers)) {
    if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(provider) || !isRecord(profile) || !Array.isArray(profile.models)) continue;
    for (const entry of profile.models) {
      const id = typeof entry === 'string' ? entry : isRecord(entry) && typeof entry.id === 'string' ? entry.id : undefined;
      if (!id) continue;
      rows.push({ provider, model: id, aliases: isRecord(entry) && Array.isArray(entry.aliases)
        ? entry.aliases.filter((alias): alias is string => typeof alias === 'string') : [] });
    }
  }
  const explicit = (reference: string): typeof rows[number] | undefined => {
    const slash = reference.indexOf('/');
    if (slash < 1 || slash === reference.length - 1) return undefined;
    const provider = reference.slice(0, slash), id = reference.slice(slash + 1);
    const found = rows.find((row) => row.provider === provider && row.model === id);
    if (found) return found;
    // Built-in targets are explicitly provider-qualified, never inferred from an unknown alias.
    if (providerById(provider)) return { provider, model: id, aliases: [] };
    return undefined;
  };
  if (isRecord(routing.aliases)) {
    for (const [alias, target] of Object.entries(routing.aliases)) {
      const row = typeof target === 'string' ? explicit(target) : undefined;
      if (!row) continue;
      if (!rows.includes(row)) rows.push(row);
      if (!row.aliases.includes(alias)) row.aliases.push(alias);
    }
  }
  // The engine checks exact ids/aliases before interpreting a slash as a profile qualifier.
  let matches = rows.filter((row) => row.model === model || row.aliases.includes(model));
  if (!matches.length) {
    const slash = model.indexOf('/');
    if (slash > 0) matches = rows.filter((row) => row.provider === model.slice(0, slash)
      && (row.model === model.slice(slash + 1) || row.aliases.includes(model.slice(slash + 1))));
  }
  if (matches.length > 1) return []; // The engine rejects ambiguous bare aliases.
  const primary = matches[0] ?? explicit(model);
  const legacy = primary ? undefined : resolveProviderIdForModel(model);
  const primaryId = primary?.provider ?? (legacy && providerById(legacy) ? legacy : undefined);
  if (!primaryId) return [];
  const ids = new Set([primaryId]);
  const chain = isRecord(routing.fallback) ? routing.fallback[primary?.model ?? model] : undefined;
  if (Array.isArray(chain)) {
    for (const target of chain) {
      const row = typeof target === 'string' ? explicit(target) : undefined;
      if (row) ids.add(row.provider);
    }
  }
  return [...ids];
}

/** Fusion routes are explicit settings choices, independent of the parent model. */
export function resolveFusionCredentialProviderIds(settings: unknown, explicit = false): string[] {
  if (!isRecord(settings) || !isRecord(settings.fusion)) return [];
  const fusion = settings.fusion;
  if (!explicit && fusion.enabled !== true) return [];
  const choices = [
    ...(Array.isArray(fusion.panelModels) ? fusion.panelModels : []),
    fusion.analystModel,
    fusion.synthesizerModel,
  ];
  return [...new Set(choices.flatMap((choice) => {
    if (!isRecord(choice) || typeof choice.profile !== 'string' || typeof choice.model !== 'string') return [];
    return resolveModelCredentialProviderIds(`${choice.profile}/${choice.model}`, settings);
  }))];
}

export function readProviderEnvironmentCredential(
  providerId: string,
  environment: NodeJS.ProcessEnv = process.env,
): string | undefined {
  const variable = providerEnvironmentVariables[providerId];
  const value = variable ? environment[variable] : undefined;
  return typeof value === 'string' && value.length > 0 && value.length <= 16_384 ? value : undefined;
}

export async function resolveProviderCredential(
  providerId: string,
  options: {
    credentialBroker?: ProviderCredentialResolver;
    environment?: NodeJS.ProcessEnv;
  } = {},
): Promise<string | undefined> {
  const id = validateProviderId(providerId);
  // OAuth JSON is not an API key and must never enter the key injection path.
  if (id === CODEX_PROVIDER_ID) return undefined;
  return readProviderEnvironmentCredential(id, options.environment)
    ?? await options.credentialBroker?.resolve(id);
}

export async function resolveCodexOAuthSession(broker?: ProviderCredentialResolver): Promise<CodexOAuthSession | undefined> {
  const secret = await broker?.resolve(CODEX_PROVIDER_ID);
  if (!secret) return undefined;
  try { return parseCodexSession(JSON.parse(secret)); }
  catch { throw new Error('Codex 登录凭据无效，请重新登录。'); }
}

export async function resolveSessionLaunchCredentials(
  model: string | null | undefined,
  options: {
    credentialBroker?: ProviderCredentialResolver;
    environment?: NodeJS.ProcessEnv;
  } = {},
): Promise<{ apiKey?: string; providerCredentials?: Record<string, string> }> {
  const providerId = resolveProviderIdForModel(model);
  if (!providerId) return {};
  const credential = await resolveProviderCredential(providerId, options);
  if (!credential) return {};
  return providerId === 'anthropic'
    ? { apiKey: credential }
    : { providerCredentials: { [providerId]: credential } };
}

export async function resolveProviderTestCredential(
  providerId: string,
  credentialOverride: string | undefined,
  credentialBroker?: ProviderCredentialResolver,
): Promise<string | undefined> {
  if (credentialOverride !== undefined) return credentialOverride;
  return credentialBroker?.resolve(validateProviderId(providerId));
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
      let stdout = '';
      let stdoutBytes = 0;
      let stderr = '';
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
        const text = chunk.toString();
        stdoutBytes += Buffer.byteLength(text);
        if (stdoutBytes > MAX_RESPONSE_BYTES) {
          child.kill('SIGKILL');
          finish(() => reject(new Error('credential broker response too large')));
          return;
        }
        stdout += text;
      });
      child.stderr.on('data', (chunk: Buffer | string) => {
        const text = chunk.toString();
        if (stderrBytes >= MAX_STDERR_BYTES) return;
        const remaining = MAX_STDERR_BYTES - stderrBytes;
        const fragment = Buffer.byteLength(text) <= remaining
          ? text
          : text.slice(0, remaining);
        stderr += fragment;
        stderrBytes += Buffer.byteLength(fragment);
      });
      child.once('error', (error) => finish(() => reject(error instanceof Error ? error : new Error(String(error)))));
      child.stdin.once('error', (error) => finish(() => reject(error instanceof Error ? error : new Error(String(error)))));
      child.once('close', (code, signal) => {
        if (settled) return;
        if (code !== 0) {
          const detail = truncateStderr(stderr);
          finish(() => reject(new Error(
            `credential broker exited with ${signal ?? `code ${String(code)}`}${detail ? `: ${detail}` : ''}`,
          )));
          return;
        }
        try {
          const parsed = JSON.parse(stdout) as BrokerResponse;
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
  }), channel);
}
