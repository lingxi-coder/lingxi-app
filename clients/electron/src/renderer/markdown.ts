import { defaultSchema, type Schema } from 'hast-util-sanitize';
import type { Extension } from 'mdast-util-from-markdown';
import type { Paragraph, PhrasingContent, Root, RootContent } from 'mdast';
import type { Element, Root as HtmlRoot } from 'hast';
import type { Plugin } from 'unified';
import type {} from 'remark-parse';
import { parseContextMentionHref } from './bridge/composerMentions';

export type ParsedMarkdown = {
  source: string;
};

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

/** Normalize line endings and standalone JSON without reparsing Markdown. */
export function parseMarkdown(source: string): ParsedMarkdown {
  const standaloneJson = standaloneJsonForDisplay(source);
  return { source: standaloneJson === undefined
    ? normalizeMarkdown(source)
    : '```json\n' + standaloneJson + '\n```' };
}

export function normalizeMarkdown(source: string): string {
  return source.replace(/\r\n?/g, '\n');
}

// Enter hooks augment the default compiler; its exit hooks still own code text.
const fenceMetadata: Extension = {
  enter: {
    codeFencedFence() {
      for (let index = this.stack.length - 1; index >= 0; index -= 1) {
        const node = this.stack[index];
        if (node.type !== 'code') continue;
        node.data ??= {};
        node.data.hProperties ??= {};
        node.data.hProperties.dataFenceClosed = this.data.flowCodeInside ? 'true' : 'false';
        break;
      }
    },
  },
};

/** Split only actual top-level text delimiters before formatted list items. */
function expandLooseParagraph(node: Paragraph, source: string): RootContent[] {
  const sections: PhrasingContent[][] = [[]];
  const formattedTypes = new Set(['strong', 'inlineCode', 'delete', 'link']);
  for (let index = 0; index < node.children.length; index += 1) {
    const child = node.children[index];
    const next = node.children[index + 1];
    const marker = child.type === 'text' ? /[ \t]+[-+*][ \t]+$/.exec(child.value) : null;
    const raw = source.slice(child.position?.start.offset, child.position?.end.offset);
    if (child.type === 'text' && marker && next && formattedTypes.has(next.type)
      && /[ \t]+[-+*][ \t]+$/.test(raw)) {
      const value = child.value.slice(0, marker.index);
      if (value) sections[sections.length - 1].push({ ...child, value });
      sections.push([]);
    } else {
      sections[sections.length - 1].push(child);
    }
  }
  if (sections.length < 3) return [node];
  const result: RootContent[] = [];
  if (sections[0].length) result.push({ type: 'paragraph', children: sections[0] });
  result.push({
    type: 'list', ordered: false, spread: false,
    children: sections.slice(1).map((children) => ({
      type: 'listItem', spread: false, children: [{ type: 'paragraph', children }],
    })),
  });
  return result;
}

/** Extend the parser already owned by ReactMarkdown, without a second parse. */
export const remarkDesktopMarkdown: Plugin<[], Root> = function () {
  const data = this.data();
  const extensions = data.fromMarkdownExtensions ?? (data.fromMarkdownExtensions = []);
  extensions.push(fenceMetadata);
  return (tree, file) => {
    const source = String(file);
    tree.children = tree.children.flatMap((node) => node.type === 'paragraph'
      ? expandLooseParagraph(node, source) : [node]);
  };
};

