import type { ReactNode } from 'react';

import { parseMarkdown, type MarkdownBlock } from '../markdown';
import { useT } from '../theme/ThemeContext';
import { CodeBlock } from './CodeBlock';
import { Icon } from './Icon';

export function MarkdownContent({ text }: { text: string }) {
  const t = useT();
  const blocks = parseMarkdown(text);
  return (
    <div style={{ display: 'grid', gap: 10, minWidth: 0 }}>
      {blocks.map((block, index) => <MarkdownBlockView key={index} block={block} t={t} />)}
    </div>
  );
}

function MarkdownBlockView({ block, t }: { block: MarkdownBlock; t: ReturnType<typeof useT> }): ReactNode {
  if (block.type === 'paragraph') {
    return <p style={{ margin: 0, whiteSpace: 'pre-wrap', textWrap: 'pretty' }}><InlineMarkdown text={block.text} t={t} /></p>;
  }
  if (block.type === 'heading') {
    const sizes = [0, 20, 17, 15.5, 14.5, 14, 13.5];
    return <div style={{ margin: '3px 0 0', color: t.text, fontSize: sizes[block.level], lineHeight: 1.35, fontWeight: 600, letterSpacing: '-.012em', textWrap: 'balance' }}><InlineMarkdown text={block.text} t={t} /></div>;
  }
  if (block.type === 'blockquote') {
    return <blockquote style={{ margin: 0, padding: '2px 0 2px 12px', borderLeft: `3px solid ${t.accentBorder}`, color: t.text2 }}><MarkdownContent text={block.text} /></blockquote>;
  }
  if (block.type === 'list') {
    const Tag = block.ordered ? 'ol' : 'ul';
    return <Tag style={{ margin: 0, paddingLeft: 22, display: 'grid', gap: 4 }}>{block.items.map((item, index) => <li key={index} style={{ paddingLeft: 3 }}><InlineMarkdown text={item} t={t} /></li>)}</Tag>;
  }
  return <CodeBlock code={block.text} language={block.language} closed={block.closed} />;
}

function InlineMarkdown({ text, t }: { text: string; t: ReturnType<typeof useT> }) {
  return <>{renderInline(text, t)}</>;
}

function renderInline(text: string, t: ReturnType<typeof useT>): ReactNode[] {
  const nodes: ReactNode[] = [];
  let cursor = 0;
  let key = 0;
  while (cursor < text.length) {
    const match = findInlineToken(text, cursor);
    if (!match) {
      nodes.push(<span key={key++}>{text.slice(cursor)}</span>);
      break;
    }
    if (match.index > cursor) nodes.push(<span key={key++}>{text.slice(cursor, match.index)}</span>);
    if (match.kind === 'emoji') {
      nodes.push(<span key={key++} style={{ display: 'inline-flex', alignItems: 'center', justifyContent: 'center', width: 16, height: 16, borderRadius: 4, background: match.value === '✓' ? t.ok : t.danger, color: '#fff', verticalAlign: -3, margin: '0 1px' }}><Icon name={match.value === '✓' ? 'check' : 'x'} size={11} stroke={3} color="#fff" /></span>);
    } else if (match.kind === 'code') {
      nodes.push(<code key={key++} className="mono" style={{ padding: '2px 5px', borderRadius: 5, background: t.surfaceHover, border: `0.5px solid ${t.border}`, color: t.text, fontSize: '.9em' }}>{match.value}</code>);
    } else if (match.kind === 'link') {
      const href = safeHref(match.href);
      nodes.push(href ? <a key={key++} href={href} target="_blank" rel="noreferrer" style={{ color: t.accent, textDecoration: 'underline', textUnderlineOffset: 2 }}>{renderInline(match.value, t)}</a> : <span key={key++}>{match.raw}</span>);
    } else {
      const style = match.kind === 'strong'
        ? { fontWeight: 650 }
        : match.kind === 'strike'
          ? { textDecoration: 'line-through', color: t.text3 }
          : { fontStyle: 'italic' };
      nodes.push(<span key={key++} style={style}>{renderInline(match.value, t)}</span>);
    }
    cursor = match.end;
  }
  return nodes;
}

type InlineToken = { index: number; end: number; kind: 'emoji' | 'code' | 'link' | 'strong' | 'strike' | 'emphasis'; value: string; raw: string; href: string };

function findInlineToken(text: string, from: number): InlineToken | undefined {
  const candidates: InlineToken[] = [];
  const add = (kind: InlineToken['kind'], regex: RegExp, valueIndex = 1, hrefIndex = -1) => {
    regex.lastIndex = from;
    const match = regex.exec(text);
    if (!match || match.index < from) return;
    candidates.push({ index: match.index, end: match.index + match[0].length, kind, value: match[valueIndex] ?? '', raw: match[0], href: hrefIndex >= 0 ? match[hrefIndex] ?? '' : '' });
  };
  add('emoji', /[✓✗]/g, 0);
  add('code', /`([^`\n]+)`/g);
  add('link', /\[([^\]\n]+)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g, 1, 2);
  add('strong', /\*\*([^*\n]+)\*\*|__([^_\n]+)__/g, 1);
  add('strike', /~~([^~\n]+)~~/g);
  add('emphasis', /\*([^*\n]+)\*|_([^_\n]+)_/g);
  return candidates.sort((left, right) => left.index - right.index || tokenPriority(left.kind) - tokenPriority(right.kind))[0];
}

function tokenPriority(kind: InlineToken['kind']): number {
  return kind === 'strong' ? 0 : kind === 'code' ? 1 : kind === 'link' ? 2 : kind === 'strike' ? 3 : kind === 'emphasis' ? 4 : 5;
}

function safeHref(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'https:' || url.protocol === 'http:' || url.protocol === 'mailto:' ? raw : undefined;
  } catch {
    return undefined;
  }
}
