import type { DesktopLocalApp, LocalAppAuthoringSession } from '../../shared/localApps';
import { isDesktopAuthoringRef } from '../../shared/localApps';
import type { SessionRef } from '../../shared/settings';
import type { SessionOpenSelection } from '../../shared/sessionWorkspace';

export interface AppDevelopmentChat extends SessionRef { title: string; selection?: SessionOpenSelection }
export interface AppWorkspaceGroup {
  key: string;
  name: string;
  projectPath?: string;
  app?: DesktopLocalApp;
  chats: AppDevelopmentChat[];
}

/** Only native authoring records establish App ownership; a folder name never does. */
export function localAppWorkspaceGroups(apps: readonly DesktopLocalApp[], sources: readonly LocalAppAuthoringSession[],
  catalogs: Record<string, { sessions: readonly { uuid: string; title: string; path: string; mode?: string }[] }> = {},
  drafts: readonly SessionRef[] = [], owners: readonly SessionRef[] = []): AppWorkspaceGroup[] {
  const groups = new Map<string, AppWorkspaceGroup>();
  for (const app of apps) {
    const path = app.authoring?.sourceDirectory;
    const key = app.id;
    const group: AppWorkspaceGroup = { key, name: app.name, projectPath: path, app, chats: [] };
    groups.set(key, group);
    if (isDesktopAuthoringRef(app.authoring)) group.chats.push({ projectPath: app.authoring.projectRef.path,
      sessionId: app.authoring.authoringSessionId, mode: 'code', title: app.name });
  }
  for (const source of [...sources].reverse()) {
    const candidates = [...groups.values()].filter(group => group.projectPath === source.projectPath);
    let group = (source.appId ? groups.get(source.appId) : undefined)
      ?? candidates.find(group => group.chats.some(chat => chat.sessionId === source.sessionId))
      ?? (candidates.length === 1 ? candidates[0] : groups.get(source.projectPath));
    if (!group) { group = { key: source.projectPath, name: source.name.trim() || source.projectPath.split(/[\\/]+/).at(-1) || 'App', projectPath: source.projectPath, chats: [] }; groups.set(group.key, group); }
    if (!group.projectPath) group.projectPath = source.projectPath;
    if (!group.chats.some(chat => chat.sessionId === source.sessionId)) group.chats.push({ projectPath: source.projectPath,
      sessionId: source.sessionId, mode: 'code', title: source.name, selection: source.source });
    else {
      const chat = group.chats.find(chat => chat.sessionId === source.sessionId)!;
      chat.selection = source.source;
    }
  }
  for (const group of groups.values()) {
    if (!group.projectPath) continue;
    const shared = [...groups.values()].filter(other => other.projectPath === group.projectPath).length > 1;
    const rows = (catalogs[group.projectPath]?.sessions ?? []).filter(row => {
      const owner = owners.find(owner => owner.sessionId === row.uuid);
      return row.mode === 'code' && (!owner || owner.projectId === group.projectPath)
        && (!shared || group.chats.some(chat => chat.sessionId === row.uuid));
    });
    for (const row of rows) {
      const existing = group.chats.find(chat => chat.sessionId === row.uuid);
      if (existing) { existing.title = row.title || existing.title; continue; }
      group.chats.push({ projectPath: group.projectPath, sessionId: row.uuid, mode: 'code', title: row.title,
        selection: { scope: 'current', transcriptPath: row.path } });
    }
    for (const draft of drafts) {
      if (!shared && draft.mode === 'code' && draft.projectId !== null && draft.projectPath === group.projectPath && !group.chats.some(chat => chat.sessionId === draft.sessionId)) {
        group.chats.push({ ...draft, title: '' });
      }
    }
  }
  return [...groups.values()];
}
