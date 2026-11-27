import type { DeepLinkTarget } from '../../../../packages/deep-links/index';
import type { UseBridge } from './bridge/bridgeTypes';
import type { RuntimeCenterItemRef } from './bridge/runtimeCenterState';
import type { SubmittedPlan } from './bridge/submittedPlan';
import { planRuntimeItemId } from './bridge/runtimeCenterState';
import type { SharedBotCard } from '../../../../packages/teams/execution';
import type { TeamConversation } from '../../../../packages/teams/messages';
import { findBotDm } from '../../../../packages/teams/workspace';

export interface SettingsLinkRequest {
  requestId: string;
  kind: 'mcp' | 'skill' | 'plugin' | 'provider';
  id?: string;
  scope?: string;
  payload?: Record<string, unknown>;
}
/**
 * Where the Scheduled page goes: to a task or one of its runs, or to the editor
 * for a new task in a scope (as a Bot's routines panel asks).
 */
export type ScheduledLinkRequest =
  | { requestId: string; kind: 'job' | 'run'; id: string; scope?: string }
  | { requestId: string; kind: 'new'; scope: string };

export function settingsLinkRequest(target: DeepLinkTarget, requestId: string): { pageId: string; linkRequest: SettingsLinkRequest } | null {
  const [kind, entity, id] = target.route.split('/');
  if (kind !== 'extensions' && kind !== 'import') return null;
  const pages: Record<string, string> = { mcp: 'mcp', skill: 'skills', plugin: 'plugins', provider: 'custom-providers' };
  const pageId = pages[entity!];
  if (!pageId) return null;
  if (kind === 'import' && !target.payload) throw new Error('This import link has no configuration to preview.');
  return { pageId, linkRequest: { requestId, kind: entity as SettingsLinkRequest['kind'], id, scope: target.params.scope, payload: target.payload } };
}

/** Resolve against current local data. Missing identifiers never become a generic section. */
export function runtimeLinkItem(kind: string, id: string, bridge: Pick<UseBridge, 'runtimeCenter' | 'desktop' | 'conversation'>, plans: readonly SubmittedPlan[]): RuntimeCenterItemRef | 'activity' {
  if (kind === 'activity' && id === bridge.conversation.sessionKey && id) return 'activity';
  if (kind === 'agent' && bridge.runtimeCenter.agents[id]) return { kind: 'agent', id };
  if (kind === 'task' && bridge.desktop.tasks[id]) return { kind: 'task', id };
  if (kind === 'plan') {
    const document = plans.find(plan => plan.id === id);
    if (document) return { kind: 'plan-document', id: document.id };
    const plan = bridge.runtimeCenter.plan;
    const index = plan.findIndex((task, index) => task.id === id || planRuntimeItemId(task, index, plan) === id);
    if (index >= 0) return { kind: 'plan', id: planRuntimeItemId(plan[index]!, index, plan) };
  }
  if (kind === 'review') {
    const turn = bridge.conversation.items.find(item => item.type === 'meta' && item.id === id);
    if (turn?.type === 'meta' && turn.files?.length) return { kind: 'turn-review', id, files: turn.files };
    if (id === 'workspace') return { kind: 'section', id: 'review' };
  }
  throw new Error(`This ${kind} is unavailable in the current local conversation.`);
}

export function terminalCommandInput(command: string): string {
  if (!command.trim() || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(command)) throw new Error('Enter a command without terminal control characters.');
  // Bracketed paste prevents embedded line breaks from executing part of a reviewed command.
  return `\x1b[200~${command.replace(/\r\n?/g, '\n')}\x1b[201~\r`;
}

/** A shared team conversation or team Bot (`team/chat`, `team/bot`). A friendship's links name its pair team (`f_…`). */
export type TeamLinkRequest =
  | { kind: 'conversation'; teamId: string; conversationId: string; postId?: string }
  | { kind: 'bot'; teamId: string; botId: string }
  | { kind: 'pair'; teamId: string };

export function teamLinkRequest(target: DeepLinkTarget): TeamLinkRequest | null {
  const { team, conversation, bot } = target.params;
  if (target.route === 'team/post') return { kind: 'conversation', teamId: team!, conversationId: conversation!, postId: target.params.post! };
  if (target.route !== 'team/chat' && target.route !== 'team/bot') return null;
  if (team!.startsWith('f_')) return { kind: 'pair', teamId: team! };
  return target.route === 'team/chat' ? { kind: 'conversation', teamId: team!, conversationId: conversation! } : { kind: 'bot', teamId: team!, botId: bot! };
}

interface TeamLinkSnapshot {
  loading: boolean; uid: string; selectedTeamId: string; botsLoaded: boolean; messagingConnecting: boolean;
  teams: readonly { id: string }[]; bots: readonly SharedBotCard[]; conversations: readonly TeamConversation[];
}
/**
 * The next step toward a team link once Teams can carry it out. A team the viewer is not in is `unavailable`, never another team.
 * A conversation opens once the team's conversations are in, and one the viewer cannot see is `unavailable` rather than the
 * team's home. A Bot opens the viewer's own chat with it when there is one, else the team's Bots, where starting a chat is
 * the viewer's choice; selecting its team (`team`) keeps the conversation that team remembers.
 */
export type TeamLinkStep =
  | { step: 'wait' } | { step: 'unavailable' }
  | { step: 'open'; teamId: string; conversationId: string }
  | { step: 'team'; teamId: string }
  | { step: 'select'; conversationId: string } | { step: 'bots' };

export function teamLinkStep(request: Exclude<TeamLinkRequest, { kind: 'pair' }>, snapshot: TeamLinkSnapshot): TeamLinkStep {
  if (snapshot.loading) return { step: 'wait' };
  if (!snapshot.teams.some(team => team.id === request.teamId)) return { step: 'unavailable' };
  if (snapshot.selectedTeamId !== request.teamId) return request.kind === 'conversation'
    ? { step: 'open', teamId: request.teamId, conversationId: request.conversationId } : { step: 'team', teamId: request.teamId };
  if (request.kind === 'conversation') {
    if (snapshot.messagingConnecting) return { step: 'wait' };
    return snapshot.conversations.some(conversation => conversation.id === request.conversationId)
      ? { step: 'select', conversationId: request.conversationId } : { step: 'unavailable' };
  }
  if (!snapshot.botsLoaded || snapshot.messagingConnecting) return { step: 'wait' };
  if (!snapshot.bots.some(card => card.sharedBotId === request.botId)) return { step: 'unavailable' };
  const existing = findBotDm(snapshot.conversations, snapshot.uid, request.botId, snapshot.bots);
  return existing ? { step: 'select', conversationId: existing.id } : { step: 'bots' };
}

/** The friend whose pair team a link names, once the cards are in: `undefined` while waiting, `null` for no such friendship. */
export function pairLinkFriend(teamId: string, snapshot: { loading: boolean; cards: readonly { uid: string; state: string; teamId?: string }[] } | undefined): string | null | undefined {
  if (!snapshot || snapshot.loading) return undefined;
  return snapshot.cards.find(card => card.state === 'friends' && card.teamId === teamId)?.uid ?? null;
}
