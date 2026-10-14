import type { FusionStrategyRefDto, FusionStrategySelectionDto } from '@lingxi/bridge-client';

interface Token { value: string; start: number; end: number }
export interface FusionInvocation {
  strategy?: FusionStrategyRefDto;
  models?: string[];
  preset?: 'quality' | 'fast';
  maxPanel?: number;
  selectionSpans: Token[];
  prefixEnd: number;
}

/** Only tokenise the flag prefix; the prompt remains byte-for-byte untouched. */
function nextToken(raw: string, start: number): Token | undefined {
  while (/\s/.test(raw[start] ?? '') && start < raw.length) start++;
  if (start >= raw.length) return undefined;
  let cursor = start;
  let quote = '';
  let value = '';
  while (cursor < raw.length) {
    const char = raw[cursor]!;
    if (!quote && /\s/.test(char)) break;
    if (char === '\\' && quote !== "'" && cursor + 1 < raw.length) {
      value += raw[++cursor];
    } else if (char === quote) quote = '';
    else if (!quote && (char === '"' || char === "'")) quote = char;
    else value += char;
    cursor++;
  }
  if (quote) throw new Error('Fusion flag has an unclosed quote.');
  return { value, start, end: cursor };
}

const VALUE_FLAGS = new Set(['--strategy', '--strategy-revision', '--models', '--dimensions', '--max-panel', '--verify']);
const SWITCH_FLAGS = new Set(['--quality', '--fast', '--same-provider', '--cross-provider', '--partial-ok', '--no-partial', '--verify-claims', '--implement']);

export function fusionStrategyKey(reference: FusionStrategyRefDto): string {
  return `${reference.scope}:${reference.id}`;
}

export function parseFusionStrategyRef(value: string): FusionStrategyRefDto {
  const match = /^(user|project|local):([^\s:]+)$/.exec(value);
  if (!match) throw new Error('Choose a Fusion strategy using scope:id.');
  return { scope: match[1] as FusionStrategyRefDto['scope'], id: match[2]! };
}

export function fusionInvocation(raw: string): FusionInvocation | null {
  const head = /^\s*\/fusion(?:\s|$)/i.exec(raw);
  if (!head) return null;
  const args = raw.slice(head[0].length).trim();
  if (args.toLowerCase() === 'setup' || args.toLowerCase() === 'clean' || !args) return null;
  let cursor = head[0].length;
  let strategy: FusionStrategyRefDto | undefined;
  let models: string[] | undefined;
  let preset: FusionInvocation['preset'];
  let maxPanel: number | undefined;
  const selectionSpans: Token[] = [];
  for (;;) {
    while (cursor < raw.length && /\s/.test(raw[cursor]!)) cursor++;
    const prefix = /^--[a-z-]+(?==|\s|$)/.exec(raw.slice(cursor))?.[0];
    // An apostrophe in natural language is not a shell quote. Never scan the
    // first prompt token: only the recognized option prefix uses quoting.
    if (!prefix || (!VALUE_FLAGS.has(prefix) && !SWITCH_FLAGS.has(prefix) && prefix !== '--retry-publication')) break;
    const token = nextToken(raw, cursor);
    if (!token) break;
    const equals = token.value.indexOf('=');
    const flag = equals < 0 ? token.value : token.value.slice(0, equals);
    if (flag === '--retry-publication') return null;
    if (!VALUE_FLAGS.has(flag) && !SWITCH_FLAGS.has(flag)) break;
    cursor = token.end;
    if (!VALUE_FLAGS.has(flag)) {
      if (flag === '--quality' || flag === '--fast') {
        if (preset) throw new Error('--quality and --fast may be specified only once.');
        preset = flag === '--fast' ? 'fast' : 'quality';
      }
      continue;
    }
    const value = equals < 0 ? nextToken(raw, cursor) : undefined;
    if (equals < 0 && !value) throw new Error(`${flag} requires a value.`);
    if (value) cursor = value.end;
    const argument = value?.value ?? token.value.slice(equals + 1);
    if (flag === '--models') {
      if (models) throw new Error('--models specified more than once.');
      models = argument.split(',').map((item) => item.trim());
      if (models.length < 2) throw new Error('explicit --models must contain at least 2 entries');
      if (models.some((item) => !item || item.indexOf(':') === 0 || (item.includes(':') && item.indexOf(':') === item.length - 1))) throw new Error('--models must contain valid model references.');
    }
    if (flag === '--max-panel') {
      if (maxPanel !== undefined) throw new Error('--max-panel specified more than once.');
      if (!/^\+?\d+$/.test(argument) || Number(argument) < 2 || Number(argument) > 8) throw new Error('--max-panel must be 2..=8');
      maxPanel = Number(argument);
    }
    if (flag === '--strategy') {
      if (strategy) throw new Error('--strategy specified more than once.');
      strategy = parseFusionStrategyRef(argument);
    }
    if (flag === '--strategy' || flag === '--strategy-revision') selectionSpans.push({ ...token, end: cursor });
  }
  return { strategy, models, preset, maxPanel, selectionSpans, prefixEnd: head[0].trimEnd().length };
}

/** The revision includes scope/id/config; changing the default while loading credentials fails closed. */
export function pinFusionStrategy(raw: string, selection: Pick<FusionStrategySelectionDto, 'reference' | 'revision'>): string {
  const invocation = fusionInvocation(raw);
  if (!invocation) return raw;
  const key = fusionStrategyKey(selection.reference);
  parseFusionStrategyRef(key);
  if (!/^[a-f0-9]+$/i.test(selection.revision)) throw new Error('Invalid Fusion strategy revision.');
  if (invocation.selectionSpans.some((span) => span.value.startsWith('--strategy-revision'))) {
    throw new Error('Fusion strategy revision is managed by the application.');
  }
  return `${raw.slice(0, invocation.prefixEnd)} --strategy-revision ${selection.revision}${raw.slice(invocation.prefixEnd)}`;
}

export function chooseFusionStrategy(raw: string, reference: FusionStrategyRefDto): string {
  const invocation = fusionInvocation(raw);
  if (!invocation) return raw;
  let rest = raw.slice(invocation.prefixEnd);
  for (const span of [...invocation.selectionSpans].reverse()) {
    rest = rest.slice(0, span.start - invocation.prefixEnd) + rest.slice(span.end - invocation.prefixEnd);
  }
  return `${raw.slice(0, invocation.prefixEnd)} --strategy ${fusionStrategyKey(reference)}${rest}`;
}
