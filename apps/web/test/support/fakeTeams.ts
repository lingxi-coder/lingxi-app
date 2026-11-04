import type { TeamCommand, TeamDetail, TeamMember, TeamResult, TeamSummary, TeamsApi } from '../../../../packages/teams';
import type { TeamBotCard } from '../../../../packages/teams/bots';
import { pendingCommands, type CommandStorage, type MessageCommand, type MessageScope, type MessageSignal, type MessageSnapshot, type PendingCommands, type TeamConversation, type TeamMessage } from '../../../../packages/teams/messages';

export const uid = 'alice';
export const settle = async (turns = 12) => { for (let i = 0; i < turns; i++) await new Promise(resolve => setTimeout(resolve, 0)); };
export async function until(condition: () => boolean, label = 'condition'): Promise<void> {
  const end = Date.now() + 3000;
  while (!condition()) { if (Date.now() > end) throw new Error(`Timed out waiting for ${label}`); await new Promise(resolve => setTimeout(resolve, 2)); }
}
export function memoryStorage(): CommandStorage & { records: Map<string, PendingCommands> } {
  const records = new Map<string, PendingCommands>();
  return { records, async load(key) { return structuredClone(records.get(key)); }, async update(key, reduce) { const value = pendingCommands(reduce(structuredClone(records.get(key)))); records.set(key, structuredClone(value)); return value; } };
}
export const team = (id: string, more: Partial<TeamSummary> = {}): TeamSummary => ({ id, name: `Team ${id}`, memberCount: 3, role: 'member', generation: 1, ...more });
export const members = (...names: string[]): TeamMember[] => names.map(name => ({ uid: name, displayName: name[0]!.toUpperCase() + name.slice(1), role: name === 'alice' ? 'owner' : 'member', generation: 1 }));
export const conversation = (id: string, more: Partial<TeamConversation> = {}): TeamConversation => ({
  id, kind: 'channel', title: id, participants: ['user:alice', 'user:bob'], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0, ...more,
});
export const message = (seq: number, more: Partial<TeamMessage> = {}): TeamMessage => ({
  id: `message-${seq}`, seq, author: { kind: 'user', uid: 'bob' }, kind: 'text', text: `Body ${seq}`, createdAt: '2026-11-02T10:00:00.000Z', changeSeq: seq, changeId: `change-${seq}`, ...more,
});
export const botCard = (more: Partial<TeamBotCard> = {}): TeamBotCard => ({
  key: `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`, ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', description: 'Helper',
  revision: 1, use: 'all', uids: [], permitted: true, online: true, ...more,
});

type SnapshotBody = MessageSnapshot extends infer Value ? Value extends MessageSnapshot ? Omit<Value, keyof MessageScope | 'uid' | 'fromCache'> & { fromCache?: boolean } : never : never;
export interface Watch { scope: MessageScope; conversationId: string; next(snapshot: MessageSnapshot): void; open: boolean }

/** A scriptable TeamsApi: teams, details, directories and Bots are plain data; every listener is tracked so tests can prove it was released. */
export class FakeTeams {
  /** Whose directories and threads the listeners deliver: the signed-in account. */
  constructor(readonly viewer: string = uid) {}
  teams: TeamSummary[] = [];
  details = new Map<string, TeamDetail>();
  conversations = new Map<string, TeamConversation[]>();
  threads = new Map<string, TeamMessage[]>();
  bots: TeamBotCard[] = [];
  changed = new Set<(event: { invalidated?: boolean }) => void>();
  signals = new Set<(signal?: MessageSignal) => void>();
  watches: Watch[] = [];
  commands: MessageCommand[] = [];
  teamCommands: TeamCommand[] = [];
  listCalls = 0;
  detailCalls: string[] = [];
  list: () => Promise<{ teams: TeamSummary[] }> = async () => { this.listCalls++; return { teams: structuredClone(this.teams) }; };
  detail: (teamId: string) => Promise<TeamDetail> = async teamId => {
    this.detailCalls.push(teamId);
    const value = this.details.get(teamId);
    if (value) return structuredClone(value);
    const summary = this.teams.find(item => item.id === teamId);
    if (!summary) throw new Error('MEMBERSHIP_REQUIRED');
    return { team: structuredClone(summary), members: members('alice', 'bob', 'owner'), invites: [] };
  };
  execute: (command: TeamCommand) => Promise<TeamResult> = async command => { this.teamCommands.push(command); return {}; };
  messages: <T>(command: MessageCommand) => Promise<T> = async <T>(command: MessageCommand) => {
    this.commands.push(structuredClone(command));
    if (command.action === 'bots') return { bots: structuredClone(this.bots) } as T;
    if (command.action === 'botStates') return { states: Object.fromEntries(command.triggerIds.map(id => [id, null])) } as T;
    if (command.action === 'read') return { lastReadSeq: command.lastReadSeq } as T;
    return {} as T;
  };

  readonly api: TeamsApi = {
    list: () => this.list(), detail: id => this.detail(id), execute: command => this.execute(command),
    messages: <T>(command: MessageCommand) => this.messages<T>(command),
    onChanged: callback => { this.changed.add(callback); return () => { this.changed.delete(callback); }; },
    onMessagesChanged: callback => { this.signals.add(callback); return () => { this.signals.delete(callback); }; },
    watchMessages: (scope, conversationId, next) => {
      const watch: Watch = { scope, conversationId, next, open: true };
      this.watches.push(watch);
      queueMicrotask(() => { if (watch.open) this.deliver(watch); });
      return () => { watch.open = false; };
    },
  };

  /** Send a watch its current directory (and thread, when it follows a conversation). */
  deliver(watch: Watch, fromCache = false): void {
    const { teamId } = watch.scope;
    watch.next({ uid: this.viewer, ...watch.scope, kind: 'directory', fromCache, conversations: structuredClone(this.conversations.get(teamId) ?? []) });
    if (watch.conversationId) watch.next({ uid: this.viewer, ...watch.scope, kind: 'messages', conversationId: watch.conversationId, fromCache, replace: true, messages: structuredClone(this.threads.get(watch.conversationId) ?? []) });
  }
  open(teamId: string, conversationId?: string): Watch[] {
    return this.watches.filter(watch => watch.open && watch.scope.teamId === teamId && (conversationId === undefined || watch.conversationId === conversationId));
  }
  get openWatches(): number { return this.watches.filter(watch => watch.open).length; }
  emit(watch: Watch, snapshot: SnapshotBody): void {
    watch.next({ uid: this.viewer, ...watch.scope, fromCache: false, ...snapshot } as MessageSnapshot);
  }
  invalidate(invalidated = true): void { for (const callback of [...this.changed]) callback({ invalidated }); }
  signal(signal?: MessageSignal): void { for (const callback of [...this.signals]) callback(signal); }
}
