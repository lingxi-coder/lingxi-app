import type { FusionInvocation } from '../shared/fusionStrategy.js';
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
// Includes native cold service readiness (60s), one operation (15s), and
// helper signature/install overhead. This remains a hard process deadline.
const DEFAULT_TIMEOUT_MS = 90_000;

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

/** Credential slots declared by one provider/connection, including its failover keys. */
export function resolveProfileCredentialIds(profileId: string, settings: unknown): string[] {
  const providers = isRecord(settings) && isRecord(settings.providers) ? settings.providers : {};
  const separator = profileId.search(/[:#]/);
  const group = separator < 0 ? profileId : profileId.slice(0, separator);
  const profile = providers[group];
  if (!isRecord(profile)) return providerById(profileId) ? [profileId] : [];
  const keys = (value: unknown, fallback: string[]): string[] => {
    if (value === undefined) return fallback;
    if (!Array.isArray(value) || value.length === 0
      || !value.every((id) => typeof id === 'string' && /^[a-z0-9][a-z0-9._-]{0,63}$/.test(id))) return [];
    return value as string[];
  };
  const inherited = keys(profile.credentialIds, [group]);
  if (!Array.isArray(profile.connections)) return inherited;
  const connectionId = profileId.startsWith(`${group}:`) ? profileId.slice(group.length + 1).split('#')[0] : undefined;
  const connections = profile.connections.filter(isRecord);
  if (connectionId !== undefined && !connections.some((connection) => connection.id === connectionId)) return [];
  // A selected connection may fail over to another connection of the same
  // provider. Only this group's keys are eligible; other profiles stay private.
  return [...new Set(connections.flatMap((connection) => keys(connection.credentialIds, inherited)))];
}

/** Host writes may target a known provider or an explicitly configured key slot. */
export function configuredCredentialProviderIds(settings: unknown): string[] {
  const providers = isRecord(settings) && isRecord(settings.providers) ? settings.providers : {};
  return [...new Set(Object.keys(providers).flatMap((profile) =>
    /^[a-z0-9][a-z0-9._-]{0,63}$/.test(profile) && isRecord(providers[profile])
      ? [profile, ...resolveProfileCredentialIds(profile, settings)] : []))];
}

/** Mirror registered custom model/alias resolution; decrypt only the selected route's keys. */
export function resolveModelCredentialProviderIds(model: string, settings: unknown): string[] {
  const config = isRecord(settings) ? settings : {};
  const providers = isRecord(config.providers) ? config.providers : {};
  const routing = isRecord(config.routing) ? config.routing : {};
  const rows: Array<{ provider: string; model: string; aliases: string[]; credentialProfile?: string }> = [];
  for (const [provider, profile] of Object.entries(providers)) {
    if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(provider) || !isRecord(profile)) continue;
    const connections = Array.isArray(profile.connections) ? profile.connections.filter(isRecord) : [profile];
    for (const connection of connections) {
      const models = connection.models ?? profile.models;
      if (!Array.isArray(models)) continue;
      for (const entry of models) {
        const id = typeof entry === 'string' ? entry : isRecord(entry) && typeof entry.id === 'string' ? entry.id : undefined;
        if (!id) continue;
        rows.push({ provider, model: id, aliases: isRecord(entry) && Array.isArray(entry.aliases)
          ? entry.aliases.filter((alias): alias is string => typeof alias === 'string') : [] });
      }
    }
  }
  const explicit = (reference: string): typeof rows[number] | undefined => {
    const slash = reference.indexOf('/');
    if (slash < 1 || slash === reference.length - 1) return undefined;
    const credentialProfile = reference.slice(0, slash), id = reference.slice(slash + 1);
    const provider = credentialProfile.split(/[:#]/)[0]!;
    const found = rows.find((row) => row.provider === provider && row.model === id);
    if (found) return credentialProfile === provider ? found : { ...found, credentialProfile };
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
  if (new Set(matches.map((row) => row.provider)).size > 1) return []; // Same-group connections are one model choice.
  const primary = matches[0] ?? explicit(model);
  const legacy = primary ? undefined : resolveProviderIdForModel(model);
  const primaryId = primary?.provider ?? (legacy && providerById(legacy) ? legacy : undefined);
  if (!primaryId) return [];
  const ids = new Set(resolveProfileCredentialIds(primary?.credentialProfile ?? primaryId, settings));
  const chain = isRecord(routing.fallback) ? routing.fallback[primary?.model ?? model] : undefined;
  if (Array.isArray(chain)) {
    for (const target of chain) {
      const row = typeof target === 'string' ? explicit(target) : undefined;
      if (row) for (const id of resolveProfileCredentialIds(row.credentialProfile ?? row.provider, settings)) ids.add(id);
    }
  }
  return [...ids];
}

/** A private credential view; the captured reference/config revision stays unchanged. */
export function fusionCredentialSelection<T extends { config: Record<string, unknown> }>(
  selection: T,
  invocation: Pick<FusionInvocation, 'models' | 'preset' | 'maxPanel'> = {},
  parentProfile?: string,
): T {
  const config = selection.config;
  let panels: unknown[];
  if (invocation.models) {
    // Fusion resolves literal model ids inside one profile. A bare id inherits
    // the captured conversation profile, never an ordinary routing alias.
    panels = invocation.models.map((model) => {
      const colon = model.indexOf(':');
      if (colon < 0 && !parentProfile) throw new Error('The current conversation provider is unavailable for bare Fusion model ids.');
      return { profile: colon < 0 ? parentProfile : model.slice(0, colon), model: colon < 0 ? model : model.slice(colon + 1) };
    });
  } else {
    const integer = (key: string, fallback: number) => typeof config[key] === 'number' && Number.isInteger(config[key]) ? config[key] as number : fallback;
    const preset = invocation.preset ?? (config.preset === 'fast' ? 'fast' : 'quality');
    const configuredMax = Math.max(2, Math.min(8, integer('maxPanel', 8)));
    const maxPanel = Math.max(2, Math.min(configuredMax, invocation.maxPanel ?? configuredMax));
    const required = Math.max(2, integer('minSuccessfulPanels', 2));
    const presetCount = integer(preset === 'fast' ? 'fastPanelCount' : 'qualityPanelCount', preset === 'fast' ? 2 : 3);
    const wanted = Math.max(Math.min(presetCount, maxPanel), required);
    panels = Array.isArray(config.panelModels) ? config.panelModels.slice(0, wanted) : [];
  }
  return { ...selection, config: { ...config, panelModels: panels } };
}

/** Fusion routes are explicit settings choices, independent of the parent model. */
export function resolveFusionCredentialProviderIds(settings: unknown, selection: { config: Record<string, unknown> }, explicit = false): string[] {
  const fusion = selection.config;
  if (!explicit && fusion.enabled !== true) return [];
  const choices = [
    ...(Array.isArray(fusion.panelModels) ? fusion.panelModels : []),
    fusion.analystModel,
  ];
  return [...new Set(choices.flatMap((choice) => {
    if (!isRecord(choice) || typeof choice.profile !== 'string' || typeof choice.model !== 'string') return [];
    // Configured Fusion routes are exact pairs. Ordinary aliases and fallback
    // chains can refer to unrelated providers that this ensemble never calls.
    const profile = choice.profile.trim();
    if (!choice.model.trim()) return [];
    return resolveProfileCredentialIds(profile, settings);
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

type SessionLaunchCredentials = { apiKey?: string; providerCredentials?: Record<string, string> };

interface LaunchCacheEntry<T> {
  value: T;
  expiresAt: number;
}

/**
 * Reuse broker-owned launch material across the short burst of runtimes a
 * Desktop window commonly creates. The cache lives only in the Electron main
 * process, expires quickly, and is explicitly invalidated after credential or
 * plugin-secret mutations; secret bytes never cross into renderer state.
 */
export class SessionLaunchCache {
  private readonly credentialEntries = new Map<string, LaunchCacheEntry<SessionLaunchCredentials>>();
  private readonly credentialPending = new Map<string, Promise<SessionLaunchCredentials>>();
  private pluginSecretsEntry?: LaunchCacheEntry<Record<string, Record<string, string>>>;
  private pluginSecretsPending?: Promise<Record<string, Record<string, string>>>;
  private generation = 0;

  constructor(private readonly ttlMs = 30_000) {}

  credentials(model: string | null | undefined, broker?: ProviderCredentialResolver): Promise<SessionLaunchCredentials> {
    const key = model ?? '';
    const now = Date.now();
    const cached = this.credentialEntries.get(key);
    if (cached && cached.expiresAt > now) return Promise.resolve(cached.value);
    if (cached) this.credentialEntries.delete(key);
    const pending = this.credentialPending.get(key);
    if (pending) return pending;
    const generation = this.generation;
    const operation = resolveSessionLaunchCredentials(model, { credentialBroker: broker })
      .then((value) => {
        // Invalidation also fences callers already waiting on a broker read;
        // avoiding cache insertion alone would still return the retired key.
        if (generation !== this.generation) return this.credentials(model, broker);
        this.credentialEntries.set(key, { value, expiresAt: Date.now() + this.ttlMs });
        return value;
      })
      .finally(() => {
        if (this.credentialPending.get(key) === operation) this.credentialPending.delete(key);
      });
    this.credentialPending.set(key, operation);
    return operation;
  }

  pluginSecrets(broker?: ProviderCredentialBroker): Promise<Record<string, Record<string, string>>> {
    const now = Date.now();
    if (this.pluginSecretsEntry && this.pluginSecretsEntry.expiresAt > now) return Promise.resolve(this.pluginSecretsEntry.value);
    this.pluginSecretsEntry = undefined;
    if (this.pluginSecretsPending) return this.pluginSecretsPending;
    const generation = this.generation;
    const operation = resolveSessionLaunchPluginSecrets(broker)
      .then((value) => {
        if (generation === this.generation) this.pluginSecretsEntry = { value, expiresAt: Date.now() + this.ttlMs };
        return value;
      })
      .finally(() => {
        if (this.pluginSecretsPending === operation) this.pluginSecretsPending = undefined;
      });
    this.pluginSecretsPending = operation;
    return operation;
  }

  invalidate(): void {
    this.generation += 1;
    this.credentialEntries.clear();
    this.pluginSecretsEntry = undefined;
    this.credentialPending.clear();
    this.pluginSecretsPending = undefined;
  }
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
  // All host-owned keys enter the mutable credential store. The legacy
  // api_key scalar binds Anthropic's auth source at engine construction.
  return { providerCredentials: { [providerId]: credential } };
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
