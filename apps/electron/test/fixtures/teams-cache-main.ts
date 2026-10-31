import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { app, BrowserWindow } from 'electron';
import { openTeamsMessageSession, type TeamsMessageSession } from '../../src/main/teams/messageWindow.js';
import { emptyProjection, applyMessageSnapshot, type MessageSnapshot } from '../../../../packages/teams/messages.js';

const profile = process.env.TEAM_CACHE_DESKTOP_PROFILE, resultPath = process.env.TEAM_CACHE_DESKTOP_RESULT;
if (!profile || !resultPath) throw new Error('Isolated cache acceptance paths required');
app.setPath('userData', profile);
app.on('window-all-closed', () => {});
async function main(resultPath: string): Promise<void> {
await app.whenReady();
const base = 'http://127.0.0.1:41791';
let session: TeamsMessageSession | undefined;
const checks: string[] = [];
try {
  const fixture = await fetch(`${base}/fixture?client=desktop`).then(response => response.json());
  assert.equal(fixture.config.projectId, 'demo-lingxi-teams-t0');
  const scope = { teamId: fixture.teamId, generation: fixture.generation };
  const options = { config: fixture.config, uid: fixture.uid, customToken: fixture.customToken, emulator: { host: '127.0.0.1' as const, authPort: 9098, firestorePort: 8085 } };
  const snapshots: MessageSnapshot[] = [];
  let view = emptyProjection(fixture.uid, scope);
  const receive = (value: MessageSnapshot) => { if (value.kind === 'messages') view = applyMessageSnapshot(view, value); snapshots.push(value); };
  function latest(): Extract<MessageSnapshot, { kind: 'messages' }> | undefined { const last = snapshots.filter((value): value is Extract<MessageSnapshot, { kind: 'messages' }> => value.kind === 'messages').at(-1); return last ? { ...last, messages: view.threads[fixture.conversationId]?.messages ?? [] } : undefined; }
  async function until(predicate: () => boolean): Promise<void> { const end = Date.now() + 30000; while (!predicate()) { if (Date.now() >= end) throw new Error('Desktop SDK cache timed out'); await new Promise(resolve => setTimeout(resolve, 20)); } }
  session = await openTeamsMessageSession(options);
  session.watch('warm', scope, fixture.conversationId, receive);
  await until(() => latest()?.messages.length === 125 && latest()?.fromCache === false); checks.push('warm-server-snapshot');
  let window = BrowserWindow.getAllWindows()[0]!;
  const partition = window.webContents.session;
  assert.equal(partition.listenerCount('will-download'), 1);
  partition.webRequest.onBeforeRequest({ urls: ['http://127.0.0.1:8085/*'] }, (_details, callback) => callback({ cancel: true }));
  await session.close(); snapshots.length = 0;
  assert.equal(partition.listenerCount('will-download'), 0);
  await partition.closeAllConnections();
  session = await openTeamsMessageSession({ ...options, customToken: undefined });
  assert.equal(partition.listenerCount('will-download'), 1);
  session.watch('cold', scope, fixture.conversationId, receive);
  await until(() => latest()?.messages.length === 125 && latest()?.fromCache === true); checks.push('disk-cache-offline-reopen');
  assert.equal(latest()!.messages[0]!.text, 'Body 1');
  await fetch(`${base}/edit?client=desktop`);
  partition.webRequest.onBeforeRequest(null);
  await until(() => latest()?.messages[0]?.text === 'Edited oldest' && latest()?.fromCache === false);
  assert.equal(latest()!.messages[1]!.text, ''); assert.equal(latest()!.messages[1]!.recalled, true); checks.push('old-edit-recall-reconnect');
  await fetch(`${base}/read?client=desktop`);
  await until(() => snapshots.some(value => value.kind === 'read' && value.lastReadSeq === 120)); checks.push('cross-device-read');
  await fetch(`${base}/revoke?client=desktop`);
  await until(() => snapshots.some(value => value.kind === 'error' && value.error === 'MEMBERSHIP_REQUIRED')); checks.push('membership-revocation');
  await session.close(true); session = undefined;
  await assert.rejects(openTeamsMessageSession({ ...options, customToken: undefined })); checks.push('logout-removes-sdk-authentication');
  assert.equal(partition.listenerCount('will-download'), 0); checks.push('session-handlers-released');
  await writeFile(resultPath, JSON.stringify({ pass: true, checks }, null, 2));
} catch (error) {
  await writeFile(resultPath, JSON.stringify({ pass: false, checks, error: error instanceof Error ? error.message : 'FAILED' }, null, 2)); process.exitCode = 1;
} finally { await session?.close(true); app.exit(process.exitCode ? 1 : 0); }

}
void main(resultPath);
