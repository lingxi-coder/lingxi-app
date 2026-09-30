/**
 * The ONE client-side fallback for presenting a tool call.
 *
 * The engine derives {@link ToolHeaderDto} / {@link ToolResultDisplayDto} once
 * (`tui-core/src/tool_display/`) and ships them on the wire, so a client should
 * never re-parse `input_json` to rebuild a header. This module exists purely so
 * an OLDER engine — one that sends neither field — still renders something, and
 * so that "something" is computed in exactly ONE place instead of once per
 * surface. Before this module the desktop client had two divergent copies of
 * `previewToolInput` that disagreed on probe order.
 *
 * Everything here is pure, never throws on malformed JSON, and has no DOM or
 * React dependency, so it is unit-testable under `node --test`.
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
 * an inherited FUNCTION instead of falling through to the `??` fallback. That
 * turned `PRIMARY_KEYS[name] ?? GENERIC_PRIMARY_KEYS` into a `for…of` over a
 * function ("keys is not iterable"), thrown from inside a React setState
 * updater, in a module whose header promises it never throws. Every table below
 * is indexed by a string the engine (or the model) chose, so every one of them
 * is built here.
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
 * Probe order for a tool with no table entry — byte-for-byte the Rust
 * `header::GENERIC_PRIMARY_KEYS`, so the degraded fallback picks the same
 * argument the engine would have.
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
// Header fallback
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

/** Resolve the `mcp__{server}__{tool}` pair of a namespaced MCP call. */
function mcpParts(tool: string, input: Record<string, unknown> | null): [string, string] | null {
  const fullName = tool === 'MCP' ? (input && str(input, 'full_name')) : tool;
  if (!fullName || !fullName.startsWith('mcp__')) return null;
  const rest = fullName.slice('mcp__'.length);
  const split = rest.indexOf('__');
  if (split < 0) return null;
  return [rest.slice(0, split), rest.slice(split + 2)];
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
 * The primary argument each verb parenthesizes, by tool. An EMPTY list means
 * the tool deliberately has none — the shell verbs put the command on the
 * sub-line, and `TodoWrite` shows a bare label. Without the empty entry these
 * would fall through to {@link GENERIC_PRIMARY_KEYS} and grow a `(…)` the
 * engine never renders.
 */
const PRIMARY_KEYS: Readonly<Record<string, readonly string[]>> = lookupTable({
  Bash: [],
  Shell: [],
  PowerShell: [],
  REPL: [],
  TodoWrite: [],
  Edit: ['file_path'],
  MultiEdit: ['file_path'],
  Write: ['file_path'],
  NotebookEdit: ['notebook_path', 'file_path'],
  Read: ['file_path'],
  Grep: ['pattern'],
  Glob: ['pattern'],
  TaskOutput: ['bash_id', 'shell_id', 'task_id'],
  BashOutput: ['bash_id', 'shell_id', 'task_id'],
  BashOutputTool: ['bash_id', 'shell_id', 'task_id'],
  TaskStop: ['shell_id', 'task_id'],
  KillShell: ['shell_id', 'task_id'],
  KillBash: ['shell_id', 'task_id'],
  WebFetch: ['url'],
  WebSearch: ['query'],
  Task: ['description'],
  Agent: ['description'],
  Skill: ['command', 'name', 'skill'],
});

/**
 * DEGRADED header for an engine that shipped no `header` field.
 *
 * Deliberately a subset of the Rust table: verb, label, primary argument and
 * the shell sub-line. The qualifiers (`(3 edits)`, `(lines 1-40)`) are the
 * engine's job — reproducing them here would recreate the four-way drift this
 * whole change deletes. Never call this when `header` is present.
 */
export function fallbackToolHeader(tool: string, inputJson: string): ToolHeaderDto {
  const input = parseObject(inputJson);
  const name = tool && tool.length > 0 ? tool : 'tool';

  const mcp = mcpParts(name, input);
  if (mcp) {
    const [server, mcpTool] = mcp;
    const primary = input ? firstStr(input, GENERIC_PRIMARY_KEYS) : undefined;
    const shown = primary === undefined ? undefined : clamp(redactSensitiveText(oneLine(primary)), 160);
    const qualifier = ` (${server} MCP)`;
    return {
      verb: 'generic',
      label: mcpTool,
      ...(shown === undefined ? {} : { primary: shown }),
      qualifier,
      title: composeToolTitle(mcpTool, shown, qualifier),
    };
  }

  const verb = verbFor(name);
  let label = VERB_LABEL[verb] ?? name;
  if (name === 'Bash' || name === 'Shell' || name === 'PowerShell') {
    label = 'Running 1 shell command…';
  } else if (name === 'REPL') {
    label = 'REPL';
  } else if (name === 'WebSearch') {
    label = 'Web Search';
  } else if ((name === 'Task' || name === 'Agent') && input) {
    const kind = str(input, 'subagent_type');
    if (kind && kind !== 'general-purpose' && kind !== 'worker') label = kind;
  }

  const keys = PRIMARY_KEYS[name] ?? GENERIC_PRIMARY_KEYS;
  const raw = input ? firstStr(input, keys) : undefined;
  const primary = raw === undefined ? undefined : clamp(redactSensitiveText(oneLine(raw)), 160);

  const header: ToolHeaderDto = {
    verb,
    label,
    ...(primary === undefined ? {} : { primary }),
    title: composeToolTitle(label, primary),
  };

  if (input && (name === 'Bash' || name === 'Shell' || name === 'PowerShell')) {
    const command = str(input, 'command');
    if (command) header.sub_line = { prefix: '$', text: redactSensitiveText(oneLine(command)) };
  }
  if (input && name === 'REPL') {
    const code = str(input, 'code');
    if (code) header.sub_line = { prefix: '›', text: redactSensitiveText(oneLine(code)) };
  }
  return header;
}

// ─────────────────────────────────────────────────────────────────────────────
// Result-body fallback
// ─────────────────────────────────────────────────────────────────────────────

/** Wire cap for the degraded body, matching the old desktop summarizer. */
const FALLBACK_BODY_MAX = 2_000;

/**
 * DEGRADED expandable body for an engine that shipped no `display` field:
 * the tool result rendered as plain text, redacted and clamped. `undefined`
 * when the result carries nothing worth expanding.
 */
export function fallbackToolBody(resultJson: string): string | undefined {
  if (!resultJson) return undefined;
  let text = resultJson;
  try {
    const parsed: unknown = JSON.parse(resultJson);
    text = typeof parsed === 'string' ? parsed : JSON.stringify(parsed, null, 2);
  } catch {
    // Preserve non-JSON tool output verbatim.
  }
  const safe = redactSensitiveText(text.trim());
  if (!safe) return undefined;
  return clamp(safe, FALLBACK_BODY_MAX);
}
