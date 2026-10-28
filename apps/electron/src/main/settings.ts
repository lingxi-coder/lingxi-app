import { mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, join } from 'node:path';
import type { PermissionModeId } from '@lingxi/bridge-client';
import { isInside } from './bots/files.js';
import { isSessionId } from './sessionIdentity.js';

import {
  MAX_REMOTE_SESSIONS,
  SETTINGS_VERSION,
  defaultSettings,
  isPermissionModeId,
  parseModelPickerVisibility,
  parseNotificationPreferences,
  parseSidebarPreferences,
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
  type SessionTranscriptSelection,
} from './host-utils.js';

import { resolveDesktopTelemetry, type DesktopTelemetryConfig } from './telemetryPolicy.js';

export class SettingsStore {
  readonly settingsPath: string;
  private settings: PersistedSettings;
  private audioConfigurationError: string | undefined;
  private settingsWritable = true;
  private volatileActiveSession: SessionRef | undefined;
  // Canonical form of the managed scope. Every caller that compares a path
  // against it has already run it through `canonicalWorkspace` (which
  // realpaths), so a userData directory reached through a symlink — a
  // relocated home, a junction-redirected AppData, a `/var`->`/private/var`
  // test root — would otherwise never match and the app's own scheduled
  // workspace would be rejected as "not in the project list".
  private readonly managedWorkspace: string;
  // The app's own data folder, as a real path when it can be resolved.
  private readonly userDataFolder: string;

  constructor(userData: string) {
    this.settingsPath = join(userData, 'settings.v1.json');
    const managed = join(userData, 'scheduled-workspace');
    let canonical = managed;
    let folder = userData;
    try {
      mkdirSync(managed, { recursive: true, mode: 0o700 });
      canonical = realpathSync.native(managed);
      folder = realpathSync.native(userData);
    } catch { /* Fall back to the literal path; requireProject still mkdirs. */ }
    this.managedWorkspace = canonical;
    this.userDataFolder = folder;
    this.settings = this.readSettings();
  }

