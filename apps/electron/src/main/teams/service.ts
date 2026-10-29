import { EventEmitter } from 'node:events';
import { collection, onSnapshot, query, where, doc } from 'firebase/firestore';
import type { FirebaseOptions } from 'firebase/app';
import { teamsTransport, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../../packages/teams';
import type { MessageCommand } from '../../../../../packages/teams/messages';
import { openTeamsFirestoreSession, type TeamsFirestoreSession, type TeamsFirestoreOptions } from './firestore';

interface Account {
  snapshot(): { user: { uid: string } | null };
  getIdToken(): Promise<string>;
  onChanged(callback: () => void): () => void;
}
/** Main owns the credential and SDK; IPC carries only typed membership data. */
export class TeamsService {
  private readonly events = new EventEmitter();
  private uid?: string;
  private epoch = 0;
  private session?: TeamsFirestoreSession;
  private opening?: Promise<TeamsFirestoreSession>;
  private stops = new Map<string, () => void>();
  private failed = new Map<string, number>();
  private readonly stopAccount: () => void;
  constructor(private readonly account: Account, private readonly config: FirebaseOptions, private readonly base = 'https://lingxios.net',
    private readonly emulator?: TeamsFirestoreOptions['emulator']) {
    this.uid = account.snapshot().user?.uid;
    this.stopAccount = account.onChanged(() => {
      const uid = account.snapshot().user?.uid;
      if (uid === this.uid) return;
      this.uid = uid; this.reset(); this.events.emit('change', { invalidated: true });
    });
  }
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void {
    this.events.on('change', callback); return () => { this.events.off('change', callback); };
  }
  onMessagesChanged(callback: () => void): () => void { this.events.on('messages', callback); return () => { this.events.off('messages', callback); }; }
  private reset(): void {
    this.epoch++;
    for (const stop of this.stops.values()) stop();
    this.stops.clear();
    this.failed.clear();
    const previous = this.session; this.session = undefined; this.opening = undefined;
    void previous?.close().catch(() => undefined);
  }
  private async connect(expectedUid: string): Promise<TeamsFirestoreSession> {
    await this.account.getIdToken(); // Also waits for the saved account to restore.
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
    if (currentUid !== this.uid) { this.uid = currentUid; this.reset(); }
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    if (this.session) return this.session;
    if (this.opening) return this.opening;
    const uid = this.uid, epoch = this.epoch;
    const opening = (async () => {
      const credential = await this.request<{ uid: string; customToken: string }>('/session', {});
      if (epoch !== this.epoch || credential.uid !== uid) throw new Error('INVALID_SESSION');
      const session = await openTeamsFirestoreSession({ config: this.config, uid, customToken: credential.customToken, emulator: this.emulator });
      if (epoch !== this.epoch) { await session.close(); throw new Error('INVALID_SESSION'); }
      this.session = session;
      this.stops.set('index', onSnapshot(collection(session.db, 'users', uid, 'teams'), { includeMetadataChanges: true }, snapshot => {
        if (epoch === this.epoch) this.events.emit('change', { invalidated: snapshot.metadata.fromCache });
      }, () => {
        if (epoch !== this.epoch) return;
        this.stops.get('index')?.(); this.stops.delete('index'); this.failed.set('index', 0);
        this.events.emit('change', { invalidated: true });
      }));
      return session;
    })();
    this.opening = opening;
    try { return await opening; } finally { if (this.opening === opening) this.opening = undefined; }
  }
  private async request<T>(path = '', body?: unknown): Promise<T> {
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const uid = this.uid, epoch = this.epoch;
    const transport = teamsTransport(this.base, async () => {
      const token = await this.account.getIdToken();
      if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
      return token;
    });
    const result = await transport<T>(path, body);
    if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
    return result;
  }
  private async checked<T>(expectedUid: string, path = '', body?: unknown): Promise<T> {
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
    if (currentUid !== this.uid) { this.uid = currentUid; this.reset(); }
    const epoch = this.epoch;
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const result = await this.request<T>(path, body);
    if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
    return result;
  }
  async list(expectedUid: string): Promise<{ teams: TeamSummary[] }> { await this.connect(expectedUid); return this.checked(expectedUid); }
  async detail(teamId: string, expectedUid: string): Promise<TeamDetail> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(teamId)) throw new Error('INVALID_INPUT');
    const session = await this.connect(expectedUid), epoch = this.epoch;
    const current = await this.checked<TeamDetail>(expectedUid, `/${teamId}`);
    if (!this.stops.has(teamId) && this.failed.get(teamId) !== current.team.generation) {
      this.stops.set(teamId, onSnapshot(collection(session.db, 'teams', teamId, 'members'), { includeMetadataChanges: true }, snapshot => {
        if (epoch === this.epoch) this.events.emit('change', { invalidated: snapshot.metadata.fromCache });
      }, () => {
        if (epoch !== this.epoch) return;
        this.stops.get(teamId)?.(); this.stops.delete(teamId);
        this.failed.set(teamId, current.team.generation);
        this.events.emit('change', { invalidated: true });
      }));
    }
    return current;
  }
  async messages<T>(command: MessageCommand, expectedUid: string): Promise<T> {
    if (!command || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.teamId) || !Number.isSafeInteger(command.generation) || command.generation < 1) throw new Error('INVALID_INPUT');
    const session = await this.connect(expectedUid), epoch = this.epoch;
    const key = `messages:${command.teamId}:${command.generation}`;
    const emit = () => { if (epoch === this.epoch) this.events.emit('messages'); };
    if (!this.stops.has(key)) this.stops.set(key, onSnapshot(query(collection(session.db, 'teams', command.teamId, 'conversations'), where('memberUids', 'array-contains', expectedUid)),
      { includeMetadataChanges: true }, snapshot => { if (!snapshot.metadata.fromCache) emit(); }, emit));
    if (command.action === 'sync') {
      if (!/^[A-Za-z0-9_-]{8,64}$/u.test(command.conversationId)) throw new Error('INVALID_INPUT');
      const readKey = `${key}:${command.conversationId}`;
      if (!this.stops.has(readKey)) this.stops.set(readKey, onSnapshot(doc(session.db, 'teams', command.teamId, 'conversations', command.conversationId, 'reads', expectedUid), snapshot => { if (!snapshot.metadata.fromCache) emit(); }, emit));
    }
    return this.checked<T>(expectedUid, '/messages', command);
  }
  execute(command: TeamCommand, expectedUid: string): Promise<TeamResult> { return this.checked(expectedUid, '', command); }
  close(): void { this.stopAccount(); this.reset(); this.events.removeAllListeners(); }
}
