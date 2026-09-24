import { createContext, memo, useContext, useId, useMemo, type CSSProperties, type ReactNode } from 'react';
import type { Element } from 'hast';
import ReactMarkdown, { defaultUrlTransform } from 'react-markdown';
import { openContextMention, parseContextMentionHref } from '../bridge/composerMentions';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import type { Components } from 'react-markdown';
import type { PluggableList } from 'unified';

import { useT } from '../theme/ThemeContext';
import { CodeBlock } from './CodeBlock';
import {
  parseMarkdown,
  remarkDesktopMarkdown,
  rehypeMarkdownAnchors,
  sanitizeMarkdownHref,
  sanitizeMarkdownHtml,
} from '../markdown';

interface MarkdownContentProps {
  text: string;
  trustedHtml?: boolean;
  variant?: 'plan';
}

type MarkdownComponentProps = {
  id?: string;
  children?: ReactNode;
  className?: string;
  href?: string;
  src?: string;
  alt?: string;
  checked?: boolean;
  disabled?: boolean;
  target?: string;
  rel?: string;
  node?: Element;
};

const remarkPlugins: PluggableList = [remarkGfm, remarkDesktopMarkdown];
const safePlugins: PluggableList = [[rehypeSanitize, sanitizeMarkdownHtml(false)]];
const trustedPlugins: PluggableList = [rehypeRaw, [rehypeSanitize, sanitizeMarkdownHtml(true)]];
const InsideMarkdownLink = createContext(false);

