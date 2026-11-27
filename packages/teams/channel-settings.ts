import type { MessageScope, TeamConversation, TeamMessage } from './messages.js';

export interface ChannelPosting {
  mode: 'chat' | 'posts';
  senders: 'all' | 'selected';
  selected: string[];
  replies: 'all' | 'authors' | 'none';
}
export const DEFAULT_POSTING: ChannelPosting = { mode: 'chat', senders: 'all', selected: [], replies: 'all' };

export type ChannelRequest = { op: 'list' } | ({ conversationId: string } & (
  { op: 'get' | 'createInvite' } | { op: 'profile'; title: string; description: string }
  | { op: 'posting'; posting: ChannelPosting } | { op: 'admin'; uid: string; enabled: boolean }
  | { op: 'remove'; uid: string } | { op: 'detachBot'; botId: string }
  | { op: 'revokeInvite' | 'redeem'; code: string }
));
export type ChannelCommand = MessageScope & { action: 'channelSettings'; request: ChannelRequest };
export const channelCommand = (scope: MessageScope, request: ChannelRequest): ChannelCommand => ({ ...scope, action: 'channelSettings', request });
export interface ChannelManagement { members: string[]; admins: string[]; code: string | null }
export interface ChannelInvite { teamId: string; conversationId: string; code: string }
// The website can redeem channel invitations on Desktop and on phones whose native clients only understand team invitations.
export const channelInvitePath = ({ teamId, conversationId, code }: ChannelInvite) =>
  '/teams/join?' + new URLSearchParams({ channelTeam: teamId, channel: conversationId, channelCode: code });
export const channelInviteLink = (invite: ChannelInvite) => 'https://lingxios.net' + channelInvitePath(invite);

export function canChannelSend(channel: TeamConversation, uid: string, target?: TeamMessage): boolean {
  if (channel.kind !== 'channel') return true;
  const actor = 'user:' + uid;
  if (!channel.participants.includes(actor)) return false;
  const posting = channel.posting ?? DEFAULT_POSTING;
  const start = posting.senders === 'all' || posting.selected.includes(actor);
  if (!target) return start;
  if (target.channelPost || target.postId) return posting.mode === 'posts' && (posting.replies === 'all' || (posting.replies === 'authors' && start));
  return posting.mode === 'chat' && start;
}
