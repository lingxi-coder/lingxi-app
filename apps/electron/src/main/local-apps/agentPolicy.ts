import { createHash } from 'node:crypto';
import { LOCAL_APP_AGENT_MAX_TIMEOUT_MS, LOCAL_APP_AGENT_MAX_TURNS, type LocalAppAgentInput, type LocalAppAgentNormalizedAttachment, type LocalAppAgentRunInput, type LocalAppAgentSettings, type LocalAppAgentToolGrant } from '../../shared/localAppAgent.js';

const MAX_PROMPT_BYTES = 256 * 1024;
const MAX_SCHEMA_BYTES = 65_536;
const MAX_TOOL_COUNT = 128;
const MAX_FILE_COUNT = 8;
const MAX_FILE_BYTES = 25 * 1024 * 1024;
const MAX_TOTAL_FILE_BYTES = 50 * 1024 * 1024;

export class LocalAppAgentError extends Error {
  constructor(readonly code: string, message: string) { super(message); this.name = 'LocalAppAgentError'; }
}
export type LocalAppAgentHostInput = LocalAppAgentRunInput;

export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalJson((value as Record<string, unknown>)[key])}`).join(',')}}`;
  const result = JSON.stringify(value);
  if (result === undefined) throw new LocalAppAgentError('invalid_input', 'Input must contain only JSON values.');
  return result;
}
export function agentDigest(value: unknown): string { return createHash('sha256').update(canonicalJson(value)).digest('hex'); }
export function sameAgentGrant(a: LocalAppAgentToolGrant, b: LocalAppAgentToolGrant): boolean {
  if (a.id !== b.id || a.kind !== b.kind) return false;
  if (a.kind === 'builtin' && b.kind === 'builtin') return a.name === b.name;
  return a.kind === 'mcp' && b.kind === 'mcp' && a.serverConfigId === b.serverConfigId && a.configRevision === b.configRevision
    && a.rawToolName === b.rawToolName && a.definitionDigest === b.definitionDigest;
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new LocalAppAgentError('invalid_input', 'Expected an object.');
  return value as Record<string, unknown>;
}
function string(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || !value.trim() || value.length > max || value.includes('\0')) throw new LocalAppAgentError('invalid_input', `Invalid ${name}.`);
  return value;
}
function positive(value: unknown, name: string, max: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < 1 || (value as number) > max) throw new LocalAppAgentError('invalid_input', `Invalid ${name}.`);
  return value as number;
}
function schema(value: unknown): Record<string, unknown> {
  const input = object(value);
  let count = 0;
  const visit = (item: unknown, depth: number): void => {
    if (++count > 4096 || depth > 32) throw new LocalAppAgentError('invalid_schema', 'The output schema is too complex.');
    if (item === null || typeof item === 'boolean' || typeof item === 'string') return;
    if (typeof item === 'number' && Number.isFinite(item)) return;
    if (Array.isArray(item)) { item.forEach(child => visit(child, depth + 1)); return; }
    if (!item || typeof item !== 'object') throw new LocalAppAgentError('invalid_schema', 'The output schema must be JSON.');
    for (const [key, child] of Object.entries(item)) {
      if (key === '$ref' && (typeof child !== 'string' || !child.startsWith('#'))) throw new LocalAppAgentError('invalid_schema', 'Only local schema references are supported.');
      visit(child, depth + 1);
    }
  };
  visit(input, 0);
  if (input.$schema !== undefined && input.$schema !== 'http://json-schema.org/draft-07/schema#' && input.$schema !== 'https://json-schema.org/draft-07/schema#') throw new LocalAppAgentError('invalid_schema', 'Use JSON Schema Draft 7.');
  if (Buffer.byteLength(canonicalJson(input)) > MAX_SCHEMA_BYTES) throw new LocalAppAgentError('invalid_schema', 'The output schema is too large.');
  return structuredClone(input);
}
function attachmentBytes(value: unknown): Buffer {
  if (value instanceof ArrayBuffer) return Buffer.from(value);
  if (ArrayBuffer.isView(value)) return Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  throw new LocalAppAgentError('invalid_attachment', 'The selected file could not be read. Choose it again.');
}

