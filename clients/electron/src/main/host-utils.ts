import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve } from 'node:path';

import type { PermissionModeId } from '@lingxi/bridge-client';

import { SETTINGS_VERSION } from '../shared/settings.js';
import type {
  ModelPickerVisibilitySettings,
  PinnedSessionRecord,
  ProviderModelPickerVisibility,
  PublicSettings,
  SessionRef,
} from '../shared/settings.js';
import { parseVoicePreferences } from '../shared/voicePreferences.js';
import type { VoicePreferences } from '../shared/voicePreferences.js';
import { parseNotificationPreferences } from '../shared/notificationPreferences.js';
import type { NotificationPreferences } from '../shared/notificationPreferences.js';

export { SETTINGS_VERSION } from '../shared/settings.js';
export type {
  PinnedSessionRecord,
  PublicSettings,
  SessionPinInput,
  SessionRef,
} from '../shared/settings.js';
export type { VoicePreferences } from '../shared/voicePreferences.js';
export { parseVoicePreferences } from '../shared/voicePreferences.js';
export type { NotificationPreferences } from '../shared/notificationPreferences.js';
export { parseNotificationPreferences } from '../shared/notificationPreferences.js';

export const MAX_PROJECTS = 50;
export const MAX_PINNED_SESSIONS = 100;
export const MAX_DIAGNOSTICS = 200;
export const MAX_DIAGNOSTIC_LENGTH = 2_000;
export const MAX_TRUST_FINGERPRINT_ENTRIES = 512;
export const MAX_TRUST_FINGERPRINT_BYTES = 512 * 1024;
export const MAX_TRUST_FINGERPRINT_DEPTH = 12;
export const MAX_MODEL_PICKER_PROVIDERS = 128;
export const MAX_MODEL_PICKER_MODELS_PER_PROVIDER = 512;
export const MAX_PROVIDER_ID_LENGTH = 256;
export const MAX_MODEL_ID_LENGTH = 512;

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
  archivedSessions?: import('../shared/settings.js').ArchivedSessionRecord[];
  version: typeof SETTINGS_VERSION;
  theme?: 'dark' | 'light' | 'system';
  collapseThoughtsByDefault?: boolean;
  model?: string;
  /** Last user-selected permission mode on this device. */
  lastPermissionMode?: PermissionModeId;
  apiBaseUrl?: string;
  activeProject?: string;
  activeSession?: SessionRef;
  projects: string[];
  pinnedSessions: PinnedSessionRecord[];
  trustedWorkspaces: Record<string, TrustRecord>;
  /** One-time acknowledgement that the user accepted Bypass Permissions mode
   * (oracle `bypassPermissionsModeAccepted`). Persisted so the blocking
   * acceptance dialog is shown ONCE, not on every activation. */
  bypassPermissionsModeAccepted?: boolean;
  /** Voice recognition/synthesis preferences — see `shared/voicePreferences.ts`.
   * Omitted (not defaulted) until the first `SettingsStore.update({ voice })` call. */
  voice?: VoicePreferences;
  /** OS-notification preferences — see `shared/notificationPreferences.ts`.
   * Omitted until the first `SettingsStore.update({ notifications })`. */
  notifications?: NotificationPreferences;
  /** Device-local conversation model picker visibility by provider id. */
  modelPickerVisibility?: ModelPickerVisibilitySettings;
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
  return {
    version: SETTINGS_VERSION,
    projects: [],
    pinnedSessions: [],
    trustedWorkspaces: {},
  };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function boundedString(value: unknown, max: number): string | undefined {
  return typeof value === 'string' && value.length > 0 && value.length <= max ? value : undefined;
}

function boundedStringArray(value: unknown, maxItems: number): string[] {
  if (!Array.isArray(value)) return [];
  const unique = new Set<string>();
  for (const item of value) {
    const path = boundedString(item, 32_768);
    if (path) unique.add(path);
    if (unique.size >= maxItems) break;
  }
  return [...unique];
}

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function isPermissionModeId(value: unknown): value is PermissionModeId {
  return value === 'default' || value === 'acceptEdits' || value === 'plan'
    || value === 'auto' || value === 'dontAsk' || value === 'bypassPermissions';
}

