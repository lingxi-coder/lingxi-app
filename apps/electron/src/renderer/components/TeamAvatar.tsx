import type { RenderAvatar } from '../../../../../packages/teams/ui';
import { AgentAvatar } from './AgentAvatar';

/**
 * The id a Bot's avatar is drawn from. The workspace names a Bot by its id once it has the Bot's card and by its key
 * (`bot:<owner>:<host>:<id>`) before then, so the id is the last part of a key: the same Bot has the same avatar
 * while its card loads, and in the Bots sidebar, which draws it from the id too.
 */
export function teamBotAvatarId(identityId: string): string {
  return identityId.startsWith('bot:') ? identityId.slice(identityId.lastIndexOf(':') + 1) : identityId;
}

/** Bots wear the avatar they have in the Bots sidebar wherever the workspace shows them; people get its initials. */
export const renderTeamAvatar: RenderAvatar = (identity, size) =>
  identity.kind === 'bot' ? <AgentAvatar agentId={teamBotAvatarId(identity.id)} size={size} /> : undefined;
