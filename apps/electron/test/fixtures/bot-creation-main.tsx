// Extend the whole-window Bots host with a controllable setup chat and Bot creation.
import './bots-sidebar-main';
import { DESKTOP_BRIDGE_GLOBAL } from '../../src/shared/desktopBridge';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import { fixtureConversationSpaces } from './conversation-spaces-mock';

setLanguagePreference('en');
const host = (window as any)[DESKTOP_BRIDGE_GLOBAL];
const originalBootstrap = host.bootstrap;
const originalGet = host.bots.get;
const calls = (window as any).fixtureCalls as unknown[][];
const target = { projectPath: '/fixture/global-code', sessionId: 'bot-setup-chat', projectId: null, mode: 'code' as const };
let opened = false;
const listeners = new Set<(envelope: unknown) => void>();
let created: any;
let sequence = 0;
const fixture = {
  failOpen: true, failCreate: true,
  emit: (event: unknown) => {
    const envelope = { sessionId: target.sessionId, sequence: ++sequence, event };
    for (const listener of listeners) listener(envelope);
  },
};
(window as any).botCreationFixture = fixture;

host.onEvent = (callback: (envelope: unknown) => void) => { listeners.add(callback); return () => listeners.delete(callback); };
host.bootstrap = async () => {
  const snapshot = await originalBootstrap();
  if (!opened) return snapshot;
  return {
    ...snapshot, activeSession: target, settings: { ...snapshot.settings, activeSession: target },
    workspace: { path: target.projectPath, trusted: true },
    conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: snapshot.settings.projects, activeSession: target }),
    runtimes: [...snapshot.runtimes, { ...target, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  };
};
host.newSession = async (projectId: string | null) => {
  calls.push(['newSession', projectId]);
  await new Promise(resolve => setTimeout(resolve, 80));
  if (fixture.failOpen) { fixture.failOpen = false; throw new Error('Fixture engine unavailable'); }
  opened = true;
  return host.bootstrap();
};
host.command = async (sessionId: string, command: unknown) => { calls.push(['command', sessionId, command]); };
host.bots.create = async (request: any) => {
  calls.push(['create', request]);
  await new Promise(resolve => setTimeout(resolve, 80));
  if (fixture.failCreate) { fixture.failCreate = false; throw new Error('Fixture could not save Bot'); }
  created = { ...request, botId: 'created-bot', createdAt: Date.now(), updatedAt: Date.now(), revision: 1, workspaces: request.workspaces ?? [] };
  return created;
};
host.bots.get = async (botId: string) => botId === created?.botId ? created : originalGet(botId);
