/** Display-only projection; keep the original envelope in conversation history. */
export function crossSessionMessage(text: string): { text: string; label: string } | null {
  const envelope = /^\s*<cross-session-message(?=\s|>)([^>]*)>([\s\S]*)<\/cross-session-message>\s*$/.exec(text);
  if (!envelope) return null;
  const attribute = (name: string) => {
    const value = new RegExp(`(?:^|\\s)${name}="([^"]*)"`).exec(envelope[1]!)?.[1];
    return value?.replace(/&(quot|apos|lt|gt|amp);/g, (_, entity: string) =>
      ({ quot: '"', apos: "'", lt: '<', gt: '>', amp: '&' })[entity]!).trim();
  };
  const sender = attribute('from-name') || attribute('from') || attribute('from-session');
  return { text: envelope[2]!.trim(), label: sender ? `来自其他会话 · ${sender}` : '来自其他会话' };
}
