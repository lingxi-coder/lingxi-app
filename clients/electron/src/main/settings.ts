import { mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { PermissionModeId } from '@lingxi/bridge-client';

import {
  defaultSettings,
  isPermissionModeId,
  parseModelPickerVisibility,
  parseNotificationPreferences,
  parseSettings,
  parseVoicePreferences,
  publicSettings,
  setWorkspaceTrust,
  withActiveProject,
  withAddedProject,
  withoutProject,
  withSessionPinned,
  workspaceTrust,
  type PinnedSessionRecord,
  type PersistedSettings,
  type PublicSettings,
  type SessionRef,
} from './host-utils.js';

export class SettingsStore {
  readonly settingsPath: string;
  private settings: PersistedSettings;
  // Canonical form of the managed scope. Every caller that compares a path
  // against it has already run it through `canonicalWorkspace` (which
  // realpaths), so a userData directory reached through a symlink — a
  // relocated home, a junction-redirected AppData, a `/var`->`/private/var`
  // test root — would otherwise never match and the app's own scheduled
  // workspace would be rejected as "not in the project list".
  private readonly managedWorkspace: string;
  private volatileActiveSession: SessionRef | undefined;

  constructor(userData: string) {
    this.settingsPath = join(userData, 'settings.v1.json');
    const managed = join(userData, 'scheduled-workspace');
    let canonical = managed;
    try {
      mkdirSync(managed, { recursive: true, mode: 0o700 });
      canonical = realpathSync.native(managed);
    } catch { /* Fall back to the literal path; requireProject still mkdirs. */ }
    this.managedWorkspace = canonical;
    this.settings = this.readSettings();
  }

  private readSettings(): PersistedSettings {
    try {
      const raw: unknown = JSON.parse(readFileSync(this.settingsPath, 'utf8'));
      const input = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw as Record<string, unknown> : {};
      const settings = parseSettings(raw);
      // Decode the managed scope separately so the Project limit and legacy
      // project migration cannot discard either user projects or generic chats.
      const managed = parseSettings({ ...input, projects: [this.scheduledWorkspace] });
      if (input['activeProject'] === this.scheduledWorkspace) settings.activeProject = this.scheduledWorkspace;
      if (managed.activeSession) settings.activeSession = managed.activeSession;
      settings.pinnedSessions.push(...managed.pinnedSessions);
      settings.archivedSessions = [...(settings.archivedSessions ?? []), ...(managed.archivedSessions ?? [])];
      if (settings.apiBaseUrl) {
        try { settings.apiBaseUrl = validateApiBaseUrl(settings.apiBaseUrl); }
        catch { delete settings.apiBaseUrl; }
      }
      return settings;
    } catch {
      return defaultSettings();
    }
  }

  private persist(): void {
    mkdirSync(dirname(this.settingsPath), { recursive: true, mode: 0o700 });
    const temporary = `${this.settingsPath}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(this.settings, null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.settingsPath);
  }

  getPublic(): PublicSettings {
    return publicSettings({
      ...this.settings,
      activeSession: this.volatileActiveSession
        ? { ...this.volatileActiveSession }
        : this.settings.activeSession ? { ...this.settings.activeSession } : undefined,
    });
  }

  getWorkspace(): string | undefined {
    return this.settings.activeProject;
  }

  addProject(canonical: string): void {
    this.settings = withAddedProject(this.settings, canonical);
    this.persist();
  }

  activateProject(canonical: string): void {
    this.settings = canonical === this.scheduledWorkspace
      ? { ...this.settings, activeProject: canonical } : withActiveProject(this.settings, canonical);
    this.persist();
  }

  setActiveSession(ref: SessionRef | undefined): PublicSettings {
    const previousSettings = this.settings;
    const previousVolatileActiveSession = this.volatileActiveSession;
    if (ref !== undefined) {
      this.validateSessionRef(ref);
      this.settings = { ...this.settings, activeSession: { ...ref } };
    } else {
      const { activeSession: _activeSession, ...settings } = this.settings;
      this.settings = settings;
    }
    try {
      this.persist();
    } catch (error) {
      this.settings = previousSettings;
      this.volatileActiveSession = previousVolatileActiveSession;
      throw error;
    }
    this.volatileActiveSession = undefined;
    return this.getPublic();
  }

  setActiveSessionDraft(ref: SessionRef | undefined): PublicSettings {
    if (ref !== undefined) {
      this.validateSessionRef(ref);
      this.volatileActiveSession = { ...ref };
    } else {
      this.volatileActiveSession = undefined;
    }
    return this.getPublic();
  }

  get scheduledWorkspace(): string {
    return this.managedWorkspace;
  }

  isTrustedWorkspace(path: string): boolean {
    return path === this.scheduledWorkspace || this.hasProject(path);
  }

  hasProject(canonical: string): boolean {
    return this.settings.projects.includes(canonical);
  }

  removeProject(projectPath: string): void {
    this.settings = withoutProject(this.settings, projectPath);
    if (this.volatileActiveSession?.projectPath === projectPath) this.volatileActiveSession = undefined;
    this.persist();
  }

  isSessionArchived(ref: SessionRef): boolean {
    return this.settings.archivedSessions?.some((item) => item.projectPath === ref.projectPath && item.sessionId === ref.sessionId) ?? false;
  }

  setSessionArchived(ref: SessionRef, archived: boolean, title?: string): void {
    this.validateSessionRef(ref);
    const previous = this.settings;
    const previousDraft = this.volatileActiveSession;
    const matches = (item: SessionRef) => item.projectPath === ref.projectPath && item.sessionId === ref.sessionId;
    this.settings = { ...previous, archivedSessions: (previous.archivedSessions ?? []).filter((item) => !matches(item)) };
    if (archived) {
      this.settings.archivedSessions!.push({ ...ref, ...(title ? { title: title.slice(0, 512) } : {}), archivedAt: new Date().toISOString() });
      this.settings.pinnedSessions = previous.pinnedSessions.filter((item) => !matches(item));
      if (this.settings.activeSession && matches(this.settings.activeSession)) delete this.settings.activeSession;
      if (this.volatileActiveSession && matches(this.volatileActiveSession)) this.volatileActiveSession = undefined;
    }
    try { this.persist(); }
    catch (error) { this.settings = previous; this.volatileActiveSession = previousDraft; throw error; }
  }

  setSessionPinned(session: PinnedSessionRecord, pinned: boolean): PublicSettings {
    if (pinned && this.isSessionArchived(session)) throw new Error('Restore this archived chat before pinning it.');
    const projects = this.settings.projects;
    const scoped = session.projectPath === this.scheduledWorkspace ? { ...this.settings, projects: [...projects, this.scheduledWorkspace] } : this.settings;
    this.settings = { ...withSessionPinned(scoped, session, pinned), projects };
    this.persist();
    return this.getPublic();
  }

  update(patch: {
    theme?: 'dark' | 'light' | 'system';
    collapseThoughtsByDefault?: boolean;
    model?: string | null;
    apiBaseUrl?: string | null;
    voice?: unknown;
    notifications?: unknown;
    modelPickerVisibility?: unknown;
  }): PublicSettings {
    if ('collapseThoughtsByDefault' in patch && typeof patch.collapseThoughtsByDefault !== 'boolean') {
      throw new Error('invalid collapseThoughtsByDefault');
    }
    if ('theme' in patch) {
      if (patch.theme !== 'dark' && patch.theme !== 'light' && patch.theme !== 'system') {
        throw new Error('invalid theme');
      }
      this.settings.theme = patch.theme;
    }
    if ('model' in patch) {
      if (patch.model === null || patch.model === '') delete this.settings.model;
      else this.settings.model = validateString(patch.model, 'model', 256);
    }
    if ('apiBaseUrl' in patch) {
      if (patch.apiBaseUrl === null || patch.apiBaseUrl === '') delete this.settings.apiBaseUrl;
      else this.settings.apiBaseUrl = validateApiBaseUrl(patch.apiBaseUrl);
    }
    if ('voice' in patch) {
      // Whole-object replace, normalized leniently — matches how both
      // mobile platforms persist voice preferences (iOS's `persist()`,
      // Android's `save()` each write the full snapshot at once, never a
      // partial merge of individual fields).
      this.settings.voice = parseVoicePreferences(patch.voice);
    }
    if ('notifications' in patch) {
      // Whole-object replace, normalized leniently — same reasoning as `voice`
      // above, and the same reason `parseNotificationPreferences` never turns a
      // gate OFF for a malformed field: a corrupt value must not silently
      // disable notifications, the one failure the settings page cannot show.
      this.settings.notifications = parseNotificationPreferences(patch.notifications);
    }
    if ('modelPickerVisibility' in patch) {
      const parsed = parseModelPickerVisibility(patch.modelPickerVisibility);
      if (Object.keys(parsed).length > 0) this.settings.modelPickerVisibility = parsed;
      else delete this.settings.modelPickerVisibility;
    }
    if (typeof patch.collapseThoughtsByDefault === 'boolean') {
      this.settings.collapseThoughtsByDefault = patch.collapseThoughtsByDefault;
    }
    this.persist();
    return this.getPublic();
  }

  getTrust(workspace: string): { trusted: boolean; fingerprint: string } {
    return workspaceTrust(this.settings, workspace);
  }

  setTrust(workspace: string, trusted: boolean): { trusted: boolean; fingerprint: string } {
    this.settings = setWorkspaceTrust(this.settings, workspace, trusted);
    this.persist();
    return workspaceTrust(this.settings, workspace);
  }

  /** Commit the confirmed model without changing the in-memory default on write failure. */
  setLastModel(model: string): void {
    const validated = validateString(model, 'model', 256);
    const previous = this.settings;
    this.settings = { ...previous, model: validated };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** Restore Bypass only after the existing one-time acknowledgement. */
  getLastPermissionMode(): PermissionModeId | undefined {
    const mode = this.settings.lastPermissionMode;
    return mode === 'bypassPermissions' && !this.getBypassPermissionsAccepted() ? undefined : mode;
  }

  setLastPermissionMode(mode: PermissionModeId): void {
    if (!isPermissionModeId(mode)) throw new Error('invalid permission mode');
    const previous = this.settings;
    this.settings = { ...previous, lastPermissionMode: mode };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** Whether the user has previously accepted Bypass Permissions mode. */
  getBypassPermissionsAccepted(): boolean {
    return this.settings.bypassPermissionsModeAccepted === true;
  }

  /** Persist the one-time Bypass Permissions acceptance (oracle
   * `bypassPermissionsModeAccepted`). */
  setBypassPermissionsAccepted(accepted: boolean): void {
    if (accepted) this.settings.bypassPermissionsModeAccepted = true;
    else delete this.settings.bypassPermissionsModeAccepted;
    this.persist();
  }

  private validateSessionRef(ref: SessionRef): void {
    if (!this.isTrustedWorkspace(ref.projectPath)) throw new Error('project is not in the project list');
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(ref.sessionId)) {
      throw new Error('invalid session id');
    }
  }

}

function validateString(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function validateApiBaseUrl(value: unknown): string {
  const raw = validateString(value, 'API base URL', 2_048);
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error('invalid API base URL');
  }
  const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '::1';
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
    throw new Error('API base URL must use HTTPS (HTTP is allowed only for loopback)');
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error('API base URL must not contain credentials, query parameters, or fragments');
  }
  return url.toString().replace(/\/$/, '');
}