  private readSettings(): PersistedSettings {
    let contents: string;
    try {
      contents = readFileSync(this.settingsPath, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return defaultSettings();
      this.settingsWritable = false;
      this.audioConfigurationError = 'Device settings cannot be read. Audio is unavailable until the original settings file is restored.';
      return defaultSettings();
    }
    try {
      const raw: unknown = JSON.parse(contents);
      if (!raw || typeof raw !== 'object' || Array.isArray(raw)
        || (raw as Record<string, unknown>)['version'] !== SETTINGS_VERSION) {
        throw new Error('Unsupported device settings format');
      }
      const input = raw as Record<string, unknown>;
      const settings = parseSettings(raw);
      // Decode the managed scope separately so the Project limit cannot discard
      // sessions belonging to the app's scheduled workspace or user projects.
      const managed = parseSettings({ ...input, projects: [this.scheduledWorkspace] });
      if (input['activeProject'] === this.scheduledWorkspace) settings.activeProject = this.scheduledWorkspace;
      if (managed.activeSession) settings.activeSession = managed.activeSession;
      settings.pinnedSessions.push(...managed.pinnedSessions);
      settings.archivedSessions = [...(settings.archivedSessions ?? []), ...(managed.archivedSessions ?? [])];
      if (managed.sessionTranscriptSelections?.length) settings.sessionTranscriptSelections = [
        ...(settings.sessionTranscriptSelections ?? []), ...managed.sessionTranscriptSelections,
      ];
      if (settings.apiBaseUrl) {
        try { settings.apiBaseUrl = validateApiBaseUrl(settings.apiBaseUrl); }
        catch { delete settings.apiBaseUrl; }
      }
      return settings;
    } catch {
      this.settingsWritable = false;
      this.audioConfigurationError = 'Device settings are unreadable. Audio is unavailable until the original settings file is restored.';
      return defaultSettings();
    }
  }

  private persist(): void {
    if (!this.settingsWritable) throw new Error(this.audioConfigurationError ?? 'Device settings cannot be safely updated.');
    mkdirSync(dirname(this.settingsPath), { recursive: true, mode: 0o700 });
    const temporary = `${this.settingsPath}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(this.settings, null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.settingsPath);
  }

  getTelemetryConfig(source: NodeJS.ProcessEnv = process.env): DesktopTelemetryConfig {
    return resolveDesktopTelemetry(source, this.settingsWritable ? this.settings : { telemetryEnabled: false });
  }

  getPublic(): PublicSettings {
    const settings = publicSettings({
      ...this.settings,
      activeSession: this.volatileActiveSession
        ? { ...this.volatileActiveSession }
        : this.settings.activeSession ? { ...this.settings.activeSession } : undefined,
    });
    if (settings.voice) settings.voiceRevision = this.settings.audioConfigurationRevision ?? 0;
    if (this.audioConfigurationError) settings.audioConfigurationError = this.audioConfigurationError;
    return settings;
  }

  getAudioConfigurationError(): string | undefined {
    return this.audioConfigurationError;
  }

  getWorkspace(): string | undefined {
    return this.settings.activeProject;
  }

  /** Read the strict local Insights policy for `/insights` (defaults enabled). */
  getInsightsAllowed(): boolean {
    return this.settings.allowInsights ?? true;
  }

  /** Local team-onboarding policy; default allow does not imply workspace trust. */
  getTeamOnboardingAllowed(): boolean {
    return this.settings.allowTeamOnboarding ?? true;
  }

  addProject(canonical: string): void {
    const previous = this.settings;
    this.settings = withAddedProject(this.settings, canonical);
    try { this.persist(); }
    catch (error) { this.settings = previous; throw error; }
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

  /**
   * Show a session of a Bot in the window. The Bot's folder is not a project, so
   * the reference is not checked against the project list; the caller has
   * established that it is a Bot's. It lives in memory only, like an unsent
   * draft: the session the app reopens at startup is the owner's own.
   */
  setActiveBotSession(ref: SessionRef): PublicSettings {
    this.volatileActiveSession = { ...ref };
    return this.getPublic();
  }

  /** Stop showing the in-memory session if it is one of this folder's (a Bot that is gone); the saved one shows again. */
  dropVolatileSessionIn(folder: string): PublicSettings {
    if (this.volatileActiveSession && this.volatileActiveSession.projectPath === folder) this.volatileActiveSession = undefined;
    return this.getPublic();
  }

  /** Move host metadata with the transcript's stable UUID in one settings write. */
  relocateSession(ref: SessionRef, projectPath: string, selection?: Omit<SessionTranscriptSelection, keyof SessionRef> | null): void {
    if (selection && (!isAbsolute(selection.catalogProjectPath) || selection.catalogProjectPath.length > 32_768
      || selection.catalogProjectPath.includes('\0') || !isAbsolute(selection.transcriptPath)
      || selection.transcriptPath.length > 32_768 || selection.transcriptPath.includes('\0')
      || basename(selection.transcriptPath).toLowerCase() !== `${ref.sessionId.toLowerCase()}.jsonl`
      || !['current', 'repository', 'all'].includes(selection.scope))) throw new Error('invalid session transcript selection');
    const previous = this.settings;
    const previousDraft = this.volatileActiveSession;
    const moved = { projectPath, sessionId: ref.sessionId };
    const matches = (entry: SessionRef) => entry.sessionId === ref.sessionId && entry.projectPath === ref.projectPath;
    const relocate = <T extends SessionRef>(entry: T): T => matches(entry) ? { ...entry, projectPath } : entry;
    const next = withAddedProject(previous, projectPath);
    next.activeProject = projectPath;
    // Unsent drafts have no transcript to restore after a process restart.
    // Move their visible owner without making the volatile UUID durable.
    next.activeSession = previousDraft && matches(previousDraft) ? previous.activeSession : moved;
    next.pinnedSessions = next.pinnedSessions.map(relocate);
    next.archivedSessions = next.archivedSessions?.map(relocate);
    next.sessionModels = next.sessionModels?.map(relocate);
    const remembered = previous.sessionTranscriptSelections?.find(matches)
      ?? previous.sessionTranscriptSelections?.find((entry) => entry.projectPath === projectPath && entry.sessionId === ref.sessionId);
    const selected = selection === undefined ? remembered : selection;
    if (previous.sessionTranscriptSelections || selected) {
      next.sessionTranscriptSelections = (previous.sessionTranscriptSelections ?? []).filter((entry) => !matches(entry)
        && !(entry.projectPath === projectPath && entry.sessionId === ref.sessionId));
      if (selected) next.sessionTranscriptSelections.push({ ...selected, ...moved });
    }
    next.remoteSessions = next.remoteSessions?.map(relocate);
    if (next.sidebar && ref.projectPath !== projectPath) {
      const orders = { ...next.sidebar.manualSessionOrder };
      const oldOrder = orders[ref.projectPath] ?? [];
      const index = oldOrder.indexOf(ref.sessionId);
      orders[ref.projectPath] = oldOrder.filter((id) => id !== ref.sessionId);
      if (index >= 0) {
        const target = (orders[projectPath] ?? []).filter((id) => id !== ref.sessionId);
        target.splice(Math.min(index, target.length), 0, ref.sessionId);
        orders[projectPath] = target;
      }
      next.sidebar = { ...next.sidebar, manualSessionOrder: orders };
    }
    this.settings = next;
    this.volatileActiveSession = previousDraft && matches(previousDraft) ? moved : undefined;
    try { this.persist(); }
    catch (error) {
      this.settings = previous;
      this.volatileActiveSession = previousDraft;
      throw error;
    }
  }

  getSessionTranscriptSelection(ref: SessionRef): SessionTranscriptSelection | undefined {
    const selected = this.settings.sessionTranscriptSelections?.find((entry) => entry.projectPath === ref.projectPath && entry.sessionId === ref.sessionId);
    return selected ? { ...selected } : undefined;
  }

  /** Validate host capacity before asking the engine to migrate files. */
  assertSessionRelocationAllowed(projectPath: string): void {
    if (!this.settingsWritable) throw new Error(this.audioConfigurationError ?? 'Device settings cannot be safely updated.');
    withAddedProject(this.settings, projectPath);
  }

  get scheduledWorkspace(): string {
    return this.managedWorkspace;
  }

  isTrustedWorkspace(path: string): boolean {
    return path === this.scheduledWorkspace || this.hasProject(path);
  }

  /**
   * Whether a folder belongs to the app's own data (its settings, logs and
   * credentials, and the Bots), which is no place for a project of the owner's.
   * The scheduled workspace is in there, and is one the app manages itself.
   */
  isAppDataFolder(canonical: string): boolean {
    return isInside(canonical, this.userDataFolder) && !isInside(canonical, this.managedWorkspace);
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
    telemetryEnabled?: boolean;
    theme?: 'dark' | 'light' | 'system';
    collapseThoughtsByDefault?: boolean;
    botsKeepAwake?: boolean;
    model?: string | null;
    apiBaseUrl?: string | null;
    voice?: unknown;
    voiceRevision?: number;
    notifications?: unknown;
    modelPickerVisibility?: unknown;
    sidebar?: unknown;
  }): PublicSettings {
    if ('telemetryEnabled' in patch) {
      if (typeof patch.telemetryEnabled !== 'boolean' || Object.keys(patch).length !== 1) throw new Error('diagnostics consent must be saved separately as a boolean');
      const previous = this.settings;
      this.settings = { ...previous, telemetryEnabled: patch.telemetryEnabled };
      try { this.persist(); } catch (error) { this.settings = previous; throw error; }
      return this.getPublic();
    }
    if ('voice' in patch) {
      if (Object.keys(patch).some((key) => key !== 'voice' && key !== 'voiceRevision')) {
        throw new Error('audio configuration must be saved separately from other settings');
      }
      const currentRevision = this.settings.audioConfigurationRevision ?? 0;
      if (!Number.isSafeInteger(patch.voiceRevision) || patch.voiceRevision !== currentRevision) {
        throw new Error('audio configuration changed in another view; reload it before saving');
      }
      if (currentRevision >= Number.MAX_SAFE_INTEGER) throw new Error('audio configuration revision is exhausted');
      const previous = this.settings;
      this.settings = {
        ...previous,
        voice: parseVoicePreferences(patch.voice),
        audioConfigurationRevision: currentRevision + 1,
      };
      try {
        this.persist();
      } catch (error) {
        this.settings = previous;
        throw error;
      }
      return this.getPublic();
    }
    if ('voiceRevision' in patch) throw new Error('audio configuration revision requires a voice value');
    if ('collapseThoughtsByDefault' in patch && typeof patch.collapseThoughtsByDefault !== 'boolean') {
      throw new Error('invalid collapseThoughtsByDefault');
    }
    if ('botsKeepAwake' in patch && typeof patch.botsKeepAwake !== 'boolean') {
      throw new Error('invalid botsKeepAwake');
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
    if ('notifications' in patch) {
      // Whole-object replace, normalized leniently — same reasoning as
      // `voice` above, and the same reason `parseNotificationPreferences`
      // never turns a gate OFF for a malformed field: a corrupt value must
      // not silently disable notifications, which is the one failure the
      // settings page cannot show the user.
      this.settings.notifications = parseNotificationPreferences(patch.notifications);
    }
    if ('modelPickerVisibility' in patch) {
      const parsed = parseModelPickerVisibility(patch.modelPickerVisibility);
      if (Object.keys(parsed).length > 0) this.settings.modelPickerVisibility = parsed;
      else delete this.settings.modelPickerVisibility;
    }
    if ('sidebar' in patch) {
      this.settings.sidebar = parseSidebarPreferences(patch.sidebar, this.settings.projects);
    }
    if (typeof patch.collapseThoughtsByDefault === 'boolean') {
      this.settings.collapseThoughtsByDefault = patch.collapseThoughtsByDefault;
    }
    if (typeof patch.botsKeepAwake === 'boolean') {
      this.settings.botsKeepAwake = patch.botsKeepAwake;
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

  getSessionModel(ref: SessionRef): string | undefined {
    return this.settings.sessionModels?.find(entry =>
      entry.sessionId === ref.sessionId && entry.projectPath === ref.projectPath)?.model;
  }

  /** Save the session choice and the default for new sessions atomically. */
  setSessionModel(ref: SessionRef, model: string): void {
    const validated = validateString(model, 'model', 256);
    const previous = this.settings;
    const entries = (previous.sessionModels ?? []).filter(entry =>
      entry.sessionId !== ref.sessionId || entry.projectPath !== ref.projectPath);
    this.settings = {
      ...previous,
      model: validated,
      sessionModels: [...entries, { ...ref, model: validated }],
    };
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

  /** Return the last explicitly selected Fast mode state, if any. */
  getLastFastMode(): boolean | undefined {
    return this.settings.fastMode;
  }

  /** Persist Fast mode only after the engine confirms the selection. */
  setLastFastMode(enabled: boolean): void {
    if (typeof enabled !== 'boolean') throw new Error('invalid fast mode');
    const previous = this.settings;
    this.settings = { ...previous, fastMode: enabled };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** Whether Remote Control is switched on for this computer; off by default. */
  getRemoteControlEnabled(): boolean {
    return this.settings.remoteControlEnabled === true;
  }

  /** Switch Remote Control on or off. The caller learns of a save failure; the setting then stays as it was. */
  setRemoteControlEnabled(enabled: boolean): void {
    if (typeof enabled !== 'boolean') throw new Error('invalid remote control setting');
    const previous = this.settings;
    this.settings = { ...previous, remoteControlEnabled: enabled };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** The sessions Remote Control shows to viewers. */
  getRemoteSessions(): SessionRef[] {
    return (this.settings.remoteSessions ?? []).map((ref) => ({ ...ref }));
  }

  isRemoteSession(sessionId: string): boolean {
    return this.settings.remoteSessions?.some((ref) => ref.sessionId === sessionId) ?? false;
  }

  /** Switch Remote Control on or off for one session. A save failure leaves the list as it was. */
  setRemoteSession(ref: SessionRef, enabled: boolean): void {
    if (typeof enabled !== 'boolean') throw new Error('invalid remote control setting');
    this.validateSessionRef(ref);
    const previous = this.settings;
    const others = (previous.remoteSessions ?? []).filter((item) => item.sessionId !== ref.sessionId);
    if (enabled && others.length >= MAX_REMOTE_SESSIONS) throw new Error('too many sessions have Remote Control on');
    this.settings = { ...previous, remoteSessions: enabled ? [...others, { projectPath: ref.projectPath, sessionId: ref.sessionId }] : others };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** Whether every new session starts with Remote Control on; off by default. */
  getRemoteControlAtStartup(): boolean {
    return this.settings.remoteControlAtStartup === true;
  }

  setRemoteControlAtStartup(enabled: boolean): void {
    if (typeof enabled !== 'boolean') throw new Error('invalid remote control setting');
    const previous = this.settings;
    this.settings = { ...previous, remoteControlAtStartup: enabled };
    try {
      this.persist();
    } catch (error) {
      this.settings = previous;
      throw error;
    }
  }

  /** Whether a viewer may answer a permission prompt with "always allow". Only the settings file can turn this on. */
  getRemoteAllowAlwaysAllow(): boolean {
    return this.settings.remoteAllowAlwaysAllow === true;
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
    if (!isSessionId(ref.sessionId)) {
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
