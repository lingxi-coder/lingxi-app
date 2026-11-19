import type { ChatEvent } from '../../../../../packages/llm-vault/contracts';

/** An incremental parser for llmApi's server-sent events: feed it text as it arrives, it returns the events completed. */
export function createSseParser(): { push(text: string): ChatEvent[]; end(): ChatEvent[] } {
  let buffer = '', event = '', data: string[] = [];
  const out: ChatEvent[] = [];
  const flush = () => {
    if (data.length && event) {
      try { out.push({ type: event, ...JSON.parse(data.join('\n')) } as ChatEvent); } catch { /* a malformed frame is skipped */ }
    }
    event = ''; data = [];
  };
  const line = (text: string) => {
    if (text === '') { flush(); return; }
    if (text.startsWith(':')) return;
    const colon = text.indexOf(':'), field = colon < 0 ? text : text.slice(0, colon);
    let value = colon < 0 ? '' : text.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') data.push(value);
  };
  const drain = (final: boolean) => {
    for (;;) {
      const match = /\r\n|\r|\n/u.exec(buffer);
      if (!match || (!final && match[0] === '\r' && match.index === buffer.length - 1)) break;
      line(buffer.slice(0, match.index));
      buffer = buffer.slice(match.index + match[0].length);
    }
    return out.splice(0);
  };
  return {
    push(text) { buffer += text; return drain(false); },
    end() { const events = drain(true); if (buffer) { line(buffer); buffer = ''; } flush(); return [...events, ...out.splice(0)]; },
  };
}
