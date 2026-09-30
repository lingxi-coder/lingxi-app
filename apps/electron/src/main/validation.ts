import type {
  ClientCommand,
  ComputerAccessResponseDto,
  HookAdminCommandDto,
  ImageRefDto,
  McpAdminCommandDto,
  PermissionResponseDto,
  PluginAdminCommandDto,
  ReasoningSelectionDto,
  SkillAdminCommandDto,
} from '@lingxi/bridge-client';
import { detectImageMediaType, isSupportedImageMediaType, MAX_IMAGE_ATTACHMENTS, MAX_IMAGE_BYTES } from '../shared/imageInput.js';
import { ALLOWED_CLIENT_COMMAND_TYPES, ALLOWED_REFRESH_LISTING_KINDS } from '../shared/clientCommands.js';
import { isBase64 } from '../shared/base64.js';

const MAX_PROMPT_LENGTH = 256 * 1024;
const MAX_ID_LENGTH = 512;
const MAX_JSON_PAYLOAD_LENGTH = 64 * 1024;
const MAX_ADMIN_JSON_PAYLOAD_LENGTH = 768 * 1024;
const MAX_LIST_ITEMS = 128;
const MAX_RULE_LENGTH = 4096;
const MAX_PATH_LENGTH = 4096;
const SETTINGS_DESTINATIONS = ['user', 'project', 'local'] as const;
const PERMISSION_BEHAVIORS = ['allow', 'deny', 'ask'] as const;
const MCP_SCOPES = ['user', 'local', 'project'] as const;

/**
 * The runtime membership check for the Desktop command surface, built from
 * `../shared/clientCommands.ts`'s `ALLOWED_CLIENT_COMMAND_TYPES` — the same
 * array `AllowedClientCommand` (the compile-time gate) derives its union
 * from — plus `refresh_listings` itself, which is a separate envelope shape
 * rather than a member of that array. Restating the list here, instead of
 * importing it, is exactly how it drifted before: this file's allowlist
 * never accepted `new_session` / `resume_session` while the type claimed
 * both were part of the surface.
 */
const ALLOWED_COMMANDS: ReadonlySet<string> = new Set<string>([...ALLOWED_CLIENT_COMMAND_TYPES, 'refresh_listings']);

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

function sha256Revision(value: unknown, name: string): string {
  const revision = string(value, name, 64);
  if (!/^[0-9a-f]{64}$/.test(revision)) throw new Error(`invalid ${name}`);
  return revision;
}

function validateAdminCommand<T extends { action: string }>(
  value: unknown,
  name: string,
  readActions: readonly string[],
  writeActions: readonly string[],
  targetRequiredActions: readonly string[] = [],
  nonMutatingOperationActions: readonly string[] = [],
): T {
  const input = object(value);
  exactKeys(input, ['action', 'operation_id', 'target', 'scope', 'revision', 'payload_json']);
  const action = string(input['action'], `${name} action`, 128);
  const isRead = readActions.includes(action);
  const isNonMutatingOperation = nonMutatingOperationActions.includes(action);
  if (!isRead && !isNonMutatingOperation && !writeActions.includes(action)) throw new Error(`invalid ${name} action`);
  const command: Record<string, unknown> = { action };
  if (input['target'] !== undefined) command['target'] = string(input['target'], `${name} target`, 4096);
  if (input['scope'] !== undefined) command['scope'] = string(input['scope'], `${name} scope`, 64);
  if (isRead) {
    if (targetRequiredActions.includes(action) && command['target'] === undefined) {
      throw new Error(`invalid ${name} target`);
    }
    if (input['operation_id'] !== undefined || input['revision'] !== undefined || input['payload_json'] !== undefined) {
      throw new Error(`${name} read action contains write fields`);
    }
    return command as T;
  }
  command['operation_id'] = integer(input['operation_id'], `${name} operation id`, 1, Number.MAX_SAFE_INTEGER);
  if (!isNonMutatingOperation) {
    command['revision'] = sha256Revision(input['revision'], `${name} revision`);
  } else if (input['revision'] !== undefined) {
    command['revision'] = sha256Revision(input['revision'], `${name} revision`);
  }
  command['payload_json'] = jsonObjectString(input['payload_json'], `${name} payload`, MAX_ADMIN_JSON_PAYLOAD_LENGTH);
  return command as T;
}

export function validatePrompt(value: unknown): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_PROMPT_LENGTH || value.trim().length === 0) {
    throw new Error('invalid prompt');
  }
  return value;
}

const MAX_IMAGE_BASE64_LENGTH = Math.ceil(MAX_IMAGE_BYTES / 3) * 4;