/** Rejects paths and base64 at the renderer boundary and enforces one bounded binary upload. */
export function parseAgentRunInput(value: unknown): LocalAppAgentHostInput {
  const input = object(value);
  const allowed = ['requestId', 'prompt', 'attachments', 'outputSchema', 'tools', 'skills', 'maxTurns', 'timeoutMs'];
  if (Object.keys(input).some(key => !allowed.includes(key))) throw new LocalAppAgentError('invalid_input', 'Agent input contains unsupported fields.');
  const prompt = string(input.prompt, 'prompt', MAX_PROMPT_BYTES);
  if (Buffer.byteLength(prompt) > MAX_PROMPT_BYTES) throw new LocalAppAgentError('invalid_input', 'The prompt exceeds the 256 KiB limit.');
  const requestId = string(input.requestId, 'request identifier', 128);
  const attachments: LocalAppAgentNormalizedAttachment[] = [];
  let totalBytes = 0;
  if (input.attachments !== undefined) {
    if (!Array.isArray(input.attachments) || input.attachments.length > MAX_FILE_COUNT) throw new LocalAppAgentError('invalid_attachment', 'Choose at most eight files.');
    for (const raw of input.attachments) {
      const row = object(raw);
      if (Object.keys(row).some(key => !['name', 'mediaType', 'size', 'bytes'].includes(key))) throw new LocalAppAgentError('invalid_attachment', 'Agent attachment metadata is invalid.');
      const name = string(row.name, 'attachment name', 255);
      if (name === '.' || name === '..' || /[/\\\x00-\x1f\x7f]/.test(name)) throw new LocalAppAgentError('invalid_attachment', 'Agent attachment names cannot contain paths.');
      const mediaType = string(row.mediaType, 'attachment media type', 255).toLowerCase();
      if (!/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/.test(mediaType)) throw new LocalAppAgentError('invalid_attachment', 'Agent attachment media type is invalid.');
      const bytes = attachmentBytes(row.bytes);
      if (bytes.length < 1 || bytes.length > MAX_FILE_BYTES || row.size !== bytes.length) throw new LocalAppAgentError('invalid_attachment', 'Each Agent attachment must be 25 MiB or smaller.');
      totalBytes += bytes.length;
      if (totalBytes > MAX_TOTAL_FILE_BYTES) throw new LocalAppAgentError('invalid_attachment', 'Agent attachments must total 50 MiB or less.');
      attachments.push({ name, mediaType, base64: bytes.toString('base64') });
    }
  }
  const ids = (field: unknown, name: string): string[] | undefined => {
    if (field === undefined) return undefined;
    if (!Array.isArray(field) || field.length > MAX_TOOL_COUNT) throw new LocalAppAgentError('invalid_input', `Invalid requested ${name}.`);
    const values = field.map(item => string(item, `${name} identifier`, 512));
    if (new Set(values).size !== values.length) throw new LocalAppAgentError('invalid_input', `Duplicate requested ${name}.`);
    return values.sort();
  };
  return {
    requestId, prompt,
    ...(attachments.length ? { attachments } : {}),
    ...(input.outputSchema === undefined ? {} : { outputSchema: schema(input.outputSchema) }),
    ...(input.tools === undefined ? {} : { tools: ids(input.tools, 'tools')! }),
    ...(input.skills === undefined ? {} : { skills: ids(input.skills, 'skills')! }),
    ...(input.maxTurns === undefined ? {} : { maxTurns: positive(input.maxTurns, 'turn limit', LOCAL_APP_AGENT_MAX_TURNS) }),
    ...(input.timeoutMs === undefined ? {} : { timeoutMs: positive(input.timeoutMs, 'timeout', LOCAL_APP_AGENT_MAX_TIMEOUT_MS) }),
  };
}
export function validateAgentSettings(settings: LocalAppAgentSettings): void {
  if (!settings.enabled) throw new LocalAppAgentError('permission_denied', 'Agent access has not been granted for this application.');
  string(settings.model?.providerId, 'model provider', 512); string(settings.model?.modelId, 'model identifier', 512);
  positive(settings.maxTurns, 'host turn limit', LOCAL_APP_AGENT_MAX_TURNS); positive(settings.timeoutMs, 'host timeout', LOCAL_APP_AGENT_MAX_TIMEOUT_MS);
  if (!Array.isArray(settings.tools) || settings.tools.length > MAX_TOOL_COUNT || new Set(settings.tools.map(tool => tool.id)).size !== settings.tools.length) throw new LocalAppAgentError('invalid_grant', 'The application tool grants are invalid.');
  for (const tool of settings.tools) {
    string(tool.id, 'tool identifier', 512);
    if (tool.kind === 'builtin') string(tool.name, 'tool name', 512);
    else if (tool.kind === 'mcp') {
      string(tool.serverConfigId, 'MCP server identifier', 512); string(tool.configRevision, 'MCP configuration revision', 512);
      string(tool.rawToolName, 'MCP tool name', 512); string(tool.definitionDigest, 'MCP definition digest', 512);
    } else throw new LocalAppAgentError('invalid_grant', 'The tool grant has an unknown source.');
  }
  if (!Array.isArray(settings.skills) || settings.skills.length > MAX_TOOL_COUNT || settings.skills.some(id => typeof id !== 'string' || !id.trim() || id.length > 512)
    || new Set(settings.skills).size !== settings.skills.length) throw new LocalAppAgentError('invalid_grant', 'The application Skill grants are invalid.');
}
