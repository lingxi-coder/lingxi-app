import assert from 'node:assert/strict';
import { app } from 'electron';
import { openTeamsFirestoreSession, type TeamsFirestoreOptions } from '../../src/main/teams/firestore.js';

app.setPath('userData', process.env.TEAM_SPIKE_USER_DATA!);
let session: Awaited<ReturnType<typeof openTeamsFirestoreSession>> | undefined;
let phase = 'app-ready';
const timeout = setTimeout(() => { console.error(`Electron Team spike timed out during ${phase}`); app.exit(1); }, 30_000);
// Let Electron finish loading the ESM entry before awaiting app readiness.
async function main(): Promise<void> {
try {
  await app.whenReady();
  assert.ok(process.versions.electron && process.type === 'browser', 'this probe must run in real Electron main');
  phase = 'custom-token-sign-in';
  console.log('TEAMS_ELECTRON_PHASE', phase);
  session = await openTeamsFirestoreSession(JSON.parse(process.env.TEAM_SPIKE_SESSION!) as TeamsFirestoreOptions);
  phase = 'snapshot-listener';
  console.log('TEAMS_ELECTRON_PHASE', phase);
  const received = new Promise<void>((resolve, reject) => {
    session!.watchChanges('electron-spike', 'sdk-chat', 0, snapshot => {
      if (snapshot.docs.some(change => change.id === 'runtime01' && change.data().seq === 1)) resolve();
    }, reject);
  });
  console.log('TEAMS_ELECTRON_LISTENER_READY');
  await received;
  console.log('TEAMS_ELECTRON_SPIKE_PASS', JSON.stringify({ electron: process.versions.electron, node: process.versions.node, customToken: true, listener: true }));
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  clearTimeout(timeout);
  await session?.close();
  app.exit(Number(process.exitCode ?? 0));
}
}
void main();
