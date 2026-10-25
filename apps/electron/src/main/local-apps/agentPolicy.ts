import { createHash } from 'node:crypto';
import type { Message, RunAgentInput } from '@ag-ui/core';
import { LOCAL_APP_EXTENSION_KEY, validateRunAgentInput } from '@lingxi/local-app-agui/client';
import {
  LOCAL_APP_AGENT_MAX_FILES, LOCAL_APP_AGENT_MAX_FILE_BYTES, LOCAL_APP_AGENT_MAX_TIMEOUT_MS,
  LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES, LOCAL_APP_AGENT_MAX_TURNS,
  type LocalAppAgentRunOptions, type LocalAppAgentSettings, type LocalAppAgentToolGrant,
} from '../../shared/localAppAgent.js';
import { decodeInlineAgentData } from './agentChatAssets.js';

const MAX_PROMPT_BYTES = 256 * 1024;
const MAX_SCHEMA_BYTES = 65_536;
const MAX_TOOL_COUNT = 128;
const MAX_MESSAGE_PARTS = 256;
const EXTENSION_KEY = LOCAL_APP_EXTENSION_KEY;

export class LocalAppAgentError extends Error {
  constructor(readonly code: string, message: string) { super(message); this.name = 'LocalAppAgentError'; }
}

export type LocalAppAgentContentPart =
  | { kind: 'text'; text: string }
  | { kind: 'attachment'; name: string; mediaType: string; base64: string };
export interface LocalAppAgentExecutionInput {
  requestId: string;
  protocolThreadId: string;
  protocolInputDigest: string;
  protocolMessageSha256: string;
  /** Display/title text only; ordered contentParts are the actual model input. */
  prompt: string;
  contentParts: LocalAppAgentContentPart[];
  outputSchema?: Record<string, unknown>;
  tools: string[];
  skills: string[];
  maxTurns?: number;
  timeoutMs?: number;
}
export interface ParsedLocalAppAgentInput {
  publicInput: RunAgentInput;
  threadId: string;
  runId: string;
  currentMessage: Message;
  options: LocalAppAgentRunOptions;
  executionInput: LocalAppAgentExecutionInput;
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

function record(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null ? value as Record<string, unknown> : undefined;
}
function string(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || !value.trim() || value.length > max || value.includes('\0')) throw new LocalAppAgentError('invalid_input', `Invalid ${name}.`);
  return value;
}
function positive(value: unknown, name: string, max: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < 1 || (value as number) > max) throw new LocalAppAgentError('invalid_input', `Invalid ${name}.`);
  return value as number;
}
function protocolIdentifier(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0 || Buffer.byteLength(value, 'utf8') > 128 || /\p{Cc}/u.test(value)) {
    throw new LocalAppAgentError('invalid_input', `Invalid ${name}.`);
  }
  return value;
}
function schema(value: unknown): Record<string, unknown> {
  const input = record(value);
  if (!input) throw new LocalAppAgentError('invalid_schema', 'The output schema must be an object.');
  let count = 0;
  const visit = (item: unknown, depth: number): void => {
    if (++count > 4096 || depth > 32) throw new LocalAppAgentError('invalid_schema', 'The output schema is too complex.');
    if (item === null || typeof item === 'boolean' || typeof item === 'string') return;
    if (typeof item === 'number' && Number.isFinite(item)) return;
    if (Array.isArray(item)) { item.forEach(child => visit(child, depth + 1)); return; }
    const row = record(item);
    if (!row) throw new LocalAppAgentError('invalid_schema', 'The output schema must be JSON.');
    for (const [key, child] of Object.entries(row)) {
      if (key === '$ref' && (typeof child !== 'string' || !child.startsWith('#'))) throw new LocalAppAgentError('invalid_schema', 'Only local schema references are supported.');
      visit(child, depth + 1);
    }
  };
  visit(input, 0);
  if (input.$schema !== undefined && input.$schema !== 'http://json-schema.org/draft-07/schema#' && input.$schema !== 'https://json-schema.org/draft-07/schema#') throw new LocalAppAgentError('invalid_schema', 'Use JSON Schema Draft 7.');
  if (Buffer.byteLength(canonicalJson(input)) > MAX_SCHEMA_BYTES) throw new LocalAppAgentError('invalid_schema', 'The output schema is too large.');
  return structuredClone(input);
}
function stringIds(value: unknown, name: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > MAX_TOOL_COUNT) throw new LocalAppAgentError('invalid_input', `Invalid requested ${name}.`);
  const result = value.map(item => string(item, `${name} identifier`, 512));
  if (new Set(result).size !== result.length) throw new LocalAppAgentError('invalid_input', `Duplicate requested ${name}.`);
  return result;
}
function parseOptions(input: RunAgentInput): LocalAppAgentRunOptions {
  const props = input.forwardedProps === undefined ? {} : record(input.forwardedProps);
  if (!props) throw new LocalAppAgentError('invalid_input', 'AG-UI forwardedProps must be a JSON object for Local App runs.');
  if (Object.keys(props).some(key => key !== EXTENSION_KEY)) throw new LocalAppAgentError('invalid_input', 'Unsupported forwarded properties were supplied.');
  const raw = props[EXTENSION_KEY];
  if (raw === undefined) return { version: 1, tools: [], skills: [] };
  const extension = record(raw);
  if (!extension || extension.version !== 1 || Object.keys(extension).some(key => !['version', 'outputSchema', 'tools', 'skills', 'maxTurns', 'timeoutMs'].includes(key))) {
    throw new LocalAppAgentError('invalid_input', 'The Local App Agent extension must use the supported version 1 shape.');
  }
  return {
    version: 1,
    ...(extension.outputSchema === undefined ? {} : { outputSchema: schema(extension.outputSchema) }),
    tools: stringIds(extension.tools, 'tools'),
    skills: stringIds(extension.skills, 'skills'),
    ...(extension.maxTurns === undefined ? {} : { maxTurns: positive(extension.maxTurns, 'turn limit', LOCAL_APP_AGENT_MAX_TURNS) }),
    ...(extension.timeoutMs === undefined ? {} : { timeoutMs: positive(extension.timeoutMs, 'timeout', LOCAL_APP_AGENT_MAX_TIMEOUT_MS) }),
  };
}
function validateProtocolCapabilities(input: RunAgentInput, original: unknown): void {
  if (input.protocolVersion !== undefined && input.protocolVersion !== '1.0') throw new LocalAppAgentError('unsupported_protocol_version', 'Only AG-UI 1.0 is supported.');
  if (input.parentRunId !== undefined) throw new LocalAppAgentError('unsupported_agent_input', 'Parent runs are not supported by this Local App Agent.');
  if (input.tools?.length) throw new LocalAppAgentError('frontend_tools_unsupported', 'Frontend tool definitions are not supported by this Local App host.');
  if (input.context?.length) throw new LocalAppAgentError('context_unsupported', 'RunAgentInput.context is not supported by this Local App host.');
  const originalInput = record(original);
  const state = originalInput?.state;
  if (state !== undefined) {
    const stateRecord = record(state);
    if (!stateRecord || Object.keys(stateRecord).length > 0) throw new LocalAppAgentError('state_unsupported', 'Only an empty RunAgentInput.state object is supported.');
  }
  if (input.resume?.length) throw new LocalAppAgentError('resume_unsupported', 'RunAgentInput.resume is not supported by this Local App host.');
}

