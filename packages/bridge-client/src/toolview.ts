/**
 * The client-side header constructor for an orphan tool result.
 *
 * The engine derives {@link ToolHeaderDto} / {@link ToolResultDisplayDto} once
 * (`tui-core/src/tool_display/`) and ships them on the wire, so a client should
 * never re-parse `input_json` to rebuild a header. The sole remaining local
 * header use is an orphan result event, which has no input or header to render.
 *
 * This helper is pure, never throws on malformed JSON, and has no DOM or React
 * dependency, so it is unit-testable under `node --test`.
 */

import type { ToolHeaderDto, ToolVerbDto } from './protocol.js';

// ─────────────────────────────────────────────────────────────────────────────
// Lookup tables
// ─────────────────────────────────────────────────────────────────────────────

/**
 * A frozen lookup table with NO prototype.
 *
 * `Object.freeze({ … })` keeps `Object.prototype`, so a lookup keyed on a WIRE
 * value — a tool literally named `constructor`, `toString` or `valueOf` — finds
 * an inherited FUNCTION instead of a label. The verb lookup stays free of
 * prototype keys so wire names remain data, never callables.
 */
function lookupTable<T extends object>(entries: T): Readonly<T> {
  return Object.freeze(Object.assign(Object.create(null) as T, entries));
}

// ─────────────────────────────────────────────────────────────────────────────
// Redaction
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Mask the credential shapes that routinely appear in tool input/output.
 * Best-effort defence in depth — the engine redacts too, but a preview that
 * lands in a screenshot should not carry a live key.
 */
export function redactSensitiveText(value: string): string {
  return value
    .replace(/\b(sk-(?:ant-|proj-)?[A-Za-z0-9_-]{12,})\b/g, '[REDACTED]')
    .replace(/\b(Bearer\s+)[A-Za-z0-9._~+/-]+=*/gi, '$1[REDACTED]')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[=:]\s*)[^\s,;]+/gi, '$1[REDACTED]');
}

// ─────────────────────────────────────────────────────────────────────────────
// Input previews
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Probe order for a permission preview — byte-for-byte the Rust
 * `header::GENERIC_PRIMARY_KEYS`.
 */
const GENERIC_PRIMARY_KEYS = [
  'file_path',
  'path',
  'notebook_path',
  'pattern',
  'query',
  'url',
  'command',
  'name',
  'description',
] as const;

/** Parse a tool payload string; `null` for anything that is not a JSON object. */
function parseObject(json: string): Record<string, unknown> | null {
  if (!json) return null;
  try {
    const parsed: unknown = JSON.parse(json);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/** Collapse whitespace runs (including newlines) to single spaces, and trim. */
function oneLine(text: string): string {
  return text.split(/\s+/).filter(Boolean).join(' ');
}

/** A non-empty string field. */
function str(input: Record<string, unknown>, key: string): string | undefined {
  const value = input[key];
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

/** The first non-empty string among `keys`. */
function firstStr(input: Record<string, unknown>, keys: readonly string[]): string | undefined {
  for (const key of keys) {
    const value = str(input, key);
    if (value !== undefined) return value;
  }
  return undefined;
}

/**
 * The primary argument of a tool payload, VERBATIM — the one probe order every
 * caller shares. `undefined` when the payload carries no recognizable argument.
 */
function primaryArgument(inputJson: string): string | undefined {
  const input = parseObject(inputJson);
  if (!input) return undefined;
  return firstStr(input, GENERIC_PRIMARY_KEYS);
}

/**
 * A single-line, redacted preview of a tool's JSON input — the primary
 * argument a header would have parenthesized. `undefined` when the payload
 * carries no recognizable argument (callers decide what to show instead).
 *
 * For a surface where the user is DECIDING on the command rather than glancing
 * at it, use {@link toolInputDetail} — a preview is not a safe basis for a
 * security decision.
 */
export function toolInputPreview(inputJson: string): string | undefined {
  const candidate = primaryArgument(inputJson);
  if (candidate === undefined) return undefined;
  return clamp(redactSensitiveText(oneLine(candidate)), 160);
}

/**
 * The FULL, redacted primary argument — same probe order as
 * {@link toolInputPreview}, but neither collapsed to one line nor clamped.
 *
 * This is what an approval dialog must show. A permission prompt is a security
 * decision: a 40-line shell script rendered as one line cut at 160 characters
 * hides exactly the tail an attacker would append, so showing MORE is the safe
 * failure mode. The caller owns the scroll box.
 */
export function toolInputDetail(inputJson: string): string | undefined {
  const candidate = primaryArgument(inputJson);
  if (candidate === undefined) return undefined;
  return redactSensitiveText(candidate);
}

function clamp(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

// ─────────────────────────────────────────────────────────────────────────────
// Orphan result header
// ─────────────────────────────────────────────────────────────────────────────

/**
 * The English label for a verb, mirroring Rust `ToolVerb::english()`.
 * `null` for `generic`, whose label is the raw tool name.
 */
export const VERB_LABEL: Readonly<Record<ToolVerbDto, string | null>> = lookupTable({
  update: 'Update',
  create: 'Write',
  read: 'Read',
  search: 'Search',
  shell: 'Running shell command',
  output: 'Output',
  kill: 'Kill',
  fetch: 'Fetch',
  task: 'Task',
  todo: 'Update Todos',
  skill: 'Skill',
  generic: null,
});

/** Compose `label(primary)qualifier` — Rust `ToolHeader::title()`. */
export function composeToolTitle(
  label: string,
  primary?: string,
  qualifier?: string,
): string {
  let out = label;
  if (primary) out += `(${primary})`;
  if (qualifier) out += qualifier;
  return out;
}

/** Which verb a tool name maps to. Mirrors the Rust `tool_header` match arms. */
function verbFor(tool: string): ToolVerbDto {
  switch (tool) {
    case 'Edit':
    case 'MultiEdit':
    case 'NotebookEdit':
      return 'update';
    case 'Write':
      return 'create';
    case 'Read':
      return 'read';
    case 'Grep':
    case 'Glob':
      return 'search';
    case 'Bash':
    case 'Shell':
    case 'PowerShell':
    case 'REPL':
      return 'shell';
    case 'TaskOutput':
    case 'BashOutput':
    case 'BashOutputTool':
      return 'output';
    case 'TaskStop':
    case 'KillShell':
    case 'KillBash':
      return 'kill';
    case 'WebFetch':
    case 'WebSearch':
      return 'fetch';
    case 'Task':
    case 'Agent':
      return 'task';
    case 'TodoWrite':
      return 'todo';
    case 'Skill':
      return 'skill';
    default:
      return 'generic';
  }
}

/**
 * Header for an orphan result whose `tool_use_started` event was not observed.
 *
 * The result event has no input or header, so only its tool name is available.
 */
export function orphanToolHeader(tool: string): ToolHeaderDto {
  const name = tool && tool.length > 0 ? tool : 'tool';
  const verb = verbFor(name);
  let label = VERB_LABEL[verb] ?? name;
  if (name === 'REPL') {
    label = 'REPL';
  } else if (name === 'WebSearch') {
    label = 'Web Search';
  }
  return {
    verb,
    label,
    title: composeToolTitle(label),
  };
}
