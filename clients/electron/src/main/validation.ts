import type {
  ClientCommand,
  ComputerAccessResponseDto,
  ImageRefDto,
  PermissionResponseDto,
  ReasoningSelectionDto,
} from '@lingxi/bridge-client';
import { detectImageMediaType, isSupportedImageMediaType, MAX_IMAGE_ATTACHMENTS, MAX_IMAGE_BYTES } from '../shared/imageInput.js';

const MAX_PROMPT_LENGTH = 256 * 1024;
const MAX_ID_LENGTH = 512;
const MAX_JSON_PAYLOAD_LENGTH = 64 * 1024;
const MAX_LIST_ITEMS = 128;
const MAX_RULE_LENGTH = 4096;
const MAX_PATH_LENGTH = 4096;
const SETTINGS_DESTINATIONS = ['user', 'project', 'local'] as const;
const PERMISSION_BEHAVIORS = ['allow', 'deny', 'ask'] as const;
const MCP_SCOPES = ['user', 'local', 'project'] as const;

/**
 * The runtime mirror of `../shared/clientCommands.ts`'s `AllowedClientCommand`
 * type. That file exports a compile-time-only type (erased at runtime), so it
 * cannot itself supply this Set or the `switch` below — this list and every
 * `case` in `validateClientCommand` must be kept byte-for-byte in sync with
 * it by hand. Changing one without the other means the desktop typechecks a
 * command it cannot actually send (or worse, cannot send one it can type).
 */
const ALLOWED_COMMANDS = new Set([
  'set_model',
  'set_permission_mode',
  'get_conversation_controls',
  'set_reasoning_selection',
  'set_fast_mode',
  'list_models',
  'run_slash_command',
  'list_sessions',
  'task_list',
  'task_output',
  'task_stop',
  'refresh_listings',
  'update_settings',
  'update_permission_rules',
  'set_default_permission_mode',
  'update_workspace_directories',
  'upsert_mcp_server',
  'remove_mcp_server',
]);

function object(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error('invalid payload');
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[]): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw new Error('payload contains unsupported fields');
}

function string(value: unknown, name: string, max = MAX_ID_LENGTH): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function integer(value: unknown, name: string, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  if (!Number.isSafeInteger(value) || (value as number) < min || (value as number) > max) {
    throw new Error(`invalid ${name}`);
  }
  return value as number;
}

/** A bounded string restricted to a fixed set of wire values. */
function enumValue<T extends string>(value: unknown, name: string, allowed: readonly T[]): T {
  const raw = string(value, name, 64);
  if (!allowed.includes(raw as T)) throw new Error(`invalid ${name}`);
  return raw as T;
}

/** A bounded array of bounded strings, e.g. permission rules or directory paths. */
function stringArray(value: unknown, name: string, maxItems: number, maxItemLength: number): string[] {
  if (!Array.isArray(value) || value.length > maxItems) throw new Error(`invalid ${name} list`);
  return value.map((item) => string(item, name, maxItemLength));
}

/**
 * A bounded string that must itself decode to a JSON object (never an array,
 * primitive, or `null`) — matching what the engine's own decoders
 * (`parse_settings_patch` / the `UpsertMcpServer.config_json` decoder in
 * `bridge-server::router`) require of `patch_json` / `config_json`. Rejecting
 * the wrong shape here gives the renderer an immediate, local error instead
 * of a round trip to learn the same thing.
 */
function jsonObjectString(value: unknown, name: string, max: number): string {
  const text = string(value, name, max);
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error(`invalid ${name}`);
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error(`invalid ${name}`);
  }
  return text;
}

export function validatePrompt(value: unknown): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_PROMPT_LENGTH || value.trim().length === 0) {
    throw new Error('invalid prompt');
  }
  return value;
}

const BASE64_PATTERN = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const MAX_IMAGE_BASE64_LENGTH = Math.ceil(MAX_IMAGE_BYTES / 3) * 4;

function decodeImageBase64(value: unknown): Uint8Array {
  if (
    typeof value !== 'string'
    || value.length === 0
    || value.length > MAX_IMAGE_BASE64_LENGTH
    || value.startsWith('data:')
    || value.length % 4 !== 0
    || !BASE64_PATTERN.test(value)
  ) {
    throw new Error('invalid image base64');
  }
  const bytes = Buffer.from(value, 'base64');
  if (bytes.length === 0 || bytes.length > MAX_IMAGE_BYTES || bytes.toString('base64') !== value) {
    throw new Error('invalid image base64');
  }
  return new Uint8Array(bytes);
}