export function parseSettings(value: unknown): PersistedSettings {
  if (!isPlainObject(value) || value['version'] !== SETTINGS_VERSION) {
    return defaultSettings();
  }

  const settings = defaultSettings();
  settings.theme =
    value['theme'] === 'dark' || value['theme'] === 'light' || value['theme'] === 'system'
      ? value['theme']
      : undefined;
  if (typeof value['collapseThoughtsByDefault'] === 'boolean') {
    settings.collapseThoughtsByDefault = value['collapseThoughtsByDefault'];
  }
  settings.model = boundedString(value['model'], 256);
  if (isPermissionModeId(value['lastPermissionMode'])) {
    settings.lastPermissionMode = value['lastPermissionMode'];
  }
  settings.apiBaseUrl = boundedString(value['apiBaseUrl'], 2_048);
  if (value['bypassPermissionsModeAccepted'] === true) {
    settings.bypassPermissionsModeAccepted = true;
  }
  if (value['voice'] !== undefined) {
    settings.voice = parseVoicePreferences(value['voice']);
  }
  if (value['notifications'] !== undefined) {
    settings.notifications = parseNotificationPreferences(value['notifications']);
  }
  if (value['modelPickerVisibility'] !== undefined) {
    settings.modelPickerVisibility = parseModelPickerVisibility(value['modelPickerVisibility']);
  }

  const legacyActive = boundedString(value['lastWorkspace'], 32_768);
  const persistedProjects = boundedStringArray(value['projects'], MAX_PROJECTS);
  const legacyProjects = boundedStringArray(value['recentWorkspaces'], MAX_PROJECTS);
  settings.projects = Array.isArray(value['projects'])
    ? persistedProjects
    : boundedStringArray([legacyActive, ...legacyProjects], MAX_PROJECTS);
  const requestedActive = boundedString(value['activeProject'], 32_768) ?? legacyActive;
  settings.activeProject = requestedActive && settings.projects.includes(requestedActive)
    ? requestedActive
    : settings.projects[0];

  if (isPlainObject(value['activeSession'])) {
    const projectPath = boundedString(value['activeSession']['projectPath'], 32_768);
    const sessionId = boundedString(value['activeSession']['sessionId'], 64);
    if (projectPath && sessionId && settings.projects.includes(projectPath) && SESSION_ID_PATTERN.test(sessionId)) {
      settings.activeSession = { projectPath, sessionId };
    }
  }

  if (Array.isArray(value['archivedSessions'])) {
    settings.archivedSessions = [];
    for (const item of value['archivedSessions']) {
      if (!isPlainObject(item)) continue;
      const projectPath = boundedString(item['projectPath'], 32_768);
      const sessionId = boundedString(item['sessionId'], 64);
      if (!projectPath || !sessionId || !settings.projects.includes(projectPath) || !SESSION_ID_PATTERN.test(sessionId)) continue;
      if (settings.archivedSessions.some((item) => item.projectPath === projectPath && item.sessionId === sessionId)) continue;
      settings.archivedSessions.push({ projectPath, sessionId, title: boundedString(item['title'], 512), archivedAt: boundedString(item['archivedAt'], 64) });
    }
    if (settings.activeSession && settings.archivedSessions.some((item) => item.projectPath === settings.activeSession?.projectPath && item.sessionId === settings.activeSession?.sessionId)) delete settings.activeSession;
  }
  if (Array.isArray(value['pinnedSessions'])) {
    const seen = new Set<string>();
    for (const item of value['pinnedSessions']) {
      if (!isPlainObject(item)) continue;
      const projectPath = boundedString(item['projectPath'], 32_768);
      const sessionId = boundedString(item['sessionId'], 64);
      const title = boundedString(item['title'], 512);
      const pinnedAt = boundedString(item['pinnedAt'], 64);
      if (!projectPath || !settings.projects.includes(projectPath) || !sessionId
        || !SESSION_ID_PATTERN.test(sessionId) || !title || !pinnedAt
        || Number.isNaN(Date.parse(pinnedAt))) continue;
      const key = `${projectPath}\0${sessionId}`;
      if (seen.has(key)) continue;
      seen.add(key);
      settings.pinnedSessions.push({ projectPath, sessionId, title, pinnedAt });
      if (settings.pinnedSessions.length >= MAX_PINNED_SESSIONS) break;
    }
    settings.pinnedSessions.sort((left, right) => right.pinnedAt.localeCompare(left.pinnedAt));
  }
  if (isPlainObject(value['trustedWorkspaces'])) {
    for (const [workspace, record] of Object.entries(value['trustedWorkspaces'])) {
      if (workspace.length > 32_768 || !isPlainObject(record)) continue;
      const fingerprint = boundedString(record['fingerprint'], 128);
      const trustedAt = boundedString(record['trustedAt'], 64);
      if (fingerprint && trustedAt) settings.trustedWorkspaces[workspace] = { fingerprint, trustedAt };
    }
  }
  settings.pinnedSessions = settings.pinnedSessions.filter((pin) => !settings.archivedSessions?.some((item) => item.projectPath === pin.projectPath && item.sessionId === pin.sessionId));
  return settings;
}

