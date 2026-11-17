import type { RenderAvatar } from '../../../../packages/teams/ui';
import { AGENT_AVATARS, agentAvatarIndex, teamBotAvatarId } from '../remote/reuse';

/**
 * Bots wear the avatar they have on the Desktop (`TeamAvatar.tsx` there): the same art, picked from the same id by the same
 * rule, so a Bot looks alike in a team, in a friend chat and in the Desktop's Bots list. It sits in the workspace's avatar
 * box, which `teams-web.css` rounds to 24% of its side (0.24 × the size). People get the workspace's own initials on a
 * circle, which is what drawing nothing asks for.
 *
 * The site's theme is an attribute of the page and not a React context, so both variants of the art are drawn and the
 * stylesheet shows the one of the theme, as the Remote chat does for its agents.
 */
export const renderTeamAvatar: RenderAvatar = (identity, size) => {
  if (identity.kind !== 'bot') return undefined;
  const index = agentAvatarIndex(teamBotAvatarId(identity.id));
  const art = AGENT_AVATARS[index];
  return <>
    <img className="tw-bot-art" data-art="light" data-agent-avatar={index} src={art.light} width={size} height={size} alt="" draggable={false} />
    <img className="tw-bot-art" data-art="dark" data-agent-avatar={index} src={art.dark} width={size} height={size} alt="" draggable={false} />
  </>;
};