export function validateImageRefs(value: unknown): ImageRefDto[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > MAX_IMAGE_ATTACHMENTS) throw new Error('invalid image attachments');
  return value.map((entry) => {
    const input = object(entry);
    exactKeys(input, ['media_type', 'base64']);
    if (!isSupportedImageMediaType(input['media_type'])) throw new Error('invalid image media type');
    const base64 = input['base64'];
    const bytes = decodeImageBase64(base64);
    if (detectImageMediaType(bytes) !== input['media_type']) throw new Error('invalid image format');
    return { media_type: input['media_type'], base64: base64 as string };
  });
}

export function validateOptionalTurnId(value: unknown): number | undefined {
  return value === undefined ? undefined : integer(value, 'turn id', 0);
}

export function validateRequestId(value: unknown): number {
  return integer(value, 'permission request id', 0);
}

export function validatePermissionResponse(value: unknown): PermissionResponseDto {
  if (value === undefined) return { type: 'allow_once' };
  const input = object(value);
  exactKeys(input, ['type']);
  if (input['type'] !== 'allow_once' && input['type'] !== 'allow_always' && input['type'] !== 'deny') {
    throw new Error('invalid permission response');
  }
  return { type: input['type'] };
}

export function validateComputerAccessResponse(value: unknown): ComputerAccessResponseDto {
  const input = object(value);
  exactKeys(input, ['granted_apps', 'clipboard_read', 'clipboard_write', 'system_key_combos']);
  if (!Array.isArray(input['granted_apps']) || input['granted_apps'].length > 64) {
    throw new Error('invalid computer access response');
  }
  const granted_apps = input['granted_apps'].map((label) => string(label, 'granted app label', 256));
  for (const key of ['clipboard_read', 'clipboard_write', 'system_key_combos'] as const) {
    if (typeof input[key] !== 'boolean') throw new Error('invalid computer access response');
  }
  return {
    granted_apps,
    clipboard_read: input['clipboard_read'] as boolean,
    clipboard_write: input['clipboard_write'] as boolean,
    system_key_combos: input['system_key_combos'] as boolean,
  };
}

/** Validate the bounded answer map returned by the questionnaire dialog. */
export function validateAskUserQuestionAnswers(value: unknown): Record<string, string> {
  const input = object(value);
  const entries = Object.entries(input);
  if (entries.length === 0 || entries.length > 4) {
    throw new Error('invalid AskUserQuestion answers');
  }
  const answers: Record<string, string> = {};
  for (const [question, answerValue] of entries) {
    const answer = string(answerValue, 'AskUserQuestion answer', 16 * 1024);
    if (answer.trim().length === 0) throw new Error('invalid AskUserQuestion answer');
    answers[string(question, 'AskUserQuestion question', 16 * 1024)] = answer;
  }
  return answers;
}

export function validateBridgeLockfile(
  value: unknown,
  expectedPid: number,
  expectedWorkspace: string,
): void {
  const input = object(value);
  for (const key of ['pid', 'workspaceFolders', 'ideName', 'transport', 'runningInWindows', 'authToken']) {
    if (!(key in input)) throw new Error(`bridge lockfile is missing ${key}`);
  }
  if (integer(input['pid'], 'bridge lockfile pid', 1) !== expectedPid) {
    throw new Error('bridge lockfile pid mismatch');
  }
  if (
    !Array.isArray(input['workspaceFolders'])
    || input['workspaceFolders'].length !== 1
    || input['workspaceFolders'][0] !== expectedWorkspace
  ) {
    throw new Error('bridge lockfile workspace mismatch');
  }
  if (input['ideName'] !== 'LingXi-Bridge' || input['transport'] !== 'ws') {
    throw new Error('bridge lockfile identity mismatch');
  }
  if (typeof input['runningInWindows'] !== 'boolean') {
    throw new Error('bridge lockfile platform is invalid');
  }
  if (typeof input['authToken'] !== 'string' || !/^[0-9a-f]{32}$/.test(input['authToken'])) {
    throw new Error('bridge lockfile auth token is invalid');
  }
}

