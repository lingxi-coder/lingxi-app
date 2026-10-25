import type { ReactNode } from 'react';

/**
 * A small Markdown renderer for assistant text. It builds React elements and
 * never an HTML string, so everything the model wrote reaches the page as text;
 * links are limited to http, https and mailto. It covers what chat answers use:
 * paragraphs, headings, fenced and inline code, emphasis, links, lists,
 * quotes and rules. Anything else is shown as the plain text it is.
 */

export type Inline =
  | { type: 'text'; text: string }
  | { type: 'code'; text: string }
  | { type: 'strong'; children: Inline[] }
  | { type: 'em'; children: Inline[] }
  | { type: 'link'; href: string; children: Inline[] }
  | { type: 'br' };

export type Block =
  | { type: 'paragraph'; children: Inline[] }
  | { type: 'heading'; level: 1 | 2 | 3 | 4 | 5 | 6; children: Inline[] }
  | { type: 'code'; lang: string; text: string }
  | { type: 'list'; ordered: boolean; items: Inline[][] }
  | { type: 'quote'; blocks: Block[] }
  | { type: 'rule' };

const SAFE_PROTOCOLS = new Set(['http:', 'https:', 'mailto:']);

/** The link target if it is safe to follow, else `undefined`. Relative links are not followed either. */
export function safeHref(raw: string): string | undefined {
  const value = raw.trim();
  if (value.length === 0 || value.length > 2048) return undefined;
  try {
    const url = new URL(value);
    return SAFE_PROTOCOLS.has(url.protocol) ? url.href : undefined;
  } catch {
    return undefined;
  }
}

const FENCE = /^(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/u;
const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/u;
const BULLET = /^\s{0,3}[-*+]\s+(.*)$/u;
const NUMBERED = /^\s{0,3}\d{1,9}[.)]\s+(.*)$/u;
const RULE = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/u;
const QUOTE = /^\s{0,3}>\s?(.*)$/u;

export function parseMarkdown(source: string): Block[] {
  return parseBlocks(source.replace(/\r\n?/gu, '\n').split('\n'));
}

function parseBlocks(lines: string[]): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  const flush = (): void => {
    if (paragraph.length > 0) blocks.push({ type: 'paragraph', children: parseInline(paragraph.join('\n')) });
    paragraph = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const fence = FENCE.exec(line);
    if (fence) {
      flush();
      const marker = fence[1]!;
      const body: string[] = [];
      for (i++; i < lines.length && !lines[i]!.trim().startsWith(marker); i++) body.push(lines[i]!);
      blocks.push({ type: 'code', lang: fence[2] ?? '', text: body.join('\n') });
      continue;
    }
    if (line.trim() === '') {
      flush();
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      flush();
      blocks.push({ type: 'heading', level: heading[1]!.length as 1 | 2 | 3 | 4 | 5 | 6, children: parseInline(heading[2]!) });
      continue;
    }
    if (RULE.test(line)) {
      flush();
      blocks.push({ type: 'rule' });
      continue;
    }
    if (QUOTE.test(line)) {
      flush();
      const quoted: string[] = [];
      for (; i < lines.length && QUOTE.test(lines[i]!); i++) quoted.push(QUOTE.exec(lines[i]!)![1]!);
      i--;
      blocks.push({ type: 'quote', blocks: parseBlocks(quoted) });
      continue;
    }
    const bullet = BULLET.test(line);
    if (bullet || NUMBERED.test(line)) {
      flush();
      const pattern = bullet ? BULLET : NUMBERED;
      const items: Inline[][] = [];
      for (; i < lines.length && pattern.test(lines[i]!); i++) {
        const parts = [pattern.exec(lines[i]!)![1]!];
        // Indented lines continue the item.
        while (i + 1 < lines.length && /^\s{2,}\S/u.test(lines[i + 1]!) && !pattern.test(lines[i + 1]!)) parts.push(lines[++i]!.trim());
        items.push(parseInline(parts.join('\n')));
      }
      i--;
      blocks.push({ type: 'list', ordered: !bullet, items });
      continue;
    }
    paragraph.push(line);
  }
  flush();
  return blocks;
}