function messageDigest(message: Message): string {
  const normalized = structuredClone(message) as unknown as Record<string, unknown>;
  const content = normalized.content;
  if (Array.isArray(content)) {
    for (const value of content) {
      const part = record(value);
      if (!part || (part.type !== 'image' && part.type !== 'document')) continue;
      const source = record(part.source);
      if (!source || source.type !== 'data') throw new LocalAppAgentError('invalid_message', 'The Local App user message has an unsupported attachment source.');
      const bytes = decodeInlineAgentData(source.value, LOCAL_APP_AGENT_MAX_FILE_BYTES);
      source.value = { sha256: createHash('sha256').update(bytes).digest('hex'), byteLength: bytes.length };
    }
  }
  return createHash('sha256').update(canonicalJson(normalized)).digest('hex');
}
function mapCurrentMessage(message: Message, threadId: string, runId: string, inputDigest: string, options: LocalAppAgentRunOptions): LocalAppAgentExecutionInput {
  if (message.role !== 'user') throw new LocalAppAgentError('unsupported_message_role', 'A Local App Agent run must end with one user message.');
  const parts: LocalAppAgentContentPart[] = [];
  const content = message.content;
  let attachmentCount = 0;
  let attachmentBytes = 0;
  if (typeof content === 'string') {
    if (content) parts.push({ kind: 'text', text: content });
  } else {
    if (!Array.isArray(content) || content.length > MAX_MESSAGE_PARTS) throw new LocalAppAgentError('invalid_message', 'The user message contains too many content parts.');
    let attachmentIndex = 0;
    for (const part of content) {
      if (part.type === 'text') { parts.push({ kind: 'text', text: part.text }); continue; }
      if (part.type !== 'image' && part.type !== 'document') throw new LocalAppAgentError('unsupported_content_part', `AG-UI ${part.type} content parts are not supported by this Local App host.`);
      if (part.source.type !== 'data') throw new LocalAppAgentError('unsupported_attachment_source', 'Only inline image and document DataSource parts are supported.');
      const data = decodeInlineAgentData(part.source.value, LOCAL_APP_AGENT_MAX_FILE_BYTES);
      attachmentCount += 1;
      attachmentBytes += data.length;
      if (attachmentCount > LOCAL_APP_AGENT_MAX_FILES || attachmentBytes > LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES) throw new LocalAppAgentError('invalid_attachment', 'Agent attachments must contain at most eight files and total 50 MiB.');
      const metadata = record(part.metadata);
      const appMetadata = metadata?.[EXTENSION_KEY] === undefined ? undefined : record(metadata[EXTENSION_KEY]);
      if (metadata?.[EXTENSION_KEY] !== undefined && (!appMetadata || appMetadata.version !== 1
        || Object.keys(appMetadata).some(key => key !== 'version' && key !== 'filename'))) {
        throw new LocalAppAgentError('invalid_attachment', 'The Local App attachment metadata is invalid.');
      }
      const candidate = appMetadata?.filename ?? metadata?.filename;
      attachmentIndex += 1;
      const fallbackName = part.type === 'image' ? `image-${attachmentIndex}` : `attachment-${attachmentIndex}`;
      const name = typeof candidate === 'string' && candidate.trim() ? candidate : fallbackName;
      if (name.length > 255 || name === '.' || name === '..' || /[/\\\x00-\x1f\x7f]/.test(name)) throw new LocalAppAgentError('invalid_attachment', 'An Agent attachment name is invalid.');
      if (typeof part.source.mimeType !== 'string' || !/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(part.source.mimeType)) throw new LocalAppAgentError('invalid_attachment', 'An Agent attachment media type is invalid.');
      parts.push({ kind: 'attachment', name, mediaType: part.source.mimeType.toLowerCase(), base64: part.source.value });
    }
  }
  const text = parts.filter(part => part.kind === 'text').map(part => part.text ?? '').join('\n');
  if (Buffer.byteLength(text) > MAX_PROMPT_BYTES) throw new LocalAppAgentError('invalid_input', 'The user message text exceeds the 256 KiB limit.');
  if (!text.trim() && !parts.some(part => part.kind === 'attachment')) throw new LocalAppAgentError('empty_message', 'The AG-UI user message must contain text or an attachment.');
  return {
    requestId: runId,
    protocolThreadId: threadId,
    protocolInputDigest: inputDigest,
    protocolMessageSha256: messageDigest(message),
    prompt: text,
    contentParts: parts,
    tools: options.tools ?? [], skills: options.skills ?? [],
    ...(options.maxTurns === undefined ? {} : { maxTurns: options.maxTurns }),
    ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
    ...(options.outputSchema === undefined ? {} : { outputSchema: options.outputSchema }),
  };
}

