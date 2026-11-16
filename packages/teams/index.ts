import type { MessageApi, MessageCommand } from './messages';
import type { LocalPublishableBot, SelectedSharedFile, PublishLocalBotInput, PublishedKnowledge, SharedHostStatus, OfferSharedBotInput, AcceptSharedBotInput } from './execution';
export * from './execution';
export type TeamRole = 'owner' | 'admin' | 'member';
export interface TeamSummary { id: string; name: string; memberCount: number; role: TeamRole; generation: number }
export interface TeamMember { uid: string; displayName: string; role: TeamRole; generation: number }
export interface TeamInvite { id: string; role: 'admin' | 'member'; expiresAt: string; uses: number; maxUses: number; revoked: boolean }
export interface TeamDetail { team: TeamSummary; members: TeamMember[]; invites: TeamInvite[] }
export type TeamCommand =
  | { action: 'create'; teamId: string; name: string }
  | { action: 'delete'; teamId: string }
  | { action: 'invite'; teamId: string; requestId: string; role: 'admin' | 'member'; maxUses: number; ttlSeconds: number }
  | { action: 'revokeInvite'; teamId: string; inviteId: string }
  | { action: 'join'; code: string; requestId: string }
  | { action: 'leave'; teamId: string; generation: number }
  | { action: 'remove'; teamId: string; uid: string; generation: number }
  | { action: 'role'; teamId: string; uid: string; generation: number; role: 'admin' | 'member' }
  /** The owner gives the team to a current member and stays as an admin. Bots, Chats and model accounts stay where they are. */
  | { action: 'handOver'; teamId: string; uid: string; generation: number };
export interface TeamResult { teamId?: string; code?: string; generation?: number; alreadyMember?: boolean; ok?: boolean }
export interface TeamsApi extends MessageApi {
  openBotMarket?(): Promise<void>;
  localBots?(): Promise<LocalPublishableBot[]>;
  selectSharedFiles?(instanceId: string): Promise<SelectedSharedFile[]>;
  /** The role and files of a Bot this Desktop publishes; absent off Desktop. */
  publishedKnowledge?(teamId: string, sharedBotId: string): Promise<PublishedKnowledge>;
  publishLocalBot?(input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }>;
  /** Offer a Bot this Desktop publishes to another member; absent off Desktop, where its role and files are not held. */
  offerSharedBot?(input: OfferSharedBotInput): Promise<void>;
  /** Take over a Bot offered to this person, hosted by one of this Desktop's Bots; absent off Desktop. */
  acceptSharedBot?(input: AcceptSharedBotInput): Promise<{ sharedBotId: string; revision: number }>;
  /** How this Desktop serves the Bots it publishes; absent off Desktop. */
  hostStatus?(): Promise<SharedHostStatus>;
  /** Change what keeps this Desktop serving while its owner is away. The system may still ask the owner to allow opening at login. */
  setHostBackground?(change: { openAtLogin?: boolean; keepAwake?: boolean }): Promise<void>;
  /** Show where the system lets the owner allow the app to open at login. */
  openLoginItems?(): Promise<void>;
  /** Hosted Bot services (Bots offered outside Teams): a command about a service, and a session or task command in a service's scope. */
  services?<T>(command: import('./hosted-service.js').ServiceCommand): Promise<T>;
  serviceMessages?<T>(command: Record<string, unknown> & { serviceId: string; generation: number; action: string }): Promise<T>;
  list(): Promise<{ teams: TeamSummary[] }>;
  detail(teamId: string): Promise<TeamDetail>;
  execute(command: TeamCommand): Promise<TeamResult>;
  /** An invalidated selection must be hidden immediately, before refetching. */
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void;
}
export interface DesktopTeamsApi {
  openBotMarket(uid: string): Promise<void>;
  localBots(uid: string): Promise<LocalPublishableBot[]>;
  selectSharedFiles(uid: string, instanceId: string): Promise<SelectedSharedFile[]>;
  publishedKnowledge(uid: string, teamId: string, sharedBotId: string): Promise<PublishedKnowledge>;
  publishLocalBot(uid: string, input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }>;
  hostStatus(uid: string): Promise<SharedHostStatus>;
  offerSharedBot(uid: string, input: OfferSharedBotInput): Promise<void>;
  acceptSharedBot(uid: string, input: AcceptSharedBotInput): Promise<{ sharedBotId: string; revision: number }>;
  list(uid: string): Promise<{ teams: TeamSummary[] }>;
  detail(uid: string, teamId: string): Promise<TeamDetail>;
  execute(uid: string, command: TeamCommand): Promise<TeamResult>;
  onChanged: TeamsApi['onChanged'];
  messages<T>(uid: string, command: MessageCommand): Promise<T>;
  services<T>(uid: string, command: import('./hosted-service.js').ServiceCommand): Promise<T>;
  serviceMessages<T>(uid: string, command: Record<string, unknown> & { serviceId: string; generation: number; action: string }): Promise<T>;
  onMessagesChanged: MessageApi['onMessagesChanged'];
  watchMessages(uid: string, scope: import('./messages').MessageScope, conversationId: string, next: (snapshot: import('./messages').MessageSnapshot) => void): () => void;
}
export class TeamsApiError extends Error {
  constructor(public readonly code: string, public readonly status: number) { super(code); }
}
export function teamsTransport(base: string, token: () => Promise<string>, fetcher: typeof fetch = fetch) {
  return async <T>(path = '', body?: unknown): Promise<T> => {
    const credential = await token();
    const response = await fetcher(`${base}/v1/teams${path}`, { method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: `Bearer ${credential}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(20000) });
    const value = await response.json();
    if (!response.ok) throw new TeamsApiError(value?.error?.code ?? 'SERVICE_UNAVAILABLE', response.status);
    return value as T;
  };
}
export function inviteCode(input: string): string | null {
  const value = input.trim();
  if (/^[A-Za-z0-9_-]{32}$/u.test(value)) return value;
  try {
    const url = new URL(value);
    if (url.username || url.password || url.port || url.hash) return null;
    if (!(url.protocol === 'lingxi:' && url.hostname === 'team' && url.pathname === '/join')
      && !(url.protocol === 'https:' && url.hostname === 'lingxios.net' && ['/open/team/join', '/teams/join'].includes(url.pathname))) return null;
    const entries = [...url.searchParams];
    return entries.length === 1 && entries[0]![0] === 'code' && /^[A-Za-z0-9_-]{32}$/u.test(entries[0]![1]) ? entries[0]![1] : null;
  } catch { return null; }
}
export function inviteLink(code: string): string {
  if (!/^[A-Za-z0-9_-]{32}$/u.test(code)) throw new Error('Invalid invite');
  return `https://lingxios.net/open/team/join?code=${code}`;
}