function decodeImageBase64(value: unknown): Uint8Array {
  if (
    typeof value !== 'string'
    || value.length === 0
    || value.length > MAX_IMAGE_BASE64_LENGTH
    || value.startsWith('data:')
    // `isBase64` rather than a grouped regex: the obvious pattern throws
    // `RangeError: Maximum call stack size exceeded` past ~4 MB, so a 4.5 MB
    // pasted image used to fail the whole prompt with a stack overflow
    // instead of being validated. See `shared/base64.ts`.
    || !isBase64(value)
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
  if (input['type'] !== 'allow_once' && input['type'] !== 'allow_always' && input['type'] !== 'allow_auto' && input['type'] !== 'deny') {
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
    case 'cron_manage': {
      exactKeys(input, ['type', 'request_id', 'request']);
      const request_id = string(input['request_id'], 'cron request id', 128);
      const request = object(input['request']);
      const action = string(request['action'], 'cron action', 16);
      if (!['list', 'create', 'update', 'delete', 'pause', 'resume', 'complete', 'history'].includes(action)) throw new Error('invalid cron action');
      const keys = action === 'list' ? ['action'] : ['delete', 'pause', 'resume', 'complete', 'history'].includes(action) ? ['action', 'id']
        : action === 'create' ? ['action', 'cron', 'prompt', 'recurring', 'durable', 'expires_at', 'no_expiry', 'automation']
        : ['action', 'id', 'cron', 'prompt', 'recurring', 'durable', 'expires_at', 'no_expiry', 'automation'];
      exactKeys(request, keys);
      const result: Extract<ClientCommand, { type: 'cron_manage' }>['request'] = {
        action: action as Extract<ClientCommand, { type: 'cron_manage' }>['request']['action'],
      };
      if (['update', 'delete', 'pause', 'resume', 'complete', 'history'].includes(action)) result.id = string(request['id'], 'cron task id', 128);
      for (const key of ['cron', 'prompt'] as const) {
        if (action === 'create' || request[key] !== undefined) {
          result[key] = string(request[key], `cron ${key}`, key === 'cron' ? 256 : 100_000);
        }
      }
      for (const key of ['recurring', 'durable', 'no_expiry'] as const) {
        if (request[key] !== undefined) {
          if (typeof request[key] !== 'boolean') throw new Error(`invalid cron ${key}`);
          result[key] = request[key];
        }
      }
      if (request['expires_at'] !== undefined) result.expires_at = integer(request['expires_at'], 'cron expiry', 1, Number.MAX_SAFE_INTEGER);
      if (result.no_expiry === true && result.expires_at !== undefined) throw new Error('conflicting cron expiry');
      if (request['automation'] !== undefined) {
        const config = object(request['automation']);
        exactKeys(config, ['version', 'name', 'status', 'statusReason', 'model', 'reasoning', 'runMode', 'targetSessionId', 'notificationPolicy']);
        if (config['version'] !== 2) throw new Error('unsupported scheduled task version');
        const status = string(config['status'], 'task status', 16);
        const runMode = string(config['runMode'], 'run mode', 32);
        const notificationPolicy = string(config['notificationPolicy'], 'notification policy', 16);
        if (!['active', 'paused', 'completed'].includes(status)) throw new Error('invalid task status');
        if (!['new_session', 'selected_session', 'task_session'].includes(runMode)) throw new Error('invalid run mode');
        if (!['all', 'failed', 'none'].includes(notificationPolicy)) throw new Error('invalid notification policy');
        const reasoning = validateClientCommand({ type: 'set_reasoning_selection', selection: config['reasoning'] });
        if (reasoning.type !== 'set_reasoning_selection') throw new Error('invalid reasoning selection');
        const targetSessionId = config['targetSessionId'] === undefined ? undefined : string(config['targetSessionId'], 'target session', 128);
        if (runMode === 'selected_session' && !targetSessionId) throw new Error('select a target session');
        result.automation = {
          version: 2,
          ...(config['name'] === undefined ? {} : { name: string(config['name'], 'task name', 256) }),
          // Carried, not dropped. `exactKeys` above admits `statusReason`, and the
          // renderer sends it back untouched — `scheduledTaskInput` strips `runs`
          // and `ownedSessionId` and deliberately leaves this one. It is the
          // sentence that tells the user WHY a task stopped ("Target chat was
          // archived…", set by host.ts), and the setup screen displays it, so
          // rebuilding the record without it erased the explanation on every save.
          ...(config['statusReason'] === undefined ? {} : { statusReason: string(config['statusReason'], 'status reason', 512) }),
          status: status as 'active' | 'paused' | 'completed', model: string(config['model'], 'task model', 256), reasoning: reasoning.selection,
          runMode: runMode as 'new_session' | 'selected_session' | 'task_session',
          ...(targetSessionId ? { targetSessionId } : {}),
          notificationPolicy: notificationPolicy as 'all' | 'failed' | 'none',
        };
      }
      if (result.durable === false) throw new Error('scheduled tasks must be durable');
      return { type, request_id, request: result };
    }
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
      exactKeys(input, ['type', 'raw', 'turn_id']);
      const raw = string(input['raw'], 'slash command raw', 4096);
      if (!/^\/[^\s/]+(?:\s|$)/.test(raw)) throw new Error('invalid slash command raw');
      return input['turn_id'] === undefined
        ? { type, raw }
        : { type, raw, turn_id: integer(input['turn_id'], 'slash command turn id', 1) };
    }
    case 'list_sessions':
      exactKeys(input, ['type', 'limit']);
      return input['limit'] === undefined ? { type } : { type, limit: integer(input['limit'], 'limit', 1, 200) };
    case 'login':
    case 'logout':
    case 'force_compact':
      exactKeys(input, ['type']);
      return { type };
    case 'task_list': {
      exactKeys(input, ['type', 'status_filter', 'request_id']);
      const request = input['request_id'] === undefined ? {} : { request_id: string(input['request_id'], 'request id') };
      const status = input['status_filter'];
      if (status === undefined) return { type, ...request };
      const filter = object(status);
      exactKeys(filter, ['type']);
      const filterType = string(filter['type'], 'task status', 32);
      if (!['pending', 'running', 'paused', 'completed', 'failed', 'cancelled'].includes(filterType)) {
        throw new Error('invalid task status');
      }
      return { type, ...request, status_filter: { type: filterType } as Extract<ClientCommand, { type: 'task_list' }>['status_filter'] };
    }
    case 'task_output':
      exactKeys(input, ['type', 'task_id', 'offset']);
      return { type, task_id: string(input['task_id'], 'task id'), offset: integer(input['offset'], 'offset', 0, 10_000_000) };
    case 'task_stop':
      exactKeys(input, ['type', 'task_id']);
      return { type, task_id: string(input['task_id'], 'task id') };
    case 'task_message':
      exactKeys(input, ['type', 'task_id', 'message']);
      return { type, task_id: string(input['task_id'], 'task id'), message: string(input['message'], 'task message', MAX_PROMPT_LENGTH) };
    case 'list_session_agents':
      exactKeys(input, ['type']);
      return { type };
    case 'load_session_agent_transcript': {
      exactKeys(input, ['type', 'agent_id']);
      const agentId = string(input['agent_id'], 'agent id', 128);
      if (agentId !== 'main' && !/^agent:[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(agentId)) {
        throw new Error('invalid agent id');
      }
      return { type, agent_id: agentId };
    }
    case 'refresh_listings': {
      exactKeys(input, ['type', 'which']);
      if (!Array.isArray(input['which']) || input['which'].length === 0 || input['which'].length > 8) {
        throw new Error('invalid listing selection');
      }
      const which = input['which'].map((value) => {
        const listing = object(value);
        exactKeys(listing, ['type']);
        const kind = listing['type'] as (typeof ALLOWED_REFRESH_LISTING_KINDS)[number];
        if (!ALLOWED_REFRESH_LISTING_KINDS.includes(kind)) throw new Error('listing is not allowed');
        return { type: kind };
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
    case 'skill_admin':
      exactKeys(input, ['type', 'command']);
      return { type, command: validateAdminCommand<SkillAdminCommandDto>(
        input['command'], 'skill admin',
        ['get_catalog', 'get_document'],
        ['save_document', 'create_skill', 'move_skill', 'trash_skill', 'restore_skill', 'purge_trash_skill'],
        ['get_document'],
      ) };
    case 'mcp_admin':
      exactKeys(input, ['type', 'command']);
      return { type, command: validateAdminCommand<McpAdminCommandDto>(
        input['command'], 'mcp admin', ['get_snapshot'], ['save_server', 'remove_server', 'set_approval'],
      ) };
    case 'plugin_admin':
      exactKeys(input, ['type', 'command']);
      return { type, command: validateAdminCommand<PluginAdminCommandDto>(
        input['command'], 'plugin admin', ['get_catalog'], ['apply_operation', 'save_config'], [], ['preview_operation'],
      ) };
    case 'hook_admin':
      exactKeys(input, ['type', 'command']);
      return { type, command: validateAdminCommand<HookAdminCommandDto>(
        input['command'], 'hook admin', ['get_document'], ['save_document'], [], ['validate_document'],
      ) };
    default:
      throw new Error('command is not allowed');
  }
}

export function assertCommandAllowedDuringTurn(command: ClientCommand, turnActive: boolean): void {
  if (
    turnActive
    && (
      command.type === 'set_reasoning_selection'
      || command.type === 'set_fast_mode'
      || (command.type === 'run_slash_command' && !/^\/btw(?:\s|$)/.test(command.raw))
      || command.type === 'login'
      || command.type === 'logout'
      || command.type === 'force_compact'
      || command.type === 'new_session'
      || command.type === 'resume_session'
    )
  ) {
    throw new Error('cancel the active turn before changing session controls, authentication, compaction, or running a slash command');
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
