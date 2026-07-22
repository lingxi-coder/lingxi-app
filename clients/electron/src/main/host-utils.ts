import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve } from 'node:path';

export const SETTINGS_VERSION = 1 as const;
export const MAX_RECENT_WORKSPACES = 10;
export const MAX_DIAGNOSTICS = 200;
export const MAX_DIAGNOSTIC_LENGTH = 2_000;
export const MAX_TRUST_FINGERPRINT_ENTRIES = 512;
export const MAX_TRUST_FINGERPRINT_BYTES = 512 * 1024;
export const MAX_TRUST_FINGERPRINT_DEPTH = 12;

export const TRUST_CONFIG_PATHS = [
  '.mcp.json',
  '.claude/settings.json',
  '.claude/settings.local.json',
  '.lingxi/settings.json',
  '.lingxi/settings.local.json',
] as const;

export const TRUST_DIRECTORY_PATHS = [
  '.claude/agents',
  '.claude/commands',
  '.claude/plugins',
  '.claude/skills',
  '.lingxi/agents',
  '.lingxi/commands',
  '.lingxi/plugins',
  '.lingxi/skills',
] as const;

export const TRUST_MEMORY_PATHS = [
  'CLAUDE.md',
  'CLAUDE.local.md',
  'LINGXI.md',
  'LINGXI.local.md',
] as const;

export interface TrustRecord {
  fingerprint: string;
  trustedAt: string;
}

export interface PersistedSettings {
  version: typeof SETTINGS_VERSION;
  theme?: 'dark' | 'light';
  model?: string;
  apiBaseUrl?: string;
  lastWorkspace?: string;
  recentWorkspaces: string[];
  trustedWorkspaces: Record<string, TrustRecord>;
}

export interface PublicSettings {
  version: typeof SETTINGS_VERSION;
  theme?: 'dark' | 'light';
  model?: string;
  apiBaseUrl?: string;
  lastWorkspace?: string;
  recentWorkspaces: string[];
}

export interface DiagnosticEntry {
  timestamp: string;
  level: 'info' | 'warn' | 'error';
  source: 'host' | 'bridge';
  message: string;
}

interface FingerprintBudget {
  remainingBytes: number;
  remainingEntries: number;
}

