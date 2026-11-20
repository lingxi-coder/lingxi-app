import type { ChatEvent } from '../../../../../packages/llm-vault/contracts';

/** One frame larger than this is not llmApi talking (its deltas are small): the stream is treated as broken. */
export const MAX_SSE_FRAME_CHARS = 4 * 1024 * 1024;

/**
 * An incremental parser for llmApi's server-sent events: feed it text as it arrives, it returns the events completed.
 * A frame over `MAX_SSE_FRAME_CHARS` ends the stream with one `UNAVAILABLE` error instead of growing without bound.
 */
export function createSseParser(): { push(text: string): ChatEvent[]; end(): ChatEvent[] } {
  let buffer = '', event = '', data: string[] = [], dataChars = 0, broken = false;
  const out: ChatEvent[] = [];
  const flush = () => {
    if (data.length && event) {
      try { out.push({ type: event, ...JSON.parse(data.join('\n')) } as ChatEvent); } catch { /* a malformed frame is skipped */ }
    }
    event = ''; data = []; dataChars = 0;
  };
  const line = (text: string) => {
    if (text === '') { flush(); return; }
    if (text.startsWith(':')) return;
    const colon = text.indexOf(':'), field = colon < 0 ? text : text.slice(0, colon);
    let value = colon < 0 ? '' : text.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') { data.push(value); dataChars += value.length + 1; }
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
  const guard = (events: ChatEvent[]): ChatEvent[] => {
    if (broken) return [];
    if (buffer.length + dataChars <= MAX_SSE_FRAME_CHARS) return events;
    broken = true; buffer = ''; data = []; dataChars = 0;
    return [...events, { type: 'error', code: 'UNAVAILABLE' }];
  };
  return {
    push(text) { if (broken) return []; buffer += text; return guard(drain(false)); },
    end() { if (broken) return []; const events = drain(true); if (buffer) { line(buffer); buffer = ''; } flush(); return [...events, ...out.splice(0)]; },
  };
}
