import type { MessageApi, MessageCommand } from './messages';
import type { BindLocalTeamBotInput, LocalTeamBot } from './bots';
export type { TeamBotCommand, TeamBotAnswer, TeamBotRecovery, TeamBotCard, TeamBotState, LocalTeamBot, BindLocalTeamBotInput } from './bots';
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
  | { action: 'role'; teamId: string; uid: string; generation: number; role: 'admin' | 'member' };
export interface TeamResult { teamId?: string; code?: string; generation?: number; alreadyMember?: boolean; ok?: boolean }
export interface TeamsApi extends MessageApi {
  localBots?(): Promise<LocalTeamBot[]>;
  bindLocalBot?(input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }>;
  list(): Promise<{ teams: TeamSummary[] }>;
  detail(teamId: string): Promise<TeamDetail>;
  execute(command: TeamCommand): Promise<TeamResult>;
  /** An invalidated selection must be hidden immediately, before refetching. */
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void;
}
export interface DesktopTeamsApi {
  localBots(uid: string): Promise<LocalTeamBot[]>;
  bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }>;
  list(uid: string): Promise<{ teams: TeamSummary[] }>;
  detail(uid: string, teamId: string): Promise<TeamDetail>;
  execute(uid: string, command: TeamCommand): Promise<TeamResult>;
  onChanged: TeamsApi['onChanged'];
  messages<T>(uid: string, command: MessageCommand): Promise<T>;
  onMessagesChanged: MessageApi['onMessagesChanged'];
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
