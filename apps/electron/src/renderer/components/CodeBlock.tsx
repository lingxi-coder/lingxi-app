import { memo, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import hljs from 'highlight.js/lib/common';

import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export const EXPLICIT_HIGHLIGHT_MAX_CHARS = 100_000;
export const AUTO_HIGHLIGHT_MAX_CHARS = 20_000;

const LANGUAGE_ALIASES: Readonly<Record<string, string>> = {
  'c++': 'cpp',
  'c#': 'csharp',
  cjs: 'javascript',
  cs: 'csharp',
  html: 'xml',
  htm: 'xml',
  js: 'javascript',
  jsx: 'javascript',
  md: 'markdown',
  mjs: 'javascript',
  objc: 'objectivec',
  py: 'python',
  rb: 'ruby',
  rs: 'rust',
  sh: 'bash',
  svg: 'xml',
  text: 'plaintext',
  ts: 'typescript',
  tsx: 'typescript',
  txt: 'plaintext',
  yml: 'yaml',
  zsh: 'bash',
};

const LANGUAGE_LABELS: Readonly<Record<string, string>> = {
  bash: 'Shell',
  cpp: 'C++',
  csharp: 'C#',
  graphql: 'GraphQL',
  javascript: 'JavaScript',
  json: 'JSON',
  objectivec: 'Objective-C',
  'php-template': 'PHP',
  plaintext: 'Text',
  'python-repl': 'Python REPL',
  typescript: 'TypeScript',
  wasm: 'WebAssembly',
  xml: 'HTML / XML',
  yaml: 'YAML',
};

type HighlightedCode = {
  html?: string;
  language?: string;
  label: string;
};

function normalizedLanguage(language?: string): string | undefined {
  const normalized = language?.trim().toLowerCase().replace(/^language-/, '');
  if (!normalized) return undefined;
  return LANGUAGE_ALIASES[normalized] ?? normalized;
}

function languageLabel(language?: string): string {
  if (!language) return 'Code';
  return LANGUAGE_LABELS[language] ?? language.toUpperCase();
}

export function highlightCodeForDisplay(code: string, language: string | undefined, closed: boolean): HighlightedCode {
  const normalized = normalizedLanguage(language);
  const fallback = { language: normalized ?? language, label: languageLabel(normalized ?? language) };
  if (!closed) return fallback;

  try {
    if (normalized) {
      if (code.length > EXPLICIT_HIGHLIGHT_MAX_CHARS || !hljs.getLanguage(normalized)) return fallback;
      const highlighted = hljs.highlight(code, { language: normalized, ignoreIllegals: true });
      return { html: highlighted.value, language: normalized, label: languageLabel(normalized) };
    }
    if (code.length > AUTO_HIGHLIGHT_MAX_CHARS) return fallback;
    const highlighted = hljs.highlightAuto(code);
    return {
      html: highlighted.value,
      language: highlighted.language,
      label: languageLabel(highlighted.language),
    };
  } catch {
    return fallback;
  }
}

export interface CodeBlockProps {
  id?: string;
  codeId?: string;
  code: string;
  language?: string;
  closed?: boolean;
  copyText?: string;
  variant?: 'message' | 'tool';
}

type CopyState = 'idle' | 'copied' | 'error';

export const CodeBlock = memo(function CodeBlock({
  id,
  codeId,
  code,
  language,
  closed = true,
  copyText = code,
  variant = 'message',
}: CodeBlockProps) {
  const t = useT();
  const highlighted = useMemo(
    () => highlightCodeForDisplay(code, language, closed),
    [closed, code, language],
  );
  const [copyState, setCopyState] = useState<CopyState>('idle');
  const resetTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => () => {
    if (resetTimer.current !== undefined) clearTimeout(resetTimer.current);
  }, []);

  const setTemporaryCopyState = (state: Exclude<CopyState, 'idle'>) => {
    if (resetTimer.current !== undefined) clearTimeout(resetTimer.current);
    setCopyState(state);
    resetTimer.current = setTimeout(() => setCopyState('idle'), state === 'copied' ? 1_500 : 2_000);
  };

  const copy = async () => {
    try {
      if (window.lingxi?.copyText) await window.lingxi.copyText(copyText);
      else if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(copyText);
      else throw new Error('Clipboard unavailable');
      setTemporaryCopyState('copied');
    } catch {
      setTemporaryCopyState('error');
    }
  };

  const variables = {
    '--code-bg': t.windowBg,
    '--code-toolbar-bg': t.surface,
    '--code-border': t.border,
    '--code-text': t.syntax.plain,
    '--code-muted': t.text3,
    '--code-hover': t.surfaceHover,
    '--code-focus': t.accent,
    '--code-success': t.ok,
    '--code-error': t.danger,
    '--syntax-keyword': t.syntax.keyword,
    '--syntax-type': t.syntax.type_name,
    '--syntax-function': t.syntax.function,
    '--syntax-string': t.syntax.string_lit,
    '--syntax-number': t.syntax.number,
    '--syntax-comment': t.syntax.comment,
    '--syntax-punctuation': t.syntax.punctuation,
    '--syntax-operator': t.syntax.operator,
    '--syntax-variable': t.syntax.variable,
    '--syntax-constant': t.syntax.constant,
    '--syntax-attribute': t.syntax.attribute,
  } as CSSProperties;
  const copyLabel = copyState === 'copied' ? 'Copied' : copyState === 'error' ? 'Copy failed' : 'Copy';

  return (
    <div id={id} className={`code-card code-card-${variant}`} style={variables}>
      <div className="code-card-toolbar">
        <span className="code-card-language mono">{highlighted.label}</span>
        <button
          type="button"
          className="code-card-copy"
          aria-label="Copy code"
          data-state={copyState}
          onClick={() => { void copy(); }}
        >
          <Icon
            name={copyState === 'copied' ? 'check' : copyState === 'error' ? 'x' : 'copy'}
            size={13}
            stroke={copyState === 'idle' ? 1.7 : 2.2}
          />
          <span aria-live="polite">{copyLabel}</span>
        </button>
      </div>
      <div className="code-card-scroll">
        <pre className="mono-code">
          {highlighted.html !== undefined ? (
            <code
              className="hljs"
              id={codeId}
              data-language={highlighted.language}
              dangerouslySetInnerHTML={{ __html: highlighted.html }}
            />
          ) : (
            <code id={codeId} data-language={highlighted.language}>{code}</code>
          )}
        </pre>
      </div>
    </div>
  );
});
