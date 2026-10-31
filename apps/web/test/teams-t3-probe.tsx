import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, inMemoryPersistence, signInWithCustomToken } from 'firebase/auth';
import { connectFirestoreEmulator, getFirestore } from 'firebase/firestore';
import { TeamsPanel } from '../../../packages/teams/TeamsPanel';
import type { TeamDetail } from '../../../packages/teams';
import { webTeamsApi } from '../src/teams/api';
import '../../../acceptance/teams-t3-complete-20261029/probes/web/probe.css';

const projectId = 'demo-lingxi-teams-t0';
const requestedUid = new URL(location.href).searchParams.get('uid') ?? 'web-human';
if (!['127.0.0.1', 'localhost'].includes(location.hostname) || !/^web-[a-z0-9-]{1,60}$/u.test(requestedUid)) {
  throw new Error('Acceptance harness only supports local synthetic Web accounts.');
}

interface Session {
  api: ReturnType<typeof webTeamsApi>; uid: string; detail: TeamDetail; conversationId: string;
}
async function connect(): Promise<Session> {
  const response = await fetch(`/_acceptance/bootstrap?uid=${encodeURIComponent(requestedUid)}`, { cache: 'no-store', redirect: 'error' });
  if (!response.ok) throw new Error(`Bootstrap failed (${response.status}). Start the synthetic emulator fixture first.`);
  const bootstrap = await response.json() as { customToken: string; detail: TeamDetail; conversationId: string };
  if (!bootstrap.customToken || !bootstrap.detail?.team?.id || !Number.isSafeInteger(bootstrap.detail.team.generation)) throw new Error('Invalid fixture bootstrap.');
  const app = initializeApp({ projectId, apiKey: 'emulator-only', appId: 'teams-t3-browser-acceptance' }, `teams-t3-probe-${requestedUid}`);
  const auth = initializeAuth(app, { persistence: inMemoryPersistence });
  connectAuthEmulator(auth, 'http://127.0.0.1:9098', { disableWarnings: true });
  const db = getFirestore(app);
  connectFirestoreEmulator(db, '127.0.0.1', 8085);
  const credential = await signInWithCustomToken(auth, bootstrap.customToken);
  if (credential.user.uid !== requestedUid) throw new Error('Fixture account mismatch.');
  const api = webTeamsApi(credential.user, db);
  return { api, uid: credential.user.uid, detail: bootstrap.detail, conversationId: bootstrap.conversationId };
}

// One real authenticated adapter per page. Product components own all message
// controllers, IndexedDB, state, permissions and command transport below.
const session = connect();
function Probe() {
  const [current, setCurrent] = useState<Session>(), [error, setError] = useState('');
  useEffect(() => {
    let closed = false, active: Session | undefined;
    void session.then(value => { active = value; if (closed) value.api.close(); else setCurrent(value); }, failure => { if (!closed) setError(failure instanceof Error ? failure.message : 'Harness startup failed'); });
    return () => { closed = true; active?.api.close(); };
  }, []);
  return <><aside className="probe-banner" aria-label="Acceptance environment"><b>Local Teams T3 acceptance</b><span>{projectId} · Auth 9098 · Firestore 8085 · API 18783</span>{current ? <output data-testid="probe-ready" data-uid={current.uid} data-team-id={current.detail.team.id} data-conversation-id={current.conversationId}>Synthetic account: {current.uid}</output> : <span role="status">Connecting to emulators…</span>}</aside>{error ? <p role="alert">{error}</p> : null}{current ? <TeamsPanel key={`${current.uid}:${current.detail.team.generation}`} api={current.api} uid={current.uid} zh={false} /> : null}</>;
}
createRoot(document.getElementById('root')!).render(<Probe />);
