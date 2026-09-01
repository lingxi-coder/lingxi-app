export type MarkdownBlock =
  | { type: 'paragraph'; text: string }
  | { type: 'heading'; level: number; text: string }
  | { type: 'blockquote'; text: string }
  | { type: 'list'; ordered: boolean; items: string[] }
  | { type: 'code'; language?: string; text: string; closed: boolean };

/**
 * Return a display-safe, two-space formatted JSON object/array only when the
 * complete input is valid JSON. Formatting is lexical rather than
 * parse/stringify based, so duplicate keys and large integer spellings survive
 * unchanged.
 */
export function standaloneJsonForDisplay(source: string): string | undefined {
  const trimmed = source.trim();
  const objectLike = trimmed.startsWith('{') && trimmed.endsWith('}');
  const arrayLike = trimmed.startsWith('[') && trimmed.endsWith(']');
  if (!objectLike && !arrayLike) return undefined;

  try {
    JSON.parse(trimmed);
  } catch {
    return undefined;
  }

  let output = '';
  let depth = 0;
  let inString = false;
  let escaped = false;
  const indentation = () => '  '.repeat(depth);

  for (let index = 0; index < trimmed.length; index += 1) {
    const character = trimmed[index] ?? '';
    if (inString) {
      output += character;
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === '"') inString = false;
      continue;
    }

    if (/\s/.test(character)) continue;
    if (character === '"') {
      inString = true;
      output += character;
      continue;
    }
    if (character === '{' || character === '[') {
      const closing = character === '{' ? '}' : ']';
      let next = index + 1;
      while (next < trimmed.length && /\s/.test(trimmed[next] ?? '')) next += 1;
      if (trimmed[next] === closing) {
        output += `${character}${closing}`;
        index = next;
      } else {
        output += `${character}\n`;
        depth += 1;
        output += indentation();
      }
      continue;
    }
    if (character === '}' || character === ']') {
      depth -= 1;
      output += `\n${indentation()}${character}`;
      continue;
    }
    if (character === ',') {
      output += `,\n${indentation()}`;
      continue;
    }
    if (character === ':') {
      output += ': ';
      continue;
    }
    output += character;
  }

  return output;
}

/**
 * Parse the small, safe Markdown subset used by model messages. The renderer
 * turns this data into React nodes, so source HTML is always treated as text.
 */
export function parseMarkdown(source: string): MarkdownBlock[] {
  const standaloneJson = standaloneJsonForDisplay(source);
  if (standaloneJson !== undefined) {
    return [{ type: 'code', language: 'json', text: standaloneJson, closed: true }];
  }

  const lines = normalizeMarkdown(source).split('\n');
  const blocks: MarkdownBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? '';
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const fence = line.match(/^ {0,3}(`{3,}|~{3,})\s*([^ ]*)\s*$/);
    if (fence) {
      const marker = fence[1];
      const content: string[] = [];
      index += 1;
      while (index < lines.length && !new RegExp(`^ {0,3}${marker[0]}{${marker.length},}\\s*$`).test(lines[index] ?? '')) {
        content.push(lines[index] ?? '');
        index += 1;
      }
      const closed = index < lines.length;
      if (closed) index += 1;
      blocks.push({ type: 'code', language: fence[2] || undefined, text: content.join('\n'), closed });
      continue;
    }

    const heading = line.match(/^ {0,3}(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      blocks.push({ type: 'heading', level: heading[1].length, text: heading[2] });
      index += 1;
      continue;
    }

    if (/^ {0,3}>/.test(line)) {
      const quote: string[] = [];
      while (index < lines.length && /^ {0,3}>/.test(lines[index] ?? '')) {
        quote.push((lines[index] ?? '').replace(/^ {0,3}>\s?/, ''));
        index += 1;
      }
      blocks.push({ type: 'blockquote', text: quote.join('\n') });
      continue;
    }

    const list = line.match(/^ {0,3}([-+*])\s+(.+)$/) ?? line.match(/^ {0,3}(\d+)[.)]\s+(.+)$/);
    if (list) {
      const ordered = /^\d/.test(list[1]);
      const items: string[] = [];
      while (index < lines.length) {
        const candidate = lines[index] ?? '';
        const match = ordered
          ? candidate.match(/^ {0,3}\d+[.)]\s+(.+)$/)
          : candidate.match(/^ {0,3}[-+*]\s+(.+)$/);
        if (match) {
          items.push(match[1]);
          index += 1;
          continue;
        }
        const continuation = candidate.match(/^ {2,}(.+)$/);
        if (continuation && items.length > 0) {
          items[items.length - 1] += `\n${continuation[1]}`;
          index += 1;
          continue;
        }
        break;
      }
      blocks.push({ type: 'list', ordered, items });
      continue;
    }

    const paragraph: string[] = [line];
    index += 1;
    while (index < lines.length) {
      const next = lines[index] ?? '';
      if (!next.trim() || isBlockStart(next)) break;
      paragraph.push(next);
      index += 1;
    }
    blocks.push({ type: 'paragraph', text: paragraph.join('\n') });
  }

  return blocks;
}

/** Split the common `intro - **item** - **item**` model output into a list. */
export function normalizeMarkdown(source: string): string {
  return source
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .flatMap((line) => splitLooseBullets(line))
    .join('\n');
}

function splitLooseBullets(line: string): string[] {
  const markers = [...line.matchAll(/\s+[-+*]\s+(?=\*\*|__|`|~~|\[[^\]]+\]\()/g)];
  if (markers.length < 2) return [line];
  const first = markers[0];
  const firstIndex = first.index ?? 0;
  const result = [line.slice(0, firstIndex).trimEnd()];
  markers.forEach((marker, markerIndex) => {
    const start = (marker.index ?? 0) + marker[0].length;
    const nextIndex = markers[markerIndex + 1]?.index ?? line.length;
    const item = line.slice(start, nextIndex).trim();
    if (item) result.push(`- ${item}`);
  });
  return result;
}

function isBlockStart(line: string): boolean {
  return /^ {0,3}(#{1,6})\s+/.test(line)
    || /^ {0,3}>/.test(line)
    || /^ {0,3}(`{3,}|~{3,})/.test(line)
    || /^ {0,3}[-+*]\s+/.test(line)
    || /^ {0,3}\d+[.)]\s+/.test(line);
}