/** Run after sanitize: retain its safe IDs and scope references to this message. */
export const rehypeMarkdownAnchors: Plugin<[string], HtmlRoot> = function (namespace) {
  return (tree) => {
    const elements: Element[] = [];
    const collect = (parent: HtmlRoot | Element): void => {
      for (const child of parent.children) {
        if (child.type !== 'element') continue;
        elements.push(child);
        collect(child);
      }
    };
    collect(tree);

    const fragmentIds = new Map<string, string>();
    const sanitizedIds = new Map<string, string>();
    const prefix = defaultSchema.clobberPrefix ?? '';
    for (const element of elements) {
      const id = element.properties.id;
      if (typeof id !== 'string') continue;
      const scopedId = namespace + id;
      const originalId = id.startsWith(prefix) ? id.slice(prefix.length) : id;
      // Keep the first target for duplicate authored IDs, like native navigation.
      if (!fragmentIds.has(originalId)) fragmentIds.set(originalId, scopedId);
      if (!sanitizedIds.has(id)) sanitizedIds.set(id, scopedId);
      element.properties.id = scopedId;
    }

    for (const element of elements) {
      const href = element.properties.href;
      if (typeof href === 'string' && href.startsWith('#')) {
        const fragment = href.slice(1);
        const literalTarget = fragmentIds.get(fragment);
        let decodedTarget: string | undefined;
        try {
          decodedTarget = fragmentIds.get(decodeURIComponent(fragment));
        } catch { /* A literal percent sign can still name an authored ID. */ }
        // Generated footnote IDs contain percent escapes literally; normal HTML
        // fragments follow URL decoding rules, including when both IDs exist.
        const footnote = element.properties.dataFootnoteRef !== undefined
          || element.properties.dataFootnoteBackref !== undefined;
        const target = footnote ? literalTarget ?? decodedTarget : decodedTarget ?? literalTarget;
        if (target) element.properties.href = '#' + encodeURIComponent(target);
      }
      for (const property of ['ariaDescribedBy', 'ariaLabelledBy']) {
        const value = element.properties[property];
        if (value === undefined || value === null) continue;
        const ids = Array.isArray(value) ? value.map(String) : String(value).split(/\s+/);
        element.properties[property] = ids.map((id) => sanitizedIds.get(id) ?? id);
      }
    }
  };
};

/** Keep only trusted URL schemes for rendered links. */
export function sanitizeMarkdownHref(raw: string): string | undefined {
  if (parseContextMentionHref(raw)) return raw;
  if (/^(#|\/|\.\/|\.{2}\/|\?)/.test(raw)) return raw;
  try {
    const url = new URL(raw);
    if (url.protocol === 'https:' || url.protocol === 'http:' || url.protocol === 'mailto:') return raw;
  } catch {
    return undefined;
  }
  return undefined;
}

/**
 * Shared rehype-sanitize schema for markdown and optional trusted HTML.
 * Trusted HTML is still sanitized to a strict allow-list.
 */
export function sanitizeMarkdownHtml(_trusted = false): Schema {
  const tagNames = new Set(defaultSchema.tagNames);
  const tableLikeTags = ['table', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td', 'caption', 'colgroup', 'col'];
  const taskTags = ['input'];
  for (const tagName of [...tableLikeTags, ...taskTags]) tagNames.add(tagName);

  const dedupe = (current: unknown, value: string[]) => {
    const normalized = Array.isArray(current)
      ? current
        .flatMap((item): string[] => {
          if (typeof item === 'string') return [item];
          if (Array.isArray(item) && typeof item[0] === 'string') return [item[0] as string];
          return [];
        })
        .filter((item): item is string => item.length > 0)
      : [];
    return Array.from(new Set([...normalized, ...value]));
  };

  return {
    ...defaultSchema,
    tagNames: [...tagNames],
    attributes: {
      ...defaultSchema.attributes,
      a: dedupe(defaultSchema.attributes?.a, ['target', 'rel', 'title']),
      code: dedupe(defaultSchema.attributes?.code, ['className', 'dataFenceClosed']),
      pre: dedupe(defaultSchema.attributes?.pre, ['className']),
      table: dedupe(defaultSchema.attributes?.table, ['align']),
      th: dedupe(defaultSchema.attributes?.th, ['align']),
      td: dedupe(defaultSchema.attributes?.td, ['align', 'rowspan', 'colspan']),
      input: ['type', 'checked', 'disabled'],
      span: dedupe(defaultSchema.attributes?.span, ['className']),
      del: dedupe(defaultSchema.attributes?.del, ['className']),
    },
    protocols: {
      ...defaultSchema.protocols,
      href: ['https', 'http', 'mailto', 'lingxi-mention', ''],
      src: ['https', 'http'],
    },
  };
}