export function defaultSettings(): PersistedSettings {
  return { version: SETTINGS_VERSION, recentWorkspaces: [], trustedWorkspaces: {} };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function boundedString(value: unknown, max: number): string | undefined {
  return typeof value === 'string' && value.length > 0 && value.length <= max ? value : undefined;
}

export function parseSettings(value: unknown): PersistedSettings {
  if (!isPlainObject(value) || value['version'] !== SETTINGS_VERSION) {
    return defaultSettings();
  }

  const settings = defaultSettings();
  settings.theme = value['theme'] === 'dark' || value['theme'] === 'light' ? value['theme'] : undefined;
  settings.model = boundedString(value['model'], 256);
  settings.apiBaseUrl = boundedString(value['apiBaseUrl'], 2_048);
  settings.lastWorkspace = boundedString(value['lastWorkspace'], 32_768);

  if (Array.isArray(value['recentWorkspaces'])) {
    settings.recentWorkspaces = value['recentWorkspaces']
      .filter((item): item is string => typeof item === 'string' && item.length > 0 && item.length <= 32_768)
      .slice(0, MAX_RECENT_WORKSPACES);
  }
  if (isPlainObject(value['trustedWorkspaces'])) {
    for (const [workspace, record] of Object.entries(value['trustedWorkspaces'])) {
      if (workspace.length > 32_768 || !isPlainObject(record)) continue;
      const fingerprint = boundedString(record['fingerprint'], 128);
      const trustedAt = boundedString(record['trustedAt'], 64);
      if (fingerprint && trustedAt) settings.trustedWorkspaces[workspace] = { fingerprint, trustedAt };
    }
  }
  return settings;
}

export function publicSettings(settings: PersistedSettings): PublicSettings {
  const { version, theme, model, apiBaseUrl, lastWorkspace, recentWorkspaces } = settings;
  return { version, theme, model, apiBaseUrl, lastWorkspace, recentWorkspaces: [...recentWorkspaces] };
}

export function canonicalWorkspace(input: string): string {
  if (typeof input !== 'string' || input.length === 0 || input.length > 32_768 || input.includes('\0')) {
    throw new Error('invalid workspace path');
  }
  const absolute = isAbsolute(input) ? normalize(input) : resolve(input);
  const canonical = realpathSync.native(absolute);
  if (!lstatSync(canonical).isDirectory()) throw new Error('workspace path is not a directory');
  return canonical;
}

export function withRecentWorkspace(settings: PersistedSettings, workspace: string): PersistedSettings {
  return {
    ...settings,
    lastWorkspace: workspace,
    recentWorkspaces: [workspace, ...settings.recentWorkspaces.filter((item) => item !== workspace)].slice(
      0,
      MAX_RECENT_WORKSPACES,
    ),
    trustedWorkspaces: { ...settings.trustedWorkspaces },
  };
}

function hashBuffer(hash: ReturnType<typeof createHash>, budget: FingerprintBudget, value: Buffer): void {
  if (value.byteLength > budget.remainingBytes) {
    throw new Error('workspace executable configuration exceeds the safe trust fingerprint limit');
  }
  hash.update(value);
  budget.remainingBytes -= value.byteLength;
}

function hashWorkspacePath(
  hash: ReturnType<typeof createHash>,
  root: string,
  relativePath: string,
  budget: FingerprintBudget,
  depth = 0,
): void {
  hash.update(relativePath);
  hash.update('\0');
  if (!existsSync(join(root, relativePath))) {
    hash.update('absent\0');
    return;
  }
  if (budget.remainingEntries <= 0) {
    throw new Error('workspace executable configuration exceeds the safe trust fingerprint entry limit');
  }
  budget.remainingEntries -= 1;

  const path = join(root, relativePath);
  const stat = lstatSync(path);
  if (stat.isSymbolicLink()) {
    hash.update('symlink\0');
    hash.update(readlinkSync(path));
    hash.update('\0');
    const target = statSync(path);
    if (target.isFile()) {
      hash.update(`target-file:${target.size}\0`);
      hashBuffer(hash, budget, readFileSync(path));
      hash.update('\0');
      return;
    }
    if (!target.isDirectory()) {
      hash.update(`target-non-file:${target.mode}\0`);
      return;
    }
    hash.update('target-dir\0');
    if (depth >= MAX_TRUST_FINGERPRINT_DEPTH) {
      throw new Error('workspace executable configuration exceeds the safe trust fingerprint depth');
    }
    for (const entry of readdirSync(path).sort((left, right) => left.localeCompare(right))) {
      hashWorkspacePath(hash, root, join(relativePath, entry), budget, depth + 1);
    }
    return;
  }
  if (stat.isDirectory()) {
    hash.update('dir\0');
    if (depth >= MAX_TRUST_FINGERPRINT_DEPTH) {
      throw new Error('workspace executable configuration exceeds the safe trust fingerprint depth');
    }
    const entries = readdirSync(path).sort((left, right) => left.localeCompare(right));
    for (const entry of entries) {
      if (budget.remainingEntries <= 0) {
        throw new Error('workspace executable configuration exceeds the safe trust fingerprint entry limit');
      }
      hashWorkspacePath(hash, root, join(relativePath, entry), budget, depth + 1);
    }
    return;
  }
  if (!stat.isFile()) {
    hash.update(`non-file:${stat.mode}\0`);
    return;
  }
  hash.update(`file:${stat.size}\0`);
  hashBuffer(hash, budget, readFileSync(path));
  hash.update('\0');
}

/** Hash executable project configuration, agents/plugins, and project memory with explicit absence markers. */
export function workspaceFingerprint(workspace: string): string {
  const root = canonicalWorkspace(workspace);
  const hash = createHash('sha256');
  const budget: FingerprintBudget = {
    remainingBytes: MAX_TRUST_FINGERPRINT_BYTES,
    remainingEntries: MAX_TRUST_FINGERPRINT_ENTRIES,
  };
  hash.update('lingxi-workspace-trust-v1\0');
  for (const relativePath of [...TRUST_CONFIG_PATHS, ...TRUST_MEMORY_PATHS, ...TRUST_DIRECTORY_PATHS]) {
    hashWorkspacePath(hash, root, relativePath, budget);
  }
  return hash.digest('hex');
}

export function workspaceTrust(settings: PersistedSettings, workspace: string): {
  trusted: boolean;
  fingerprint: string;
} {
  const canonical = canonicalWorkspace(workspace);
  const fingerprint = workspaceFingerprint(canonical);
  return {
    trusted: settings.trustedWorkspaces[canonical]?.fingerprint === fingerprint,
    fingerprint,
  };
}

export function setWorkspaceTrust(
  settings: PersistedSettings,
  workspace: string,
  trusted: boolean,
  now = new Date(),
): PersistedSettings {
  const canonical = canonicalWorkspace(workspace);
  const records = { ...settings.trustedWorkspaces };
  if (trusted) {
    records[canonical] = { fingerprint: workspaceFingerprint(canonical), trustedAt: now.toISOString() };
  } else {
    delete records[canonical];
  }
  return { ...settings, trustedWorkspaces: records };
}

const ENV_ALLOWLIST = [
  'HOME', 'PATH', 'TMPDIR', 'TMP', 'TEMP', 'LANG', 'LC_ALL', 'LC_CTYPE', 'USER', 'LOGNAME', 'SHELL',
  'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'LOCALAPPDATA', 'APPDATA', 'USERPROFILE',
  'SSL_CERT_FILE', 'SSL_CERT_DIR', 'NIX_SSL_CERT_FILE',
] as const;

export function buildBridgeEnvironment(source: NodeJS.ProcessEnv, apiBaseUrl?: string): NodeJS.ProcessEnv {
  const result: NodeJS.ProcessEnv = {};
  for (const name of ENV_ALLOWLIST) {
    const value = source[name];
    if (value !== undefined) result[name] = value;
  }
  if (apiBaseUrl) result['LINGXI_API_BASE_URL'] = apiBaseUrl;
  return result;
}

export function buildBridgeArguments(config: {
  workspace: string;
  bridgeDir: string;
  model?: string;
  hasApiKey: boolean;
  hasCredentialStdin?: boolean;
  trusted: boolean;
  packagedCredentialBoundary?: boolean;
}): string[] {
  const args = ['--cwd', config.workspace, '--bridge-dir', config.bridgeDir];
  if (config.model) args.push('--model', config.model);
  if (config.hasCredentialStdin) args.push('--credential-stdin');
  else if (config.hasApiKey) args.push('--api-key-stdin');
  if (config.trusted) args.push('--trusted-workspace');
  if (config.packagedCredentialBoundary) args.push('--packaged-credential-stdin-only');
  return args;
}

/** Serialize the one-shot credential payload consumed by bridge-server.
 *
 * Keep this boundary explicitly snake_case: it is a Rust serde contract, not
 * an Electron/JavaScript object passed over IPC.
 */
export function buildCredentialEnvelope(config: {
  apiKey?: string;
  providerCredentials?: Record<string, string>;
}): string {
  return `${JSON.stringify({
    api_key: config.apiKey ?? null,
    provider_keys: config.providerCredentials ?? {},
  })}\n`;
}

export function sanitizeDiagnostic(input: unknown, secrets: readonly string[] = []): string {
  let message = input instanceof Error ? input.message : String(input);
  for (const secret of secrets) {
    if (secret.length >= 4) message = message.split(secret).join('[REDACTED]');
  }
  message = message
    .replace(/(authorization|api[-_ ]?key|token|secret|password)(\s*[=:]\s*)([^\s,;]+)/gi, '$1$2[REDACTED]')
    .replace(/\b(sk-[A-Za-z0-9_-]{8,})\b/g, '[REDACTED]')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, ' ')
    .trim();
  return message.length <= MAX_DIAGNOSTIC_LENGTH
    ? message
    : `${message.slice(0, MAX_DIAGNOSTIC_LENGTH - 1)}…`;
}

function stableDiagnosticValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map((item) => stableDiagnosticValue(item));
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, stableDiagnosticValue(item)]),
    );
  }
  return value;
}

export function diagnosticEvent(event: string, details: Record<string, unknown>): string {
  const stableDetails = stableDiagnosticValue(details) as Record<string, unknown>;
  return sanitizeDiagnostic(JSON.stringify({
    event,
    ...stableDetails,
  }));
}

export class DiagnosticBuffer {
  private readonly entries: DiagnosticEntry[] = [];

  constructor(private readonly filePath?: string) {
    if (!filePath) return;
    try {
      if (statSync(filePath).size > 2_000_000) return;
      for (const line of readFileSync(filePath, 'utf8').split('\n').filter(Boolean).slice(-MAX_DIAGNOSTICS)) {
        const entry = JSON.parse(line) as Partial<DiagnosticEntry>;
        if (
          typeof entry.timestamp === 'string'
          && (entry.level === 'info' || entry.level === 'warn' || entry.level === 'error')
          && (entry.source === 'host' || entry.source === 'bridge')
          && typeof entry.message === 'string'
        ) {
          this.entries.push({
            timestamp: entry.timestamp,
            level: entry.level,
            source: entry.source,
            message: sanitizeDiagnostic(entry.message),
          });
        }
      }
    } catch {
      // Missing, corrupt, or unreadable logs start with a clean bounded buffer.
    }
  }

  add(level: DiagnosticEntry['level'], source: DiagnosticEntry['source'], input: unknown, secrets: readonly string[] = []): void {
    this.entries.push({ timestamp: new Date().toISOString(), level, source, message: sanitizeDiagnostic(input, secrets) });
    if (this.entries.length > MAX_DIAGNOSTICS) this.entries.splice(0, this.entries.length - MAX_DIAGNOSTICS);
    this.persist();
  }

  snapshot(): DiagnosticEntry[] {
    return this.entries.map((entry) => ({ ...entry }));
  }

  private persist(): void {
    if (!this.filePath) return;
    try {
      mkdirSync(dirname(this.filePath), { recursive: true, mode: 0o700 });
      const temporary = `${this.filePath}.tmp`;
      const body = this.entries.map((entry) => JSON.stringify(entry)).join('\n');
      writeFileSync(temporary, body ? `${body}\n` : '', { mode: 0o600 });
      renameSync(temporary, this.filePath);
    } catch {
      // Diagnostics must never destabilize the desktop host.
    }
  }
}