export function parseInline(source: string): Inline[] {
  const out: Inline[] = [];
  let text = '';
  const push = (): void => {
    if (text) out.push({ type: 'text', text });
    text = '';
  };
  for (let i = 0; i < source.length; ) {
    const rest = source.slice(i);
    const ch = source[i]!;
    if (ch === '\\' && i + 1 < source.length && /[\\`*_[\]()#+\-.!~]/u.test(source[i + 1]!)) {
      text += source[i + 1];
      i += 2;
      continue;
    }
    if (ch === '`') {
      const ticks = /^`+/u.exec(rest)![0];
      const end = source.indexOf(ticks, i + ticks.length);
      if (end > i + ticks.length) {
        push();
        out.push({ type: 'code', text: source.slice(i + ticks.length, end).replace(/^ (.*) $/su, '$1') });
        i = end + ticks.length;
        continue;
      }
    }
    if (ch === '*' || ch === '_') {
      const double = source.startsWith(ch + ch, i);
      const marker = double ? ch + ch : ch;
      const end = source.indexOf(marker, i + marker.length);
      const inner = end > i + marker.length ? source.slice(i + marker.length, end) : '';
      // `_` inside a word (snake_case) is not emphasis; neither is a marker followed by a space.
      const wordInternal = ch === '_' && /\w/u.test(source[i - 1] ?? '');
      if (inner !== '' && !/^\s|\s$/u.test(inner) && !wordInternal) {
        push();
        out.push({ type: double ? 'strong' : 'em', children: parseInline(inner) });
        i = end + marker.length;
        continue;
      }
    }
    if (ch === '[') {
      const link = /^\[([^\]]+)\]\(\s*((?:[^\s()]|\([^\s()]*\))+)(?:\s+"[^"]*")?\s*\)/u.exec(rest);
      if (link) {
        const href = safeHref(link[2]!);
        push();
        if (href === undefined) out.push(...parseInline(link[1]!)); // an unsafe target leaves just the label
        else out.push({ type: 'link', href, children: parseInline(link[1]!) });
        i += link[0].length;
        continue;
      }
    }
    if (ch === 'h') {
      const bare = /^https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"]/u.exec(rest);
      if (bare) {
        const href = safeHref(bare[0]);
        if (href !== undefined) {
          push();
          out.push({ type: 'link', href, children: [{ type: 'text', text: bare[0] }] });
          i += bare[0].length;
          continue;
        }
      }
    }
    if (ch === '\n') {
      push();
      out.push({ type: 'br' });
      i += 1;
      continue;
    }
    text += ch;
    i += 1;
  }
  push();
  return out;
}

function renderInline(nodes: Inline[]): ReactNode[] {
  return nodes.map((node, index) => {
    switch (node.type) {
      case 'text':
        return node.text;
      case 'code':
        return <code key={index}>{node.text}</code>;
      case 'strong':
        return <strong key={index}>{renderInline(node.children)}</strong>;
      case 'em':
        return <em key={index}>{renderInline(node.children)}</em>;
      case 'link':
        return (
          <a key={index} href={node.href} target="_blank" rel="noopener noreferrer">
            {renderInline(node.children)}
          </a>
        );
      case 'br':
        return <br key={index} />;
    }
  });
}

function renderBlocks(blocks: Block[]): ReactNode[] {
  return blocks.map((block, index) => {
    switch (block.type) {
      case 'paragraph':
        return <p key={index}>{renderInline(block.children)}</p>;
      case 'heading': {
        const Tag = `h${Math.min(6, block.level + 2)}` as 'h3';
        return <Tag key={index}>{renderInline(block.children)}</Tag>;
      }
      case 'code':
        return (
          <pre key={index} className="rc-code" data-lang={block.lang || undefined}>
            <code>{block.text}</code>
          </pre>
        );
      case 'list': {
        const List = block.ordered ? 'ol' : 'ul';
        return (
          <List key={index}>
            {block.items.map((item, i) => (
              <li key={i}>{renderInline(item)}</li>
            ))}
          </List>
        );
      }
      case 'quote':
        return <blockquote key={index}>{renderBlocks(block.blocks)}</blockquote>;
      case 'rule':
        return <hr key={index} />;
    }
  });
}

export function Markdown({ text }: { text: string }) {
  return <div className="rc-md">{renderBlocks(parseMarkdown(text))}</div>;
}
