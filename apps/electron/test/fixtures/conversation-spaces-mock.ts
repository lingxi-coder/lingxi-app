import type { ConversationSpace, ConversationSpacesSnapshot, OwnedConversation } from '../../src/shared/conversationSpaces';

/** Build the explicit owner and navigation data returned by the Desktop host. */
export function fixtureConversationSpaces(options: {
  mode: ConversationSpace;
  projects: readonly string[];
  selectedProject?: string | null;
  activeSession?: { projectPath: string; sessionId: string; projectId?: string | null };
  sessions?: readonly { projectPath: string; sessionId: string; projectId?: string | null }[];
}): ConversationSpacesSnapshot {
  const owner = (session: { projectPath: string; sessionId: string; projectId?: string | null }): OwnedConversation => ({
    ...session, mode: options.mode, projectId: session.projectId === undefined ? session.projectPath : session.projectId,
  });
  const sessions = (options.sessions ?? []).map(owner);
  const active = options.activeSession && owner(options.activeSession);
  if (active && !sessions.some((session) => session.sessionId === active.sessionId)) sessions.push(active);
  const navigation: ConversationSpacesSnapshot['navigation'] = { chat: { projectId: null }, code: { projectId: null } };
  navigation[options.mode] = { projectId: options.selectedProject ?? null, ...(active ? { session: active } : {}) };
  return {
    activeMode: options.mode,
    projects: Object.fromEntries(options.projects.map((project) => [project, [options.mode]])),
    navigation, sessions, globalWorkspace: '/fixture/global-space',
  };
}