export function publicSettings(settings: PersistedSettings): PublicSettings {
  const {
    version, theme, model, apiBaseUrl, activeProject, activeSession, projects, pinnedSessions,
    bypassPermissionsModeAccepted, voice, notifications, modelPickerVisibility, collapseThoughtsByDefault,
  } = settings;
  return {
    version,
    theme,
    model,
    apiBaseUrl,
    activeProject,
    activeSession: activeSession ? { ...activeSession } : undefined,
    projects: [...projects],
    ...(settings.archivedSessions ? { archivedSessions: settings.archivedSessions.map((item) => ({ ...item })) } : {}),
    pinnedSessions: pinnedSessions.map((session) => ({ ...session })),
    ...(typeof collapseThoughtsByDefault === 'boolean' ? { collapseThoughtsByDefault } : {}),
    ...(bypassPermissionsModeAccepted ? { bypassPermissionsModeAccepted: true } : {}),
    ...(voice ? { voice: { ...voice } } : {}),
    ...(notifications ? { notifications: { ...notifications } } : {}),
    ...(modelPickerVisibility ? { modelPickerVisibility: cloneModelPickerVisibility(modelPickerVisibility) } : {}),
  };
}

function parseProviderModelPickerVisibility(value: unknown): ProviderModelPickerVisibility | undefined {
  if (!isPlainObject(value)) return undefined;
  const parsed: ProviderModelPickerVisibility = {};
  if (value['showInModelPicker'] === true || value['showInModelPicker'] === false) {
    parsed.showInModelPicker = value['showInModelPicker'];
  }
  if (value['visibleModelIds'] === null) {
    parsed.visibleModelIds = undefined;
  } else if (Array.isArray(value['visibleModelIds'])) {
    const ids: string[] = [];
    const seen = new Set<string>();
    for (const item of value['visibleModelIds']) {
      const id = boundedString(item, MAX_MODEL_ID_LENGTH);
      if (!id || seen.has(id)) continue;
      seen.add(id);
      ids.push(id);
      if (ids.length >= MAX_MODEL_PICKER_MODELS_PER_PROVIDER) break;
    }
    parsed.visibleModelIds = ids;
  }
  return parsed;
}

export function parseModelPickerVisibility(value: unknown): ModelPickerVisibilitySettings {
  if (!isPlainObject(value)) return {};
  const out: ModelPickerVisibilitySettings = {};
  for (const [providerId, rawVisibility] of Object.entries(value)) {
    const boundedProviderId = boundedString(providerId, MAX_PROVIDER_ID_LENGTH);
    if (!boundedProviderId) continue;
    const parsed = parseProviderModelPickerVisibility(rawVisibility);
    if (!parsed) continue;
    out[boundedProviderId] = parsed;
    if (Object.keys(out).length >= MAX_MODEL_PICKER_PROVIDERS) break;
  }
  return out;
}

