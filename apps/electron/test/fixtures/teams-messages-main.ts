import { app, BrowserWindow } from 'electron';
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { TeamsService } from '../../src/main/teams/service';
import { installTeamsIpc } from '../../src/main/teams/ipc';

async function main() {
  const input = JSON.parse(process.env.TEAMS_MEMBERS_FIXTURE!);
  app.setPath('userData', input.userData); await app.whenReady();
  const account = { snapshot: () => ({ user: { uid: input.uid } }), getIdToken: async () => input.idToken, onChanged: () => () => {} };
  const service = new TeamsService(account, input.config, input.base, input.emulator);
  const window = new BrowserWindow({ show: false, webPreferences: { preload: input.preload, contextIsolation: true, sandbox: true, nodeIntegration: false } });
  const close = installTeamsIpc(service, () => [window]);
  const post = async (body: unknown) => { const response = await fetch(`${input.base}/v1/teams`, { method: 'POST', headers: { Authorization: `Bearer ${input.memberIdToken}` }, body: JSON.stringify(body) }); assert.equal(response.status, 200); return response.json(); };
  try {
    await window.loadURL(`${input.base}/probe`);
    const call = (method: string, ...args: unknown[]) => window.webContents.executeJavaScript(`window.lingxi.teams[${JSON.stringify(method)}](...${JSON.stringify(args)})`);
    const teamId = 'electron-messages'; await call('execute', input.uid, { action: 'create', teamId, name: 'Electron messaging probe' });
    const invite = await call('execute', input.uid, { action: 'invite', teamId, requestId: 'ipc-invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 });
    await post({ action: 'join', code: invite.code, requestId: 'ipc-member-join' });
    await call('messages', input.uid, { action: 'create', teamId, generation: 1, conversationId: 'electron-chat', kind: 'dm', title: 'IPC chat', participantUids: [input.uid, 'teams-member'] });
    await assert.rejects(call('messages', 'wrong-account', { action: 'list', teamId, generation: 1 }));
    await window.webContents.executeJavaScript(readFileSync(input.rendererProbe, 'utf8'));
    const result = await window.webContents.executeJavaScript(`window.runTeamMessageProbe(${JSON.stringify(input.uid)})`);
    assert.deepEqual(result, { cursor: 3, messageCount: 1, recalled: true, unread: 0, durableRestart: true });
    console.log('TEAMS_MESSAGES_ELECTRON_PASS: real sandboxed preload/IPC/main SDK, renderer IndexedDB restart, send/edit/recall/read and expected UID');
    await call('execute', input.uid, { action: 'delete', teamId });
  } finally { close(); window.destroy(); }
}
void main().then(() => app.exit(0), error => { console.error(error); app.exit(1); });
