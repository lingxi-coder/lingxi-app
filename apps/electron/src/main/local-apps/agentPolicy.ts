import { createHash } from 'node:crypto';
import type { LocalAppAgentRunInput, LocalAppAgentSettings, LocalAppAgentToolGrant } from '../../shared/localAppAgent.js';
import { validateImageRefs } from '../validation.js';

export class LocalAppAgentError extends Error {
  constructor(readonly code: string, message: string) { super(message); this.name = 'LocalAppAgentError'; }
}
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
  if (Buffer.byteLength(canonicalJson(input)) > 65_536) throw new LocalAppAgentError('invalid_schema', 'The output schema is too large.');
  return structuredClone(input);
}
export function parseAgentRunInput(value: unknown): LocalAppAgentRunInput {
  const input = object(value);
  const allowed = ['requestId', 'prompt', 'images', 'outputSchema', 'tools', 'maxTurns', 'timeoutMs'];
  if (Object.keys(input).some(key => !allowed.includes(key))) throw new LocalAppAgentError('invalid_input', 'Agent input contains unsupported fields.');
  const prompt = string(input.prompt, 'prompt', 256 * 1024);
  if (Buffer.byteLength(prompt) > 256 * 1024) throw new LocalAppAgentError('invalid_input', 'The prompt exceeds the 256 KiB limit.');
  let images;
  try { images = validateImageRefs(input.images); }
  catch { throw new LocalAppAgentError('invalid_image', 'Images must be valid supported inline image attachments.'); }
  let tools: string[] | undefined;
  if (input.tools !== undefined) {
    if (!Array.isArray(input.tools) || input.tools.length > 128) throw new LocalAppAgentError('invalid_input', 'Invalid requested tools.');
    tools = [...new Set(input.tools.map(entry => string(entry, 'tool identifier', 512)))].sort();
  }
  return {
    requestId: string(input.requestId, 'request identifier', 128), prompt,
    ...(images.length ? { images } : {}), ...(input.outputSchema === undefined ? {} : { outputSchema: schema(input.outputSchema) }),
    ...(tools === undefined ? {} : { tools }),
    ...(input.maxTurns === undefined ? {} : { maxTurns: positive(input.maxTurns, 'turn limit', 10_000) }),
    ...(input.timeoutMs === undefined ? {} : { timeoutMs: positive(input.timeoutMs, 'timeout', 86_400_000) }),
  };
}
export function validateAgentSettings(settings: LocalAppAgentSettings): void {
  if (!settings.enabled) throw new LocalAppAgentError('permission_denied', 'Agent access has not been granted for this application.');
  string(settings.model?.providerId, 'model provider', 512); string(settings.model?.modelId, 'model identifier', 512);
  positive(settings.maxTurns, 'host turn limit', 10_000); positive(settings.timeoutMs, 'host timeout', 86_400_000);
  if (!Array.isArray(settings.tools) || settings.tools.length > 128 || new Set(settings.tools.map(tool => tool.id)).size !== settings.tools.length) throw new LocalAppAgentError('invalid_grant', 'The application tool grants are invalid.');
  for (const tool of settings.tools) {
    string(tool.id, 'tool identifier', 512);
    if (tool.kind === 'builtin') string(tool.name, 'tool name', 512);
    else if (tool.kind === 'mcp') {
      string(tool.serverConfigId, 'MCP server identifier', 512); string(tool.configRevision, 'MCP configuration revision', 512);
      string(tool.rawToolName, 'MCP tool name', 512); string(tool.definitionDigest, 'MCP definition digest', 512);
    } else throw new LocalAppAgentError('invalid_grant', 'The tool grant has an unknown source.');
  }
}
