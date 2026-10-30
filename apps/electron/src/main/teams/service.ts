import { EventEmitter } from 'node:events';
import { collection, onSnapshot, query, where, doc } from 'firebase/firestore';
import type { FirebaseOptions } from 'firebase/app';
import { teamsTransport, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../../packages/teams';
import type { MessageCommand } from '../../../../../packages/teams/messages';
import type { BindLocalTeamBotInput, LocalTeamBot } from '../../../../../packages/teams/bots';
import { openTeamsFirestoreSession, type TeamsFirestoreSession, type TeamsFirestoreOptions } from './firestore';

interface Account {
  snapshot(): { user: { uid: string } | null };
  sessionEpoch?(): number;
  getIdToken(): Promise<string>;
  onChanged(callback: () => void): () => void;
}
export interface TeamAccountContext { uid: string; epoch: number }
export interface LocalTeamHost {
  localBots(uid: string): Promise<LocalTeamBot[]>;
  bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }>;
  invalidate(): void;
  stopBinding(teamId: string, botKey: string): void;
  runStatus?(uid: string, teamId: string, conversationId: string, triggerId: string): string | undefined;
  close(): void;
}
/** Main owns the credential and SDK; IPC carries only typed membership data. */
export class TeamsService {
  private readonly events = new EventEmitter();
  private uid?: string;
  private epoch = 0;
  private authenticationEpoch: number;
  private session?: TeamsFirestoreSession;
  private opening?: Promise<TeamsFirestoreSession>;
  private stops = new Map<string, () => void>();
  private failed = new Map<string, number>();
  private readonly stopAccount: () => void;
  private host?: LocalTeamHost;
  constructor(private readonly account: Account, private readonly config: FirebaseOptions, private readonly base = 'https://lingxios.net',
    private readonly emulator?: TeamsFirestoreOptions['emulator'], private readonly watchSnapshot: typeof onSnapshot = onSnapshot) {
    this.uid = account.snapshot().user?.uid;
    this.authenticationEpoch = account.sessionEpoch?.() ?? 0;
    this.stopAccount = account.onChanged(() => {
      this.syncAccount();
    });
  }
  private syncAccount(): void {
    const uid = this.account.snapshot().user?.uid, authenticationEpoch = this.account.sessionEpoch?.() ?? 0;
    if (uid === this.uid && authenticationEpoch === this.authenticationEpoch) return;
    this.uid = uid; this.authenticationEpoch = authenticationEpoch; this.reset(); this.events.emit('change', { invalidated: true });
  }
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void {
    this.events.on('change', callback); return () => { this.events.off('change', callback); };
  }
  onMessagesChanged(callback: () => void): () => void { this.events.on('messages', callback); return () => { this.events.off('messages', callback); }; }
  private reset(): void {
    this.epoch++;
    this.host?.invalidate();
    for (const stop of this.stops.values()) stop();
    this.stops.clear();
    this.failed.clear();
    const previous = this.session; this.session = undefined; this.opening = undefined;
    void previous?.close().catch(() => undefined);
  }
  private async connect(expectedUid: string): Promise<TeamsFirestoreSession> {
    this.syncAccount();
    const startedEpoch = this.epoch, restoring = !this.uid;
    await this.account.getIdToken(); // Also waits for the saved account to restore.
    this.syncAccount();
    if (!restoring && startedEpoch !== this.epoch) throw new Error('INVALID_SESSION');
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
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
    this.syncAccount();
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const uid = this.uid, epoch = this.epoch;
    const transport = teamsTransport(this.base, async () => {
      this.syncAccount();
      if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
      const token = await this.account.getIdToken();
      this.syncAccount();
      if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
      return token;
    });
    const result = await transport<T>(path, body);
    this.syncAccount();
    if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
    return result;
  }
  private async checked<T>(expectedUid: string, path = '', body?: unknown): Promise<T> {
    this.syncAccount();
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
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
    if (this.account.snapshot().user?.uid !== expectedUid) throw new Error('INVALID_SESSION');
    if (!command || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.teamId) || !Number.isSafeInteger(command.generation) || command.generation < 1) throw new Error('INVALID_INPUT');
    // Runtime checks are required: IPC values do not acquire TypeScript's union.
    if (!['list', 'create', 'sync', 'read', 'mutate', 'bots', 'unbindBot', 'botState', 'botAnswer', 'recoverBot'].includes(command.action)) throw new Error('INVALID_INPUT');
    if (command.action === 'unbindBot') this.host?.stopBinding(command.teamId, command.botKey);
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
    const result = await this.checked<T>(expectedUid, '/messages', command);
    if (command.action === 'botState' && result && typeof result === 'object'
      && (result as Record<string, unknown>).ownerUid === expectedUid) {
      const hostStatus = this.host?.runStatus?.(expectedUid, command.teamId, command.conversationId, command.triggerId);
      if (hostStatus) return { ...result, hostStatus };
    }
    if (command.action === 'unbindBot' || command.action === 'recoverBot') this.host?.invalidate();
    return result;
  }
  attachHost(host: LocalTeamHost): void { if (this.host) throw new Error('TEAM_HOST_ALREADY_ATTACHED'); this.host = host; }
  notifyHostChanged(): void { this.events.emit('messages'); }
  localBots(uid: string): Promise<LocalTeamBot[]> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.localBots(uid); }
  bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }> {
    this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.bindLocalBot(uid, input);
  }
  /** Main-only capabilities; no IPC branch exposes these methods or commands. */
  hostContext(): TeamAccountContext | undefined { this.syncAccount(); return this.uid && this.account.snapshot().user?.uid === this.uid ? { uid: this.uid, epoch: this.epoch } : undefined; }
  assertHostContext(context: TeamAccountContext): void {
    this.syncAccount();
    if (!this.uid || context.uid !== this.uid || context.epoch !== this.epoch || this.account.snapshot().user?.uid !== context.uid) throw new Error('INVALID_SESSION');
  }
  async hostToken(context: TeamAccountContext): Promise<string> {
    this.assertHostContext(context);
    const token = await this.account.getIdToken();
    this.assertHostContext(context);
    if (!token || token.length > 32 * 1024) throw new Error('INVALID_SESSION');
    return token;
  }
  async hostMessages<T>(context: TeamAccountContext, command: Record<string, unknown>): Promise<T> {
    this.assertHostContext(context);
    if (!['bots', 'bindBot', 'unbindBot', 'heartbeatBot', 'botQueue', 'list'].includes(command.action as string)) throw new Error('INVALID_INPUT');
    const session = await this.connect(context.uid);
    this.assertHostContext(context);
    // Listeners wake discovery; fallback polling and the HTTP queue remain authoritative.
    // Bot cards include owner-only disabled revisions and are intentionally HTTP
    // only. Rules deny raw Bot SDK reads; do not attach an always-denied watch.
    if (command.action === 'list' && typeof command.teamId === 'string') {
      const key = `host:${command.action}:${command.teamId}:${command.generation}`;
      if (!this.stops.has(key) && !this.failed.has(key)) {
        const emit = () => { if (context.epoch === this.epoch) this.events.emit('messages'); };
        const fail = () => { if (context.epoch === this.epoch) {
          this.stops.get(key)?.(); this.stops.delete(key);
          this.failed.set(key, command.generation as number);
          this.events.emit('change', { invalidated: true });
        } };
        const source = query(collection(session.db, 'teams', command.teamId, 'conversations'), where('memberUids', 'array-contains', context.uid));
        this.stops.set(key, this.watchSnapshot(source, snapshot => { if (!snapshot.metadata.fromCache) emit(); }, fail));
      }
    }
    const result = await this.checked<T>(context.uid, '/messages', command);
    this.assertHostContext(context); return result;
  }
  execute(command: TeamCommand, expectedUid: string): Promise<TeamResult> { return this.checked(expectedUid, '', command); }
  close(): void { this.stopAccount(); this.host?.close(); this.host = undefined; this.reset(); this.events.removeAllListeners(); }
}