/** Runtime validator for the deliberately small desktop command surface. */
export function validateClientCommand(value: unknown, workspace?: string): ClientCommand {
  const input = object(value);
  const type = input['type'];
  if (typeof type !== 'string' || !ALLOWED_COMMANDS.has(type)) throw new Error('command is not allowed');

  switch (type) {
    case 'set_model':
      exactKeys(input, ['type', 'model']);
      return { type, model: string(input['model'], 'model', 256) };
    case 'set_permission_mode': {
      exactKeys(input, ['type', 'mode']);
      // Type-check BEFORE the allow-list: String(mode) coercion would let a
      // structured-clone array like ['default'] through (String(['default'])
      // === 'default') and forward the ARRAY over the bridge, where Rust serde
      // rejects the frame. Every other validator here type-checks first.
      const mode = string(input['mode'], 'permission mode', 64);
      if (!['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'].includes(mode)) {
        throw new Error('invalid permission mode');
      }
      return { type, mode } as Extract<ClientCommand, { type: 'set_permission_mode' }>;
    }
    case 'list_models':
      exactKeys(input, ['type']);
      return { type };
    case 'get_conversation_controls':
      exactKeys(input, ['type']);
      return { type };
    case 'set_reasoning_selection':
      exactKeys(input, ['type', 'selection']);
      return { type, selection: validateReasoningSelection(input['selection']) };
    case 'set_fast_mode':
      exactKeys(input, ['type', 'enabled']);
      if (typeof input['enabled'] !== 'boolean') throw new Error('invalid fast mode enabled flag');
      return { type, enabled: input['enabled'] };
    case 'run_slash_command': {
      exactKeys(input, ['type', 'raw']);
      const raw = string(input['raw'], 'slash command raw', 4096);
      if (!/^\/[^\s/]+(?:\s|$)/.test(raw)) throw new Error('invalid slash command raw');
      return { type, raw };
    }
    case 'list_sessions':
      exactKeys(input, ['type', 'limit']);
      return input['limit'] === undefined ? { type } : { type, limit: integer(input['limit'], 'limit', 1, 200) };
    case 'task_list': {
      exactKeys(input, ['type', 'status_filter']);
      const status = input['status_filter'];
      if (status === undefined) return { type };
      const filter = object(status);
      exactKeys(filter, ['type']);
      const filterType = string(filter['type'], 'task status', 32);
      if (!['pending', 'running', 'paused', 'completed', 'failed', 'cancelled'].includes(filterType)) {
        throw new Error('invalid task status');
      }
      return { type, status_filter: { type: filterType } as Extract<ClientCommand, { type: 'task_list' }>['status_filter'] };
    }
    case 'task_output':
      exactKeys(input, ['type', 'task_id', 'offset']);
      return { type, task_id: string(input['task_id'], 'task id'), offset: integer(input['offset'], 'offset', 0, 10_000_000) };
    case 'task_stop':
      exactKeys(input, ['type', 'task_id']);
      return { type, task_id: string(input['task_id'], 'task id') };
    case 'refresh_listings': {
      exactKeys(input, ['type', 'which']);
      if (!Array.isArray(input['which']) || input['which'].length === 0 || input['which'].length > 3) {
        throw new Error('invalid listing selection');
      }
      const which = input['which'].map((value) => {
        const listing = object(value);
        exactKeys(listing, ['type']);
        if (
          listing['type'] !== 'status'
          && listing['type'] !== 'doctor'
          && listing['type'] !== 'slash_commands'
          && listing['type'] !== 'settings'
          && listing['type'] !== 'mcp'
          && listing['type'] !== 'skills'
        ) throw new Error('listing is not allowed');
        return { type: listing['type'] } as const;
      });
      return { type, which };
    }
    case 'update_settings':
      exactKeys(input, ['type', 'destination', 'patch_json']);
      return {
        type,
        destination: enumValue(input['destination'], 'settings destination', SETTINGS_DESTINATIONS),
        patch_json: jsonObjectString(input['patch_json'], 'settings patch', MAX_JSON_PAYLOAD_LENGTH),
      };
    case 'update_permission_rules':
      exactKeys(input, ['type', 'destination', 'behavior', 'add', 'remove']);
      return {
        type,
        destination: enumValue(input['destination'], 'settings destination', SETTINGS_DESTINATIONS),
        behavior: enumValue(input['behavior'], 'permission behavior', PERMISSION_BEHAVIORS),
        add: stringArray(input['add'], 'permission rule', MAX_LIST_ITEMS, MAX_RULE_LENGTH),
        remove: stringArray(input['remove'], 'permission rule', MAX_LIST_ITEMS, MAX_RULE_LENGTH),
      };
    case 'set_default_permission_mode':
      exactKeys(input, ['type', 'destination', 'mode']);
      return {
        type,
        destination: enumValue(input['destination'], 'settings destination', SETTINGS_DESTINATIONS),
        mode: string(input['mode'], 'default permission mode', 64),
      };
    case 'update_workspace_directories':
      exactKeys(input, ['type', 'destination', 'add', 'remove']);
      return {
        type,
        destination: enumValue(input['destination'], 'settings destination', SETTINGS_DESTINATIONS),
        add: stringArray(input['add'], 'workspace directory', MAX_LIST_ITEMS, MAX_PATH_LENGTH),
        remove: stringArray(input['remove'], 'workspace directory', MAX_LIST_ITEMS, MAX_PATH_LENGTH),
      };
    case 'upsert_mcp_server':
      exactKeys(input, ['type', 'scope', 'name', 'config_json']);
      return {
        type,
        scope: enumValue(input['scope'], 'mcp scope', MCP_SCOPES),
        name: string(input['name'], 'mcp server name', 256),
        config_json: jsonObjectString(input['config_json'], 'mcp server config', MAX_JSON_PAYLOAD_LENGTH),
      };
    case 'remove_mcp_server':
      exactKeys(input, ['type', 'scope', 'name']);
      return {
        type,
        scope: enumValue(input['scope'], 'mcp scope', MCP_SCOPES),
        name: string(input['name'], 'mcp server name', 256),
      };
    default:
      throw new Error('command is not allowed');
  }
}

