import { TeamMessaging } from '../../../../packages/teams/messages';
import { indexedDbMessageStorage } from '../../../../packages/teams/message-storage';
import type { TeamsApi } from '../../../../packages/teams';

declare global { interface Window { runTeamMessageProbe(uid: string): Promise<unknown> } }
window.runTeamMessageProbe = async uid => {
  const desktop = window.lingxi!.teams;
  const api = { messages: command => desktop.messages(uid, command), onMessagesChanged: callback => desktop.onMessagesChanged(callback) } as TeamsApi;
  const storage = indexedDbMessageStorage(), scope = { teamId: 'electron-messages', generation: 1 };
  const until = async (condition: () => boolean) => { const deadline = Date.now() + 15000; while (!condition()) { if (Date.now() > deadline) throw new Error('Projection timed out'); await new Promise(resolve => setTimeout(resolve, 20)); } };
  const first = new TeamMessaging(api, storage, uid, scope);
  try {
    await first.start(); await until(() => !first.offline); first.select('electron-chat');
    await first.queue('create', 'Synthetic Electron message');
    await until(() => first.state.outbox.length === 0 && first.state.threads['electron-chat']?.messages.length === 1);
  } finally { first.close(); }
  const restored = new TeamMessaging(api, storage, uid, scope);
  try {
    await restored.start(); restored.select('electron-chat');
    await until(() => restored.state.threads['electron-chat']?.cursor === 1);
    const message = restored.state.threads['electron-chat']!.messages[0]!;
    if (message.text !== 'Synthetic Electron message') throw new Error('Durable body missing');
    await restored.queue('edit', 'Edited Electron message', message.id); await until(() => restored.state.threads['electron-chat']?.cursor === 2);
    await restored.queue('recall', '', message.id); await until(() => restored.state.threads['electron-chat']?.cursor === 3);
    await restored.markRead();
    if (restored.state.threads['electron-chat']!.messages[0]!.text !== '' || restored.state.conversations[0]!.lastReadSeq !== 1) throw new Error('Recall/read failed');
    return { cursor: 3, messageCount: 1, recalled: true, unread: 0, durableRestart: true };
  } finally { restored.close(); }
};
