import type { RenderAvatar } from '../../../../../packages/teams/ui';
import { AgentAvatar } from './AgentAvatar';
import { teamBotAvatarId } from './agentAvatarArt';

// The rule that names the Bot an identity stands for is in agentAvatarArt.ts, with the art it picks from, so the Web page draws the same.
export { teamBotAvatarId };

/** Bots wear the avatar they have in the Bots sidebar wherever the workspace shows them; people get its initials. */
export const renderTeamAvatar: RenderAvatar = (identity, size) =>
  identity.kind === 'bot' ? <AgentAvatar agentId={teamBotAvatarId(identity.id)} size={size} /> : undefined;
