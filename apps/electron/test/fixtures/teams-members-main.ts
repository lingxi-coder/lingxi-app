import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { TeamsService } from '../../src/main/teams/service';
import { installTeamsIpc } from '../../src/main/teams/ipc';

async function main() {
  const input = JSON.parse(process.env.TEAMS_MEMBERS_FIXTURE!);
  app.setPath('userData', input.userData);
  await app.whenReady();
  const account = { snapshot: () => ({ user: { uid: input.uid } }), getIdToken: async () => input.idToken, onChanged: () => () => {} };
  const service = new TeamsService(account, input.config, input.base, input.emulator);
  const window = new BrowserWindow({ show: false, webPreferences: { preload: input.preload, contextIsolation: true, sandbox: true, nodeIntegration: false } });
  const close = installTeamsIpc(service, () => [window]);
  try {
    await window.loadURL('data:text/html,<title>Teams membership IPC probe</title>');
    const call = (method: string, ...args: unknown[]) => window.webContents.executeJavaScript(`window.lingxi.teams[${JSON.stringify(method)}](...${JSON.stringify(args)})`);
    const teamId = 'electron-members';
    await call('execute', input.uid, { action: 'create', teamId, name: 'Electron membership probe' });
    const list = await call('list', input.uid);
    assert.ok(list.teams.some((team: { id: string }) => team.id === teamId));
    const detail = await call('detail', input.uid, teamId);
    assert.equal(detail.members[0].uid, input.uid);
    const invite = await call('execute', input.uid, { action: 'invite', teamId, requestId: 'ipc-invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 });
    assert.match(invite.code, /^[A-Za-z0-9_-]{32}$/u);
    await assert.rejects(call('execute', 'wrong-account', { action: 'join', code: invite.code, requestId: 'ipc-join01' }));
    await call('execute', input.uid, { action: 'delete', teamId });
    console.log('TEAMS_MEMBERS_ELECTRON_PASS: real preload IPC, custom-token session, Firestore listeners, create/invite/delete, account binding');
  } finally { close(); window.destroy(); }
}
void main().then(() => app.exit(0), error => { console.error(error); app.exit(1); });
