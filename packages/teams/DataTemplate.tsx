import { createElement, useState, type CSSProperties, type FormEvent, type ReactElement, type ReactNode } from 'react';
import type { TeamsApi } from './index.js';
import type { DataDescription, DataScope, DataTemplateNode, DataTemplateSpec, DataTemplateTag, DataTemplateView } from './lingxi-data.js';

const tags = new Set<DataTemplateTag>(['section', 'div', 'h3', 'p', 'span', 'strong', 'small', 'ul', 'li']);
const length = /^(?:0|[0-9]{1,2}(?:px|rem))$/u;
const color = /^#[a-f0-9]{6}$/iu;
const properties: Record<string, [string, RegExp]> = {
  display: ['display', /^(grid|flex|block)$/u], 'flex-direction': ['flexDirection', /^(row|column)$/u],
  gap: ['gap', length], padding: ['padding', length], 'border-radius': ['borderRadius', length],
  'font-size': ['fontSize', length], 'font-weight': ['fontWeight', /^[4-7]00$/u],
  color: ['color', color], 'background-color': ['backgroundColor', color],
};

function rules(input: string): Record<string, Record<string, string>> {
  const values: Record<string, Record<string, string>> = Object.create(null);
  const rule = /^\.([A-Za-z][A-Za-z0-9_-]{0,63})\s*\{([^{}]*)\}\s*/u;
  let rest = input.trim();
  while (rest) {
    const match = rule.exec(rest);
    if (!match) throw new Error('Use class selectors with supported CSS properties.');
    values[match[1]!] = { ...values[match[1]!], ...declarations(match[2]!) };
    rest = rest.slice(match[0].length);
  }
  return values;
}

function declarations(input: string): Record<string, string> {
  const values: Record<string, string> = Object.create(null);
  for (const item of input.split(';')) {
    if (!item.trim()) continue;
    const pair = item.split(':');
    const key = pair[0]?.trim() ?? '';
    const value = pair[1]?.trim() ?? '';
    const field = Object.prototype.hasOwnProperty.call(properties, key) ? properties[key] : undefined;
    if (pair.length !== 2 || !field?.[1].test(value)) throw new Error('Unsupported CSS value.');
    values[field[0]] = value;
  }
  return values;
}

export function compileDataTemplate(html: string, css: string, fields: string[]): DataTemplateSpec {
  if (!html.trim() || html.length > 16000 || css.length > 4000) throw new Error('Template is empty or too large.');
  const template = document.createElement('template');
  template.innerHTML = html;
  const selectors = rules(css);
  const allowed = new Set(fields);
  let count = 0;
  function compile(nodes: NodeListOf<ChildNode>, depth: number): DataTemplateNode[] {
    if (depth > 10) throw new Error('Template nesting is too deep.');
    const out: DataTemplateNode[] = [];
    for (const node of nodes) {
      if (++count > 128) throw new Error('Template has too many nodes.');
      if (node.nodeType === Node.TEXT_NODE) {
        const content = node.textContent ?? '';
        if (!content.trim()) continue;
        const tokens = content.split(/(\{\{\s*[A-Za-z0-9_-]{1,64}\s*\}\})/u);
        for (const token of tokens) {
          if (!token) continue;
          if (token.startsWith('{{') && token.endsWith('}}')) {
            const field = token.slice(2, -2).trim();
            if (!allowed.has(field)) throw new Error('Template refers to an unknown field.');
            out.push({ kind: 'field', field });
          } else {
            if (token.length > 1024 || token.includes('{{') || token.includes('}}')) throw new Error('Invalid field binding.');
            out.push({ kind: 'text', text: token });
          }
        }
      } else if (node instanceof HTMLElement) {
        const tag = node.localName as DataTemplateTag;
        if (!tags.has(tag) || [...node.attributes].some(attribute => !['class', 'style'].includes(attribute.name))) {
          throw new Error('Template contains an unsupported element or attribute.');
        }
        const style = Object.assign({}, ...[...node.classList].map(name => selectors[name] ?? {}), declarations(node.getAttribute('style') ?? ''));
        out.push({ kind: 'element', tag, style, children: compile(node.childNodes, depth + 1) });
      } else if (node.nodeType !== Node.COMMENT_NODE) {
        throw new Error('Template contains unsupported content.');
      }
    }
    return out;
  }
  const nodes = compile(template.content.childNodes, 0);
  if (!nodes.length) throw new Error('Template is empty.');
  return { version: 1, nodes };
}

export function DataTemplate({ spec, value }: { spec: DataTemplateSpec; value: Record<string, unknown> }): ReactElement | null {
  if (spec.version !== 1) return null;
  function render(node: DataTemplateNode, key: number): ReactNode {
    if (node.kind === 'text') return node.text;
    if (node.kind === 'field') {
      const field = Object.prototype.hasOwnProperty.call(value, node.field) ? value[node.field] : undefined;
      return typeof field === 'string' ? field : JSON.stringify(field) ?? '—';
    }
    if (!tags.has(node.tag)) return null;
    const safe = Object.fromEntries(Object.entries(node.style ?? {}).filter(([key, value]) =>
      Object.values(properties).some(([name, pattern]) => key === name && pattern.test(value))));
    return createElement(node.tag, { key, style: safe as CSSProperties }, node.children.map(render));
  }
  return <div>{spec.nodes.map(render)}</div>;
}

export function DataTemplateSettings({ api, scope, dataset, description, views, locale, saved }: {
  api: TeamsApi; scope: DataScope; dataset: string; description: DataDescription; views: DataTemplateView[];
  locale: string; saved(): void;
}): ReactElement | null {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const zh = locale.toLowerCase().startsWith('zh');
  if (!api.data) return null;
  const data = api.data;

  function save(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const view = String(form.get('view'));
    setError(false);
    let definition: DataTemplateSpec;
    try {
      definition = compileDataTemplate(String(form.get('html')), String(form.get('css')), Object.keys(description.schema.properties ?? {}));
    } catch { setError(true); return; }
    setBusy(true);
    void data({ scope, command: { action: 'setTemplateView', dataset, view, schemaVersion: description.schemaVersion,
      expectedVersion: views.find(entry => entry.view === view)?.version ?? 0, definition } })
      .then(saved, () => setError(true)).finally(() => setBusy(false));
  }
  return <details className="tw-data-workspace">
    <summary>{zh ? '自定义 HTML/CSS 视图' : 'Custom HTML/CSS view'}</summary>
    <form method="post" onSubmit={save}>
      <label>{zh ? '视图 ID' : 'View ID'}<input name="view" required pattern="[A-Za-z0-9_\-]{1,64}" /></label>
      <label>HTML<textarea name="html" required maxLength={16000} rows={5} spellCheck={false}
        placeholder="<section><h3>{{title}}</h3></section>" /></label>
      <label>CSS<textarea name="css" maxLength={4000} rows={3} spellCheck={false} /></label>
      {error && <p role="alert">{zh ? '视图未保存。请检查布局、样式或字段绑定。' : 'The view could not be saved. Check its layout, styles and field bindings.'}</p>}
      <button type="submit" disabled={busy}>{zh ? '保存视图' : 'Save view'}</button>
    </form>
  </details>;
}