export function assertCommandAllowedDuringTurn(command: ClientCommand, turnActive: boolean): void {
  if (
    turnActive
    && (
      command.type === 'set_model'
      || command.type === 'set_permission_mode'
      || command.type === 'set_reasoning_selection'
      || command.type === 'set_fast_mode'
      || command.type === 'run_slash_command'
      || command.type === 'new_session'
      || command.type === 'resume_session'
    )
  ) {
    throw new Error('cancel the active turn before changing the model, session, or running a slash command');
  }
}

function validateReasoningSelection(value: unknown): ReasoningSelectionDto {
  const input = object(value);
  const type = string(input['type'], 'reasoning selection type', 32);
  switch (type) {
    case 'automatic':
      exactKeys(input, ['type']);
      return { type };
    case 'disabled':
      exactKeys(input, ['type']);
      return { type };
    case 'enabled':
      exactKeys(input, ['type']);
      return { type };
    case 'level':
      exactKeys(input, ['type', 'id']);
      return { type, id: string(input['id'], 'reasoning level', 64) };
    case 'token_budget':
      exactKeys(input, ['type', 'tokens']);
      return { type, tokens: integer(input['tokens'], 'reasoning token budget', 0, 1_000_000_000) };
    default:
      throw new Error('invalid reasoning selection');
  }
}

export interface IpcSenderDescriptor {
  senderId: number;
  frameId: number;
  topFrameId: number;
  url: string;
}

export const MAX_CLIPBOARD_TEXT_CHARS = 2_000_000;

/** Bound the one plain-text clipboard capability exposed to the renderer. */
export function validateClipboardText(value: unknown): string {
  if (typeof value !== 'string' || value.length > MAX_CLIPBOARD_TEXT_CHARS) {
    throw new Error('invalid clipboard text');
  }
  return value;
}

export function isAllowedIpcSender(
  sender: IpcSenderDescriptor,
  allowedSenderIds: ReadonlySet<number>,
  allowedOrigins: ReadonlySet<string>,
): boolean {
  if (!allowedSenderIds.has(sender.senderId) || sender.frameId !== sender.topFrameId) return false;
  try {
    const url = new URL(sender.url);
    const origin = url.protocol === 'file:' ? 'file://' : url.origin;
    return allowedOrigins.has(origin);
  } catch {
    return false;
  }
}
