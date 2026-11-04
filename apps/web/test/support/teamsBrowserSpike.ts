import { deleteApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, inMemoryPersistence, signInWithCustomToken, signOut } from 'firebase/auth';
import { connectFirestoreEmulator, getFirestore, terminate } from 'firebase/firestore';
import { watchTeamChanges } from '../../src/teams/firestore.js';

declare global {
  interface Window { teamSpike: { projectId: string; customToken: string; authHost: string; firestoreHost: string; firestorePort: number } }
}
const settings = window.teamSpike;
const app = initializeApp({ projectId: settings.projectId, apiKey: 'emulator-only', appId: 'teams-real-browser' }, 'teams-real-browser');
const auth = initializeAuth(app, { persistence: inMemoryPersistence });
const db = getFirestore(app);
let stop: (() => void) | undefined;
let result: { passed: boolean; error?: string; changes?: string[] };
try {
  connectAuthEmulator(auth, `http://${settings.authHost}`, { disableWarnings: true });
  connectFirestoreEmulator(db, settings.firestoreHost, settings.firestorePort);
  await signInWithCustomToken(auth, settings.customToken);
  const changes = await new Promise<string[]>((resolve, reject) => {
    stop = watchTeamChanges(db, 'browser-spike', 'sdk-chat', 1, snapshot => {
      if (snapshot.docs.some(change => change.id === 'change002' && change.data().operation === 'recall')) resolve(snapshot.docs.map(change => change.id));
    }, reject);
  });
  result = { passed: true, changes };
} catch (error) {
  result = { passed: false, error: error instanceof Error ? error.message : String(error) };
} finally {
  stop?.(); await terminate(db); await signOut(auth); await deleteApp(app);
}
document.getElementById('result')!.textContent = JSON.stringify(result);
await fetch('/result', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(result) });