// Keep renderer identities stable so streaming updates preserve mounted DOM and state.
const components = {
    p: ({ children, node: _node, ...props }: MarkdownComponentProps) => (
      <p
        {...props}
        style={{ margin: 0, whiteSpace: 'pre-wrap', textWrap: 'pretty' }}
        className="markdown-paragraph"
      >
        <span>{children}</span>
      </p>
    ),
    code: ({ className, children, node: _node, ...props }: MarkdownComponentProps) => (
      <code {...props} className={`markdown-inline-code ${className ?? ''}`.trim()}>{children}</code>
    ),
    pre: function MarkdownPre({ children, node, className, ...props }: MarkdownComponentProps) {
      const codeNode = node?.children[0];
      if (node?.children.length !== 1 || codeNode?.type !== 'element'
        || codeNode.tagName !== 'code' || !codeNode.children.every((child) => child.type === 'text')) {
        return <pre {...props} className={`markdown-pre ${className ?? ''}`.trim()}>{children}</pre>;
      }

      const classes = codeNode.properties.className;
      const rawLanguage = (Array.isArray(classes) ? classes.join(' ') : String(classes ?? ''))
        .match(/(?:^|\s)language-([^\s]+)/)?.[1];
      const source = codeNode.children.map((child) => child.type === 'text' ? child.value : '').join('');
      const closed = codeNode.properties.dataFenceClosed;
      const unclosed = closed === false || closed === 'false';

      return (
        <CodeBlock
          id={props.id}
          codeId={typeof codeNode.properties.id === 'string' ? codeNode.properties.id : undefined}
          code={source.replace(/\n$/, '')}
          language={rawLanguage}
          closed={!unclosed}
          copyText={source}
        />
      );
    },
    blockquote: ({ children, node: _node, ...props }: MarkdownComponentProps) => (
      <blockquote {...props} className="markdown-blockquote">{children}</blockquote>
    ),
    h1: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h1 {...props} style={{ margin: '3px 0 0', color: 'inherit', fontSize: '2.15em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 650 }} className="markdown-heading">{children}</h1>,
    h2: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h2 {...props} style={{ margin: '8px 0 0', color: 'inherit', fontSize: '1.6em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 620 }} className="markdown-heading">{children}</h2>,
    h3: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h3 {...props} style={{ margin: '8px 0 0', color: 'inherit', fontSize: '1.4em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 600 }} className="markdown-heading">{children}</h3>,
    h4: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h4 {...props} style={{ margin: '8px 0 0', color: 'inherit', fontSize: '1.22em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 590 }} className="markdown-heading">{children}</h4>,
    h5: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h5 {...props} style={{ margin: '8px 0 0', color: 'inherit', fontSize: '1.1em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 570 }} className="markdown-heading">{children}</h5>,
    h6: ({ children, node: _node, ...props }: MarkdownComponentProps) => <h6 {...props} style={{ margin: '8px 0 0', color: 'inherit', fontSize: '1.0em', lineHeight: 1.35, letterSpacing: '-.012em', textWrap: 'balance', fontWeight: 550 }} className="markdown-heading">{children}</h6>,
    ul: ({ children, node: _node, ...props }: MarkdownComponentProps) => <ul {...props} className="markdown-ul">{children}</ul>,
    ol: ({ children, node: _node, ...props }: MarkdownComponentProps) => <ol {...props} className="markdown-ol">{children}</ol>,
    li: ({ children, node: _node, ...props }: MarkdownComponentProps) => <li {...props} className="markdown-li">{children}</li>,
    table: ({ children, node: _node, ...props }: MarkdownComponentProps) => (
      <div className="markdown-table-wrap">
        <table {...props} className="markdown-table">{children}</table>
      </div>
    ),
    thead: ({ children, node: _node, ...props }: MarkdownComponentProps) => <thead {...props} className="markdown-thead">{children}</thead>,
    tbody: ({ children, node: _node, ...props }: MarkdownComponentProps) => <tbody {...props} className="markdown-tbody">{children}</tbody>,
    tr: ({ children, node: _node, ...props }: MarkdownComponentProps) => <tr {...props} className="markdown-tr">{children}</tr>,
    th: ({ children, node: _node, ...props }: MarkdownComponentProps) => <th {...props} className="markdown-th">{children}</th>,
    td: ({ children, node: _node, ...props }: MarkdownComponentProps) => <td {...props} className="markdown-td">{children}</td>,
    em: ({ children, node: _node, ...props }: MarkdownComponentProps) => <span {...props} style={{ fontStyle: 'italic' }}>{children}</span>,
    del: ({ children, node: _node, ...props }: MarkdownComponentProps) => <span {...props} className="markdown-del">{children}</span>,
    strong: ({ children, node: _node, ...props }: MarkdownComponentProps) => <span {...props} style={{ fontWeight: 650 }}>{children}</span>,
    a: ({ href, children, node: _node, ...props }: MarkdownComponentProps) => {
      const safeHref = href === undefined ? undefined : sanitizeMarkdownHref(href);
      if (safeHref === undefined) {
        return <span id={props.id} className="markdown-link-disabled">{children}</span>;
      }
      const fragment = safeHref.startsWith('#');
      const mention = parseContextMentionHref(safeHref);
      if (mention) return <a {...props} href={safeHref} title={mention.target} onClick={(event) => { event.preventDefault(); openContextMention(mention); }}><InsideMarkdownLink.Provider value={true}>{children}</InsideMarkdownLink.Provider></a>;
      return <a {...props} href={safeHref} target={fragment ? undefined : '_blank'} rel={fragment ? undefined : 'noreferrer'}><InsideMarkdownLink.Provider value={true}>{children}</InsideMarkdownLink.Provider></a>;
    },
    img: function MarkdownImage({ src, alt, id }: MarkdownComponentProps) {
      const insideLink = useContext(InsideMarkdownLink);
      const label = alt || src || 'Image unavailable';
      if (insideLink) return <span id={id}>Image: {label}</span>;
      try {
        const url = new URL(src?.startsWith('//') ? 'https:' + src : src ?? '');
        if (url.protocol === 'https:' && !url.username && !url.password) {
          return <a id={id} href={url.href} target="_blank" rel="noreferrer" className="markdown-image-link">Image: {label}</a>;
        }
      } catch { /* Unresolvable image sources remain visible as text. */ }
      return <span id={id} className="markdown-image-unavailable">Image unavailable: {label}</span>;
    },
    input: ({ checked, disabled, node: _node, ...props }: MarkdownComponentProps) => (
      <input
        {...props}
        type="checkbox"
        disabled={Boolean(disabled)}
        checked={Boolean(checked)}
        readOnly
      />
    ),
    hr: ({ id }: MarkdownComponentProps) => <hr id={id} className="markdown-hr" />,
} satisfies Components;

const planComponents: Components = {
  ...components,
  h1: ({ children }) => <h1 style={{ margin: '0 0 12px', fontSize: 18, lineHeight: 1.45, fontWeight: 600 }}>{children}</h1>,
  h2: ({ children }) => <h2 style={{ margin: '20px 0 0', fontSize: 16, lineHeight: 1.45, fontWeight: 600 }}>{children}</h2>,
  h3: ({ children }) => <h3 style={{ margin: '16px 0 0', fontSize: 15, lineHeight: 1.45, fontWeight: 600 }}>{children}</h3>,
};

export const MarkdownContent = memo(function MarkdownContent({ text, trustedHtml = false, variant }: MarkdownContentProps) {
  const t = useT();
  const instanceId = useId();
  const rehypePlugins = useMemo<PluggableList>(() => [
    ...(trustedHtml ? trustedPlugins : safePlugins),
    [rehypeMarkdownAnchors, `markdown-${instanceId}-`],
  ], [instanceId, trustedHtml]);
  const parsed = useMemo(() => parseMarkdown(text), [text]);
  const content = useMemo(() => (
    <ReactMarkdown
      children={parsed.source}
      remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins}
      components={variant === 'plan' ? planComponents : components}
      urlTransform={(url) => parseContextMentionHref(url) ? url : defaultUrlTransform(url)}
    />
  ), [parsed.source, rehypePlugins, variant]);
  const variables = {
    display: 'grid', gap: 10, minWidth: 0,
    '--text': t.text,
    '--text3': t.text3,
    '--surface': t.surface,
    '--surface-hover': t.surfaceHover,
    '--accent-border': t.accentBorder,
    '--code-bg': t.windowBg,
    '--code-border': t.border,
    '--syntax-string': t.syntax.string_lit,
  } as CSSProperties;

  return (
    <div className="markdown-content" style={variables}>
      {content}
    </div>
  );
});
