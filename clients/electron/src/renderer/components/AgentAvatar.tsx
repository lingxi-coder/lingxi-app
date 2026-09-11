import { useT } from '../theme/ThemeContext';

// Codex's 28 paired SVG avatars, in the original seed-selection order.
const AVATARS = [
  { dark: new URL('../assets/agent-avatars/variant-00-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-00-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-01-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-01-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-02-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-02-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-03-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-03-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-04-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-04-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-05-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-05-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-06-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-06-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-07-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-07-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-08-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-08-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-09-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-09-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-10-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-10-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-11-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-11-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-12-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-12-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-13-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-13-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-14-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-14-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-15-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-15-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-16-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-16-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-17-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-17-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-18-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-18-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-19-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-19-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-20-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-20-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-21-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-21-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-22-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-22-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-23-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-23-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-24-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-24-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-25-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-25-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-26-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-26-light.svg', import.meta.url).href },
  { dark: new URL('../assets/agent-avatars/variant-27-dark.svg', import.meta.url).href, light: new URL('../assets/agent-avatars/variant-27-light.svg', import.meta.url).href },
] as const;

/** Stable across refreshes, renames, status changes, and display surfaces. */
export function agentAvatarIndex(agentId: string): number {
  let hash = 0;
  for (let index = 0; index < agentId.length; index += 1) {
    hash = (hash * 31 + agentId.charCodeAt(index)) % 2147483647;
  }
  return hash % AVATARS.length;
}

export function AgentAvatar({ agentId, size = 20 }: { agentId: string; size?: number }) {
  const t = useT();
  const index = agentAvatarIndex(agentId);
  return <img
    src={AVATARS[index][t.dark ? 'dark' : 'light']}
    data-agent-avatar={index}
    width={size}
    height={size}
    alt=""
    aria-hidden="true"
    draggable={false}
    style={{ display: 'block', flexShrink: 0 }}
  />;
}
