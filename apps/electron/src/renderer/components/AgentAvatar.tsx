import { useT } from '../theme/ThemeContext';

import { AGENT_AVATARS, agentAvatarIndex } from './agentAvatarArt';
export { agentAvatarIndex } from './agentAvatarArt';

export function AgentAvatar({ agentId, size = 20 }: { agentId: string; size?: number }) {
  const t = useT();
  const index = agentAvatarIndex(agentId);
  return <img
    src={AGENT_AVATARS[index][t.dark ? 'dark' : 'light']}
    data-agent-avatar={index}
    width={size}
    height={size}
    alt=""
    aria-hidden="true"
    draggable={false}
    style={{ display: 'block', flexShrink: 0 }}
  />;
}
