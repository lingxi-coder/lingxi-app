export interface ActiveFileMention {
  start: number;
  end: number;
  query: string;
}

/** Locate the `@fragment` currently being edited at the rich-text caret. */
export function activeFileMention(text: string, cursor: number): ActiveFileMention | null {
  const caret = Math.max(0, Math.min(cursor, text.length));
  const quotedStart = text.lastIndexOf('@"', caret);
  if (
    quotedStart >= 0
    && (quotedStart === 0 || /\s/.test(text[quotedStart - 1]!))
    && !text.slice(quotedStart + 2, caret).includes('"')
  ) {
    const closingQuote = text.indexOf('"', caret);
    return {
      start: quotedStart,
      end: closingQuote >= 0 ? closingQuote + 1 : caret,
      query: text.slice(quotedStart + 2, caret),
    };
  }

  let start = caret;
  while (start > 0 && !/\s/.test(text[start - 1]!)) start -= 1;
  if (start >= caret || text[start] !== '@') return null;
  let end = caret;
  while (end < text.length && !/\s/.test(text[end]!)) end += 1;
  return { start, end, query: text.slice(start + 1, caret) };
}

/** Encode paths with whitespace as a quoted mention while keeping ordinary paths compact. */
export function fileMentionToken(path: string): string {
  return /\s/.test(path)
    ? `@"${path.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
    : `@${path}`;
}

/** Serialize rich file tokens into the engine prompt exactly once. */
export function promptWithFileMentions(text: string, paths: string[]): string {
  const body = text.trim();
  const mentions = [...new Set(paths)].map(fileMentionToken).join(' ');
  if (!mentions) return body;
  if (!body) return mentions;
  return `${mentions}\n\n${body}`;
}

/** Recover the leading mention line written by {@link promptWithFileMentions}. */
export function fileMentionsFromPrompt(text: string): string[] {
  const separator = text.indexOf('\n\n');
  const prefix = (separator >= 0 ? text.slice(0, separator) : text).trim();
  if (!prefix.startsWith('@')) return [];
  const paths: string[] = [];
  let index = 0;
  while (index < prefix.length) {
    while (/\s/.test(prefix[index] ?? '')) index += 1;
    if (index >= prefix.length) break;
    if (prefix[index] !== '@') return [];
    index += 1;
    let path = '';
    if (prefix[index] === '"') {
      index += 1;
      let closed = false;
      while (index < prefix.length) {
        const character = prefix[index]!;
        index += 1;
        if (character === '"') {
          closed = true;
          break;
        }
        if (character === '\\') {
          if (index >= prefix.length) return [];
          path += prefix[index]!;
          index += 1;
        } else {
          path += character;
        }
      }
      if (!closed) return [];
      if (index < prefix.length && !/\s/.test(prefix[index]!)) return [];
    } else {
      const start = index;
      while (index < prefix.length && !/\s/.test(prefix[index]!)) index += 1;
      path = prefix.slice(start, index);
    }
    if (!path || path.includes('\0')) return [];
    paths.push(path);
  }
  return [...new Set(paths)];
}
