import { initializeApp, deleteApp } from 'firebase/app';
import { initializeAuth, browserLocalPersistence, connectAuthEmulator, signInWithCustomToken } from 'firebase/auth';
import { disableNetwork, enableNetwork, terminate } from 'firebase/firestore';
import { teamsDatabase } from '../../src/teams/firestore.js';
import { webTeamsApi } from '../../src/teams/api.js';
import { TeamMessaging } from '../../../../packages/teams/messages.js';
import { indexedDbCommandStorage } from '../../../../packages/teams/command-storage.js';

async function start(): Promise<void> {
  const fixture = await fetch('/fixture?client=web').then(response => response.json());
  if (fixture.config.projectId !== 'demo-lingxi-teams-t0') throw new Error('Demo fixture required');
  const app = initializeApp(fixture.config, 'teams-cache-web');
  const auth = initializeAuth(app, { persistence: browserLocalPersistence });
  connectAuthEmulator(auth, 'http://127.0.0.1:9098', { disableWarnings: true });
  await auth.authStateReady();
  if (auth.currentUser?.uid !== fixture.uid) await signInWithCustomToken(auth, fixture.customToken);
  const user = auth.currentUser;
  if (!user || user.uid !== fixture.uid) throw new Error('Wrong fixture account');
  const db = teamsDatabase(app);
  if (new URL(location.href).searchParams.has('offline')) await disableNetwork(db);
  const api = webTeamsApi(user, db), model = new TeamMessaging(api, indexedDbCommandStorage(), fixture.uid, { teamId: fixture.teamId, generation: fixture.generation });
  function render(): void {
    const messages = model.state.threads[fixture.conversationId]?.messages ?? [];
    document.querySelector('#state')!.textContent = JSON.stringify({ count: messages.length, offline: model.offline, error: model.error, first: messages[0]?.text, second: messages[1]?.text, recalled: messages[1]?.recalled, read: model.state.conversations[0]?.lastReadSeq });
  }
  model.onChanged(render); await model.start(); model.select(fixture.conversationId);
  document.querySelector('#pause')!.addEventListener('click', () => void disableNetwork(db));
  document.querySelector('#resume')!.addEventListener('click', () => void enableNetwork(db));
  document.querySelector('#close')!.addEventListener('click', () => { void (async () => { model.close(); api.close(); await terminate(db); await deleteApp(app); document.querySelector('#state')!.textContent = 'Closed'; })(); });
}
void start().catch(error => { document.querySelector('#state')!.textContent = error instanceof Error ? error.message : 'FAILED'; });
