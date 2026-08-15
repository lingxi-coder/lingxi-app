import type { ClientCommand, ComputerAccessResponseDto, PermissionResponseDto } from '@lingxi/bridge-client';

const MAX_PROMPT_LENGTH = 256 * 1024;
const MAX_ID_LENGTH = 512;
const ALLOWED_COMMANDS = new Set([
  'set_model',
  'set_permission_mode',
  'list_models',
  'run_slash_command',
  'new_session',
  'resume_session',
  'list_sessions',
  'task_list',
  'task_output',
  'task_stop',
  'refresh_listings',
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

export function validatePrompt(value: unknown): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_PROMPT_LENGTH || value.trim().length === 0) {
    throw new Error('invalid prompt');
  }
  return value;
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
    case 'new_session': {
      exactKeys(input, ['type', 'cwd', 'model']);
      if (input['cwd'] !== undefined && input['cwd'] !== workspace) throw new Error('session cwd must match the active workspace');
      const command: ClientCommand = { type };
      if (workspace) command.cwd = workspace;
      if (input['model'] !== undefined) command.model = string(input['model'], 'model', 256);
      return command;
    }
    case 'resume_session': {
      exactKeys(input, ['type', 'session_id', 'cwd']);
      if (input['cwd'] !== undefined && input['cwd'] !== workspace) throw new Error('session cwd must match the active workspace');
      const command: ClientCommand = { type, session_id: string(input['session_id'], 'session id') };
      if (workspace) command.cwd = workspace;
      return command;
    }
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
        ) throw new Error('listing is not allowed');
        return { type: listing['type'] } as const;
      });
      return { type, which };
    }
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
      || command.type === 'run_slash_command'
      || command.type === 'new_session'
      || command.type === 'resume_session'
    )
  ) {
    throw new Error('cancel the active turn before changing the model, session, or running a slash command');
  }
}

export interface IpcSenderDescriptor {
  senderId: number;
  frameId: number;
  topFrameId: number;
  url: string;
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