function cloneModelPickerVisibility(
  value: ModelPickerVisibilitySettings,
): ModelPickerVisibilitySettings {
  return Object.fromEntries(
    Object.entries(value).map(([providerId, visibility]) => [
      providerId,
      {
        ...(visibility.showInModelPicker !== undefined
          ? { showInModelPicker: visibility.showInModelPicker }
          : {}),
        ...(visibility.visibleModelIds !== undefined
          ? { visibleModelIds: [...visibility.visibleModelIds] }
          : {}),
      },
    ]),
  );
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

export function withAddedProject(settings: PersistedSettings, projectPath: string): PersistedSettings {
  const existing = settings.projects.includes(projectPath);
  if (!existing && settings.projects.length >= MAX_PROJECTS) {
    throw new Error(`Project limit reached (${MAX_PROJECTS}). Remove a project before adding another.`);
  }
  return {
    ...settings,
    activeProject: settings.activeProject ?? projectPath,
    activeSession: settings.activeSession ? { ...settings.activeSession } : undefined,
    projects: existing ? [...settings.projects] : [projectPath, ...settings.projects],
    pinnedSessions: settings.pinnedSessions.map((session) => ({ ...session })),
    trustedWorkspaces: { ...settings.trustedWorkspaces },
  };
}

export function withActiveProject(settings: PersistedSettings, projectPath: string): PersistedSettings {
  if (!settings.projects.includes(projectPath)) throw new Error('project is not in the project list');
  return {
    ...settings,
    activeProject: projectPath,
    activeSession: settings.activeSession ? { ...settings.activeSession } : undefined,
    projects: [...settings.projects],
    pinnedSessions: settings.pinnedSessions.map((session) => ({ ...session })),
    trustedWorkspaces: { ...settings.trustedWorkspaces },
  };
}

export function withoutProject(settings: PersistedSettings, projectPath: string): PersistedSettings {
  if (!settings.projects.includes(projectPath)) throw new Error('project is not in the project list');
  const projects = settings.projects.filter((path) => path !== projectPath);
  const trustedWorkspaces = { ...settings.trustedWorkspaces };
  delete trustedWorkspaces[projectPath];
  const activeSession = settings.activeSession?.projectPath === projectPath
    ? undefined
    : settings.activeSession ? { ...settings.activeSession } : undefined;
  return {
    ...settings,
    activeProject: settings.activeProject === projectPath ? projects[0] : settings.activeProject,
    activeSession,
    projects,
    pinnedSessions: settings.pinnedSessions
      .filter((session) => session.projectPath !== projectPath)
      .map((session) => ({ ...session })),
    trustedWorkspaces,
  };
}

export function withSessionPinned(
  settings: PersistedSettings,
  session: PinnedSessionRecord,
  pinned: boolean,
): PersistedSettings {
  if (!settings.projects.includes(session.projectPath)) throw new Error('project is not in the project list');
  const matches = (item: PinnedSessionRecord): boolean => (
    item.projectPath === session.projectPath && item.sessionId === session.sessionId
  );
  const existing = settings.pinnedSessions.filter((item) => !matches(item));
  if (pinned && existing.length >= MAX_PINNED_SESSIONS) {
    throw new Error(`Pinned session limit reached (${MAX_PINNED_SESSIONS}). Unpin a session before adding another.`);
  }
  return {
    ...settings,
    projects: [...settings.projects],
    activeSession: settings.activeSession ? { ...settings.activeSession } : undefined,
    pinnedSessions: pinned ? [{ ...session }, ...existing] : existing,
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

export const ENV_ALLOWLIST = [
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
  sessionId?: string;
  listSessionsJson?: boolean;
  model?: string;
  hasApiKey: boolean;
  hasCredentialStdin?: boolean;
  trusted: boolean;
  packagedCredentialBoundary?: boolean;
}): string[] {
  if (config.listSessionsJson) return ['--cwd', config.workspace, '--list-sessions-json'];
  const args = ['--cwd', config.workspace, '--bridge-dir', config.bridgeDir];
  if (config.sessionId) args.push('--session-id', config.sessionId);
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
export interface OpenAiOAuthSession {
  access_token: string;
  refresh_token?: string;
  expires_at: number;
  account_id?: string;
  fedramp: boolean;
}

export function buildCredentialEnvelope(config: {
  openaiOAuth?: OpenAiOAuthSession;
  apiKey?: string;
  providerCredentials?: Record<string, string>;
  pluginSecrets?: Record<string, Record<string, string>>;
}): string {
  return `${JSON.stringify({
    api_key: config.apiKey ?? null,
    provider_keys: config.providerCredentials ?? {},
    plugin_secrets: config.pluginSecrets ?? {},
    ...(config.openaiOAuth ? { openai_oauth: config.openaiOAuth } : {}),
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