export interface RunAgentInputContext { persistentChat?: boolean; chatId?: string }
export function parseRunAgentInput(value: unknown, context: RunAgentInputContext = {}): ParsedLocalAppAgentInput {
  let input: RunAgentInput;
  try { input = validateRunAgentInput(value) as RunAgentInput; }
  catch { throw new LocalAppAgentError('invalid_input', 'The AG-UI RunAgentInput is invalid.'); }
  validateProtocolCapabilities(input, value);
  const threadId = protocolIdentifier(input.threadId, 'threadId');
  const runId = protocolIdentifier(input.runId, 'runId');
  if (context.persistentChat && context.chatId !== undefined && context.chatId !== threadId) throw new LocalAppAgentError('thread_mismatch', 'The AG-UI thread does not match this Local App chat.');
  if (!Array.isArray(input.messages) || input.messages.length === 0 || input.messages.length > 10_000) throw new LocalAppAgentError('invalid_message', 'The AG-UI message history is invalid.');
  if (!context.persistentChat && (input.messages.length !== 1 || input.messages[0]?.role !== 'user')) {
    throw new LocalAppAgentError('stateless_history_unsupported', 'An independent Local App Agent run requires exactly one user message on a fresh thread.');
  }
  const currentMessage = input.messages.at(-1)!;
  const extension = parseOptions(input);
  const publicInput = structuredClone(value as RunAgentInput);
  const executionInput = mapCurrentMessage(currentMessage, threadId, runId, agentDigest(publicInput), extension);
  // Keep the caller's complete validated JSON envelope for exact runId dedupe.
  // The private execution DTO is separately normalized from only the current
  // user message and never substitutes for canonical chat history.
  return { publicInput, threadId, runId, currentMessage, options: extension, executionInput };
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
