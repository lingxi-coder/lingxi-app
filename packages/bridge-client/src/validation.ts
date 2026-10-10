import type {
  VisualizationBlockStatusDto,
  VisualizationMountDto,
  VisualizationRefDto,
  VisualizationRevisionDto,
  VisualizationServeDto,
  VisualizationStateWriteDto,
  AudioCapabilitySnapshotDto,
  AudioInitiatorDto,
  AudioOperationIdDto,
  AudioOperationKindDto,
  AudioOperationReadinessDto,
  AudioOperationRequestDto,
  AudioOwnerDto,
  AudioReadinessStateDto,
  ClientEvent,
  MessageDto,
  SessionAgentMessageRowDto,
  NativeUiClientAddressDto,
  NativeUiComponent,
  NativeUiControlRequest,
  NativeUiKeyedRegionDto,
  NativeUiOnScreenDto,
  NativeUiParentInputResponseDto,
  NativeUiParentPressResponseDto,
  NativeUiParentSelectResponseDto,
  NativeUiParentControlRequest,
  NativeUiSurfaceDto,
  NativeUiViewportDto,
  NativeUiControlResponse,
  NativeUiControlResponseFor,
  NativeUiRenderResponseDto,
  NativeUiClientModuleResponseDto,
  NativeUiClientPressResponseDto,
  NativeUiMessageResponseDto,
  NativeUiClientFaultResponseDto,
  UiControlMetadataDto,
  UiClientOperation,
  UiClientOperationResponseFor,
  UiClientFrameDto,
  UiClientFrameOperationResponseDto,
  UiClientHandledOperationResponseDto,
  UiClientOperationNoopResponseDto,
  UiClientWorkerFaultSnapshotDto,
  UiJsonValue,
  ServerHello,
  ServerFallbackTombstoneMessageDto,
  TaskRowDto,
} from './protocol.js';

function object(value: unknown, name = 'payload'): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${name}`);
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[], name = 'payload'): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) {
    throw new Error(`${name} contains unsupported fields`);
  }
}

function string(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0) throw new Error(`invalid ${name}`);
  return value;
}

function boolean(value: unknown, name: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`invalid ${name}`);
  return value;
}

function integer(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error(`invalid ${name}`);
  return value as number;
}

function optionalString(value: unknown, name: string): string | undefined {
  return value === undefined ? undefined : string(value, name);
}

const VISUALIZATION_ID = /^[A-Za-z0-9_-]{1,64}$/;

/** A visualization reference: `[A-Za-z0-9_-]{1,64}` id and a positive revision. */
export function visualizationRef(value: unknown, name: string): VisualizationRefDto {
  const input = object(value, name);
  exactKeys(input, ['id', 'revision'], name);
  const id = string(input['id'], `${name} id`);
  if (!VISUALIZATION_ID.test(id)) throw new Error(`invalid ${name} id`);
  const revision = integer(input['revision'], `${name} revision`);
  if (revision < 1 || revision > 0xffff_ffff) throw new Error(`invalid ${name} revision`);
  return { id, revision };
}

const VISUALIZATION_BLOCK_STATUSES: readonly VisualizationBlockStatusDto[] = ['pending', 'ready', 'unavailable', 'discarded'];

function conversationMessage(value: unknown): MessageDto {
  const input = object(value, 'session agent message');
  exactKeys(input, ['role', 'blocks', 'images', 'loop_wakeup', 'visualization_context'], 'session agent message');
  string(input['role'], 'session agent message role');
  if (!Array.isArray(input['blocks'])) throw new Error('invalid session agent message blocks');
  for (const value of input['blocks']) {
    const block = object(value, 'session agent message block');
    switch (block['type']) {
      case 'text':
        if (typeof block['text'] !== 'string') throw new Error('invalid session agent text block');
        break;
      case 'thinking':
        if (typeof block['thinking'] !== 'string') throw new Error('invalid session agent thinking block');
        if (block['signature'] !== undefined) string(block['signature'], 'session agent thinking signature');
        break;
      case 'redacted_thinking':
        if (typeof block['data'] !== 'string') throw new Error('invalid session agent redacted thinking block');
        break;
      case 'compact_boundary':
        integer(block['messages_before'], 'session agent compact messages_before');
        integer(block['messages_after'], 'session agent compact messages_after');
        if (typeof block['summary'] !== 'string') throw new Error('invalid session agent compact summary');
        break;
      case 'tool_use':
        string(block['id'], 'session agent tool use id');
        string(block['tool'], 'session agent tool name');
        string(block['input_json'], 'session agent tool input');
        break;
      case 'tool_result':
        string(block['id'], 'session agent tool result id');
        string(block['tool'], 'session agent tool result name');
        if (typeof block['result_json'] !== 'string') throw new Error('invalid session agent tool result');
        boolean(block['is_error'], 'session agent tool result error');
        break;
      case 'visualization':
        exactKeys(block, ['type', 'reference'], 'session agent visualization block');
        if (block['reference'] !== undefined) visualizationRef(block['reference'], 'session agent visualization reference');
        break;
      default: throw new Error('invalid session agent message block type');
    }
  }
  if (input['images'] !== undefined) {
    if (!Array.isArray(input['images'])) throw new Error('invalid session agent message images');
    for (const value of input['images']) {
      const image = object(value, 'session agent message image');
      exactKeys(image, ['media_type', 'url'], 'session agent message image');
      string(image['media_type'], 'session agent image media type');
      string(image['url'], 'session agent image URL');
    }
  }
  if (input['visualization_context'] !== undefined && input['visualization_context'] !== null) {
    const context = object(input['visualization_context'], 'session agent visualization context');
    exactKeys(context, ['id', 'revision', 'title'], 'session agent visualization context');
    visualizationRef({ id: context['id'], revision: context['revision'] }, 'session agent visualization context');
    string(context['title'], 'session agent visualization context title');
  }
  if (input['loop_wakeup'] !== undefined && input['loop_wakeup'] !== null) {
    const wakeup = object(input['loop_wakeup'], 'session agent loop wakeup');
    exactKeys(wakeup, ['message', 'companion', 'streak', 'since_ms'], 'session agent loop wakeup');
    if (typeof wakeup['message'] !== 'string') throw new Error('invalid session agent loop wakeup message');
    if (wakeup['companion'] !== undefined && wakeup['companion'] !== null && typeof wakeup['companion'] !== 'string') {
      throw new Error('invalid session agent loop wakeup companion');
    }
    integer(wakeup['streak'], 'session agent loop wakeup streak');
    integer(wakeup['since_ms'], 'session agent loop wakeup time');
  }
  return value as MessageDto;
}

function sessionAgentMessageRow(value: unknown): SessionAgentMessageRowDto {
  const input = object(value, 'session agent transcript row');
  exactKeys(input, ['message_index', 'message_uuid', 'message', 'api_error_json'], 'session agent transcript row');
  const messageIndex = integer(input['message_index'], 'session agent message index');
  const messageUuid = string(input['message_uuid'], 'session agent message UUID');
  const message = conversationMessage(input['message']);
  let apiErrorJson: string | undefined;
  if (input['api_error_json'] !== undefined) {
    apiErrorJson = string(input['api_error_json'], 'session agent API error JSON');
    let parsed: unknown;
    try { parsed = JSON.parse(apiErrorJson); }
    catch { throw new Error('invalid session agent API error JSON'); }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      throw new Error('invalid session agent API error JSON');
    }
  }
  return {
    message_index: messageIndex,
    message_uuid: messageUuid,
    message,
    ...(apiErrorJson === undefined ? {} : { api_error_json: apiErrorJson }),
  };
}

function taskRow(value: unknown): TaskRowDto {
  const row = object(value, 'task row');
  exactKeys(row, ['task_id', 'task_type', 'status', 'description', 'agent_id',
    'awaiting_plan_approval', 'can_resume', 'started_at_ms', 'error', 'stage',
    'kind', 'unread', 'model', 'effort'], 'task row');
  string(row['task_id'], 'task id');
  string(row['task_type'], 'task type');
  const status = object(row['status'], 'task status');
  exactKeys(status, ['type'], 'task status');
  if (!['pending', 'running', 'paused', 'completed', 'failed', 'cancelled'].includes(String(status['type']))) {
    throw new Error('invalid task status');
  }
  if (typeof row['description'] !== 'string') throw new Error('invalid task description');
  for (const key of ['agent_id', 'error', 'stage', 'kind', 'model', 'effort']) {
    if (row[key] !== undefined && typeof row[key] !== 'string') throw new Error(`invalid task ${key}`);
  }
  for (const key of ['awaiting_plan_approval', 'can_resume', 'unread']) {
    if (row[key] !== undefined) boolean(row[key], `task ${key}`);
  }
  if (row['started_at_ms'] !== undefined) integer(row['started_at_ms'], 'task start time');
  return row as unknown as TaskRowDto;
}

function fallbackTombstoneMessage(value: unknown): ServerFallbackTombstoneMessageDto {
  const row = object(value, 'server fallback tombstone row');
  exactKeys(row, [
    'uuid', 'type', 'timestamp', 'request_id', 'request_ref_json', 'message',
    'is_api_error_message', 'supersedes_uuids',
  ], 'server fallback tombstone row');
  string(row['uuid'], 'tombstone uuid');
  string(row['type'], 'tombstone type');
  string(row['timestamp'], 'tombstone timestamp');
  for (const key of ['request_id', 'request_ref_json']) {
    if (row[key] !== undefined) string(row[key], `tombstone ${key}`);
  }
  if (row['is_api_error_message'] !== undefined) boolean(row['is_api_error_message'], 'tombstone is_api_error_message');
  if (row['supersedes_uuids'] !== undefined) stringArray(row['supersedes_uuids'], 'tombstone supersedes_uuids');

  const message = object(row['message'], 'server fallback tombstone provider message');
  exactKeys(message, ['id', 'model', 'stop_reason', 'stop_details_json', 'usage_json', 'content_json'], 'server fallback tombstone provider message');
  for (const key of ['id', 'model', 'stop_reason', 'stop_details_json', 'usage_json']) {
    if (message[key] !== undefined) string(message[key], `tombstone provider ${key}`);
  }
  string(message['content_json'], 'tombstone provider content_json');

  return row as unknown as ServerFallbackTombstoneMessageDto;
}

/** Product-only scope lookup; request identity must match the pending ask. */
export function validatePermissionScope(value: unknown, requestId: number): {
  request_id: number; background_owned: boolean;
} | null {
  integer(requestId, 'permission request id');
  if (value === null) return null;
  const scope = object(value, 'permission scope');
  exactKeys(scope, ['request_id', 'background_owned'], 'permission scope');
  const id = integer(scope['request_id'], 'permission request id');
  if (id !== requestId) throw new Error('permission scope request mismatch');
  return { request_id: id, background_owned: boolean(scope['background_owned'], 'background permission scope') };
}

function stringArray(value: unknown, name: string): string[] {
  if (!Array.isArray(value)) throw new Error(`invalid ${name}`);
  return value.map((entry, index) => string(entry, `${name}[${index}]`));
}

function audioIdentity(value: unknown): AudioOperationIdDto {
  const input = object(value, 'audio operation identity');
  exactKeys(input, ['id', 'generation', 'service_epoch'], 'audio operation identity');
  return {
    id: string(input['id'], 'audio operation id'),
    generation: integer(input['generation'], 'audio operation generation'),
    service_epoch: integer(input['service_epoch'], 'audio service epoch'),
  };
}

function audioOwner(value: unknown): AudioOwnerDto {
  const input = object(value, 'audio owner');
  switch (input['type']) {
    case 'session':
      exactKeys(input, ['type', 'session_id'], 'audio owner');
      return { type: 'session', session_id: string(input['session_id'], 'audio session id') };
    case 'ui':
    case 'system':
      exactKeys(input, ['type', 'instance_id'], 'audio owner');
      return { type: input['type'], instance_id: string(input['instance_id'], 'audio owner instance id') };
    default:
      throw new Error('invalid audio owner type');
  }
}

function audioOperationRequest(value: unknown): AudioOperationRequestDto {
  const input = object(value, 'audio operation request');
  exactKeys(input, ['identity', 'owner', 'initiator', 'timeout_budget_ms', 'max_payload_bytes', 'operation'], 'audio operation request');
  let initiator: AudioInitiatorDto | undefined;
  if (input['initiator'] !== undefined) {
    const raw = object(input['initiator'], 'audio initiator');
    exactKeys(raw, ['agent_id', 'tool_use_id', 'request_id'], 'audio initiator');
    initiator = {
      ...(raw['agent_id'] === undefined ? {} : { agent_id: string(raw['agent_id'], 'audio agent id') }),
      ...(raw['tool_use_id'] === undefined ? {} : { tool_use_id: string(raw['tool_use_id'], 'audio tool use id') }),
      ...(raw['request_id'] === undefined ? {} : { request_id: string(raw['request_id'], 'audio initiator request id') }),
    };
  }
  const timeoutBudget = input['timeout_budget_ms'] === undefined
    ? undefined : integer(input['timeout_budget_ms'], 'audio timeout budget');
  return {
    identity: audioIdentity(input['identity']),
    owner: audioOwner(input['owner']),
    ...(initiator === undefined ? {} : { initiator }),
    ...(timeoutBudget === undefined ? {} : { timeout_budget_ms: timeoutBudget }),
    max_payload_bytes: integer(input['max_payload_bytes'], 'audio payload bound'),
    operation: audioOperation(input['operation']),
  };
}

function audioOperation(value: unknown): AudioOperationRequestDto['operation'] {
  const input = object(value, 'audio operation');
  const type = string(input['type'], 'audio operation type');
  switch (type) {
    case 'capture':
    case 'start_recording':
      exactKeys(input, ['type', 'sample_rate_hz', 'format'], 'audio operation');
      return {
        type,
        sample_rate_hz: integer(input['sample_rate_hz'], 'audio sample rate'),
        format: string(input['format'], 'audio recording format'),
      };
    case 'play':
      exactKeys(input, ['type', 'pcm_base64', 'sample_rate_hz'], 'audio operation');
      return { type, pcm_base64: string(input['pcm_base64'], 'audio PCM'), sample_rate_hz: integer(input['sample_rate_hz'], 'audio sample rate') };
    case 'stop_recording':
      exactKeys(input, ['type', 'handle'], 'audio operation');
      return { type, handle: string(input['handle'], 'audio recording handle') };
    case 'listen':
      exactKeys(input, ['type', 'language'], 'audio operation');
      return { type, ...(input['language'] === undefined ? {} : { language: string(input['language'], 'audio language') }) };
    case 'synthesize':
    case 'speak':
      exactKeys(input, ['type', 'text', 'language', 'rate', 'voice'], 'audio operation');
      return {
        type,
        text: string(input['text'], 'audio speech text'),
        ...(input['language'] === undefined ? {} : { language: string(input['language'], 'audio language') }),
        ...(input['rate'] === undefined ? {} : {
          rate: (() => {
            const rate = input['rate'];
            if (typeof rate !== 'number' || !Number.isFinite(rate)) throw new Error('invalid audio rate');
            return rate;
          })(),
        }),
        ...(input['voice'] === undefined ? {} : { voice: string(input['voice'], 'audio voice') }),
      };
    case 'status':
      exactKeys(input, ['type', 'handle'], 'audio operation');
      return { type, ...(input['handle'] === undefined ? {} : { handle: string(input['handle'], 'audio recording handle') }) };
    case 'end_owner':
      exactKeys(input, ['type'], 'audio operation');
      return { type };
    default:
      throw new Error('invalid audio operation type');
  }
}

function audioOperationKind(value: unknown): AudioOperationKindDto {
  const kind = string(value, 'audio operation kind');
  if (kind !== 'record' && kind !== 'listen' && kind !== 'synthesize' && kind !== 'speak') {
    throw new Error('invalid audio operation kind');
  }
  return kind;
}

function audioReadiness(value: unknown): AudioReadinessStateDto {
  const state = string(value, 'audio readiness');
  if (!['ready', 'needs_permission', 'busy', 'missing_model', 'unavailable'].includes(state)) {
    throw new Error('invalid audio readiness');
  }
  return state as AudioReadinessStateDto;
}

function audioCapabilities(value: unknown): AudioCapabilitySnapshotDto {
  const input = object(value, 'audio capability snapshot');
  exactKeys(input, ['service_epoch', 'support_revision', 'supported_operations', 'readiness', 'max_payload_bytes'], 'audio capability snapshot');
  if (!Array.isArray(input['supported_operations']) || !Array.isArray(input['readiness'])) {
    throw new Error('invalid audio capability entries');
  }
  const readiness: AudioOperationReadinessDto[] = input['readiness'].map((value) => {
    const row = object(value, 'audio readiness entry');
    exactKeys(row, ['operation', 'state'], 'audio readiness entry');
    return { operation: audioOperationKind(row['operation']), state: audioReadiness(row['state']) };
  });
  return {
    service_epoch: integer(input['service_epoch'], 'audio service epoch'),
    support_revision: integer(input['support_revision'], 'audio support revision'),
    supported_operations: input['supported_operations'].map(audioOperationKind),
    readiness,
    max_payload_bytes: integer(input['max_payload_bytes'], 'audio payload bound'),
  };
}

const NATIVE_UI_COMPONENTS = [
  'AskUserQuestion', 'UserMessage', 'AssistantMessage', 'ToolUse', 'ToolResult', 'ToolGroup',
  'ToolProgress', 'CommandOutput', 'Spinner', 'TurnDuration', 'InfoNotice', 'SessionMode',
  'PromptHint', 'AbovePrompt', 'Pane',
] as const satisfies readonly NativeUiComponent[];

const UI_PARENT_DESCRIPTOR_STRING_LIMIT = 10_000;

function uiString(value: unknown, name: string, max = 256): string {
  const result = string(value, name);
  if (result.length > max || result.includes('\0')) throw new Error(`invalid ${name}`);
  return result;
}

function uiText(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || value.length > max || value.includes('\0')) throw new Error(`invalid ${name}`);
  return value;
}

function nativeUiString(value: unknown, name: string, max?: number): string {
  if (typeof value !== 'string' || (max !== undefined && value.length > max)) throw new Error(`invalid ${name}`);
  return value;
}

function nativeUiHandle(value: unknown): number {
  if (typeof value !== 'number' || !Number.isInteger(value)) throw new Error('invalid UI handle');
  return value;
}

function nativeUiSurface(value: unknown): NativeUiSurfaceDto {
  if (value === undefined || value === 'desktop') return 'desktop';
  if (value === 'mobile' || value === 'vscode') return value;
  throw new Error('invalid UI surface');
}

function nativeUiClientId(value: unknown): string {
  const clientId = nativeUiString(value, 'UI client id');
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(clientId)) throw new Error('invalid UI client id');
  return clientId;
}

function uiComponent(value: unknown, name = 'UI component'): NativeUiComponent {
  const result = uiString(value, name, 64);
  if (!NATIVE_UI_COMPONENTS.includes(result as NativeUiComponent)) throw new Error(`invalid ${name}`);
  return result as NativeUiComponent;
}

function positiveInteger(value: unknown, name: string): number {
  const result = integer(value, name);
  if (result < 1) throw new Error(`invalid ${name}`);
  return result;
}

interface UiJsonBudget { values: number; chars: number }
interface UiJsonLimits { values: number; chars: number; depth: number }

function uiJsonValue(
  value: unknown,
  name: string,
  depth = 0,
  budget: UiJsonBudget = { values: 0, chars: 0 },
  limits: UiJsonLimits = { values: 1_000_000, chars: 16 * 1024 * 1024, depth: 128 },
): UiJsonValue {
  budget.values += 1;
  if (budget.values > limits.values || depth > limits.depth) throw new Error(`invalid ${name}: exceeds JSON bounds`);
  if (value === null || typeof value === 'boolean') return value;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new Error(`invalid ${name}`);
    return value;
  }
  if (typeof value === 'string') {
    budget.chars += value.length;
    if (budget.chars > limits.chars) throw new Error(`invalid ${name}: exceeds JSON bounds`);
    return value;
  }
  if (Array.isArray(value)) return value.map((item) => uiJsonValue(item, name, depth + 1, budget, limits));
  const input = object(value, name);
  const result: Record<string, UiJsonValue> = {};
  for (const [key, item] of Object.entries(input)) {
    budget.chars += key.length;
    if (budget.chars > limits.chars) throw new Error(`invalid ${name}: exceeds JSON bounds`);
    Object.defineProperty(result, key, {
      value: uiJsonValue(item, name, depth + 1, budget, limits),
      enumerable: true,
      configurable: true,
      writable: true,
    });
  }
  return result;
}

function uiJsonObject(value: unknown, name: string): Record<string, UiJsonValue> {
  const input = object(value, name);
  return uiJsonValue(input, name) as Record<string, UiJsonValue>;
}

function boundedUiMessageData(value: unknown): UiJsonValue {
  return uiJsonValue(value, 'UI message data', 0, { values: 0, chars: 0 }, { values: 2_000, chars: 100_000, depth: 32 });
}

function parseUiJsonString(value: unknown, name: string): UiJsonValue {
  const source = string(value, name);
  let parsed: unknown;
  try { parsed = JSON.parse(source); }
  catch { throw new Error(`invalid ${name}`); }
  return uiJsonValue(parsed, name);
}

function uiViewport(value: unknown): NativeUiViewportDto {
  const input = object(value, 'UI viewport');
  return {
    columns: positiveInteger(input['columns'], 'UI viewport columns'),
    rows: positiveInteger(input['rows'], 'UI viewport rows'),
    ...(input['isFullscreen'] === undefined ? {} : { isFullscreen: boolean(input['isFullscreen'], 'UI viewport fullscreen') }),
  };
}

function uiOnScreen(value: unknown): NativeUiOnScreenDto | null {
  if (value === null) return null;
  const input = object(value, 'UI on-screen range');
  const first = integer(input['first'], 'UI on-screen first');
  const last = integer(input['last'], 'UI on-screen last');
  const of = positiveInteger(input['of'], 'UI on-screen total');
  if (first > last || last >= of) throw new Error('invalid UI on-screen range');
  return { first, last, of };
}

function uiAddress(value: Record<string, unknown>): NativeUiClientAddressDto {
  return {
    plugin: uiString(value['plugin'], 'UI plugin'),
    component: uiComponent(value['component']),
    instance_id: uiString(value['instance_id'], 'UI instance id'),
    client: uiString(value['client'], 'UI client id'),
    module: uiString(value['module'], 'UI module id'),
  };
}

/** Mirrors Native Zod objects: normalize known protocol fields and strip unknown structural keys. */
export function validateNativeUiControlRequest(value: unknown): NativeUiControlRequest {
  const input = object(value, 'UI control request');
  const subtype = string(input['subtype'], 'UI control subtype');
  switch (subtype) {
    case 'ui_render': {
      const surface = input['surface'];
      if (surface !== 'desktop' && surface !== 'mobile' && surface !== 'vscode') throw new Error('invalid UI surface');
      let keyed: NativeUiKeyedRegionDto[] | undefined;
      if (input['keyed'] !== undefined) {
        if (!Array.isArray(input['keyed']) || input['keyed'].length > 512) throw new Error('invalid UI keyed regions');
        keyed = input['keyed'].map((value) => {
          const item = object(value, 'UI keyed region');
          return {
            plugin: uiString(item['plugin'], 'UI keyed plugin'), key: uiString(item['key'], 'UI keyed key'),
            top: integer(item['top'], 'UI keyed top'), bottom: integer(item['bottom'], 'UI keyed bottom'),
          };
        });
      }
      let bench: { seq: number; t0: number } | undefined;
      if (input['bench'] !== undefined) {
        const raw = object(input['bench'], 'UI render bench');
        if (typeof raw['t0'] !== 'number' || !Number.isFinite(raw['t0'])) throw new Error('invalid UI render bench time');
        bench = { seq: integer(raw['seq'], 'UI render bench sequence'), t0: raw['t0'] };
      }
      let clientId: string | undefined;
      if (input['client_id'] !== undefined) {
        clientId = uiString(input['client_id'], 'UI client id', 64);
        if (!/^[A-Za-z0-9._-]{1,64}$/.test(clientId)) throw new Error('invalid UI client id');
      }
      return {
        subtype, surface, component: uiComponent(input['component']),
        instance_id: uiString(input['instance_id'], 'UI instance id'),
        props: uiJsonObject(input['props'], 'UI render props'),
        ...(clientId === undefined ? {} : { client_id: clientId }),
        ...(input['viewport'] === undefined ? {} : { viewport: uiViewport(input['viewport']) }),
        ...(input['on_screen'] === undefined ? {} : { on_screen: uiOnScreen(input['on_screen']) }),
        ...(input['content_rows'] === undefined ? {} : { content_rows: integer(input['content_rows'], 'UI content rows') }),
        ...(keyed === undefined ? {} : { keyed }),
        ...(bench === undefined ? {} : { bench }),
      };
    }
    case 'ui_client_module':
      return { subtype, plugin: uiString(input['plugin'], 'UI plugin') };
    case 'ui_client_press': {
      const event = object(input['event'], 'UI client press event');
      const eventType = string(event['type'], 'UI client press event type');
      let checkedEvent: Extract<NativeUiControlRequest, { subtype: 'ui_client_press' }>['event'];
      if (eventType === 'press') {
        checkedEvent = { type: 'press' };
      } else if (eventType === 'input') {
        const kind = event['kind'];
        if (kind !== 'change' && kind !== 'submit') throw new Error('invalid UI client input kind');
        checkedEvent = { type: 'input', kind, value: uiText(event['value'], 'UI client input value', 16_384) };
      } else if (eventType === 'select') {
        checkedEvent = { type: 'select', value: uiText(event['value'], 'UI client select value', 16_384) };
      } else throw new Error('invalid UI client press event');
      return { subtype, ...uiAddress(input), element: uiString(input['element'], 'UI element', 256), event: checkedEvent };
    }
    case 'ui_message':
      return { subtype, ...uiAddress(input), data: boundedUiMessageData(input['data']) };
    case 'ui_client_fault': {
      const phase = input['phase'];
      if (phase !== 'load' && phase !== 'render' && phase !== 'run') throw new Error('invalid UI client fault phase');
      return { subtype, ...uiAddress(input), phase, reason: uiString(input['reason'], 'UI client fault reason', 200) };
    }
    case 'ui_press': {
      const request: Extract<NativeUiParentControlRequest, { subtype: 'ui_press' }> = {
        subtype,
        plugin: nativeUiString(input['plugin'], 'UI plugin'),
        handle: nativeUiHandle(input['handle']),
        surface: nativeUiSurface(input['surface']),
        ...(input['key'] === undefined ? {} : { key: nativeUiString(input['key'], 'UI key') }),
        ...(input['href'] === undefined ? {} : { href: nativeUiString(input['href'], 'UI href', 2_048) }),
        ...(input['client_id'] === undefined ? {} : { client_id: nativeUiClientId(input['client_id']) }),
      };
      return request;
    }
    case 'ui_input': {
      const kind = input['kind'];
      if (kind !== 'change' && kind !== 'submit') throw new Error('invalid UI input kind');
      const request: Extract<NativeUiParentControlRequest, { subtype: 'ui_input' }> = {
        subtype,
        plugin: nativeUiString(input['plugin'], 'UI plugin', 256),
        handle: nativeUiHandle(input['handle']),
        kind,
        value: nativeUiString(input['value'], 'UI input value', 16_384),
        surface: nativeUiSurface(input['surface']),
        ...(input['key'] === undefined ? {} : { key: nativeUiString(input['key'], 'UI key') }),
        ...(input['component'] === undefined ? {} : { component: uiComponent(input['component']) }),
        ...(input['instance_id'] === undefined ? {} : { instance_id: nativeUiString(input['instance_id'], 'UI instance id', 256) }),
        ...(input['client_id'] === undefined ? {} : { client_id: nativeUiClientId(input['client_id']) }),
      };
      return request;
    }
    case 'ui_select': {
      const request: Extract<NativeUiParentControlRequest, { subtype: 'ui_select' }> = {
        subtype,
        plugin: nativeUiString(input['plugin'], 'UI plugin', 256),
        handle: nativeUiHandle(input['handle']),
        value: nativeUiString(input['value'], 'UI select value', 16_384),
        surface: nativeUiSurface(input['surface']),
        ...(input['key'] === undefined ? {} : { key: nativeUiString(input['key'], 'UI key') }),
        ...(input['component'] === undefined ? {} : { component: uiComponent(input['component']) }),
        ...(input['instance_id'] === undefined ? {} : { instance_id: nativeUiString(input['instance_id'], 'UI instance id', 256) }),
        ...(input['client_id'] === undefined ? {} : { client_id: nativeUiClientId(input['client_id']) }),
      };
      return request;
    }
    default:
      throw new Error('invalid UI control subtype');
  }
}

/** Strict local-adapter command guard; unknown operation keys never reach the renderer boundary. */
export function validateUiClientOperation(value: unknown): UiClientOperation {
  const input = object(value, 'UI operation');
  const type = string(input['type'], 'UI operation type');
  const runtimeId = () => uiString(input['runtimeId'], 'UI runtime id');
  const revision = () => positiveInteger(input['render_revision'], 'UI render revision');
  switch (type) {
    case 'mount':
      exactKeys(input, ['type', 'surface', 'component', 'instance_id', 'plugin', 'client', 'module', 'render_revision', 'columns', 'rows'], 'UI mount operation');
      if (input['surface'] !== 'desktop') throw new Error('invalid UI operation surface');
      return {
        type, surface: 'desktop', component: uiComponent(input['component']), instance_id: uiString(input['instance_id'], 'UI instance id'),
        plugin: uiString(input['plugin'], 'UI plugin'),
        client: uiString(input['client'], 'UI client id', UI_PARENT_DESCRIPTOR_STRING_LIMIT),
        module: uiString(input['module'], 'UI module id', UI_PARENT_DESCRIPTOR_STRING_LIMIT),
        render_revision: revision(), columns: positiveInteger(input['columns'], 'UI columns'), rows: positiveInteger(input['rows'], 'UI rows'),
      };
    case 'render':
    case 'unmount':
      exactKeys(input, ['type', 'runtimeId', 'render_revision'], `UI ${type} operation`);
      return { type, runtimeId: runtimeId(), render_revision: revision() };
    case 'setProps':
      exactKeys(input, ['type', 'runtimeId', 'render_revision', 'props'], 'UI setProps operation');
      return { type, runtimeId: runtimeId(), render_revision: revision(), props: uiJsonObject(input['props'], 'UI props') };
    case 'resize':
      exactKeys(input, ['type', 'runtimeId', 'render_revision', 'columns', 'rows'], 'UI resize operation');
      return { type, runtimeId: runtimeId(), render_revision: revision(), columns: positiveInteger(input['columns'], 'UI columns'), rows: positiveInteger(input['rows'], 'UI rows') };
    case 'pointer':
    case 'key':
      exactKeys(input, ['type', 'runtimeId', 'render_revision', 'event'], `UI ${type} operation`);
      return { type, runtimeId: runtimeId(), render_revision: revision(), event: uiJsonValue(input['event'], `UI ${type} event`) };
    case 'runHeld':
      exactKeys(input, ['type', 'runtimeId', 'render_revision', 'event', 'handle'], 'UI runHeld operation');
      return {
        type, runtimeId: runtimeId(), render_revision: revision(),
        ...(input['event'] === undefined ? {} : { event: uiJsonValue(input['event'], 'UI held event') }),
        handle: positiveInteger(input['handle'], 'UI held handle'),
      };
    case 'draw_commit': {
      exactKeys(input, ['type', 'surface', 'component', 'instance_id', 'render_revision', 'clients'], 'UI draw_commit operation');
      if (input['surface'] !== 'desktop') throw new Error('invalid UI operation surface');
      if (!Array.isArray(input['clients'])) throw new Error('invalid UI draw clients');
      const clients = input['clients'].map((value) => {
        const client = object(value, 'UI draw client');
        exactKeys(client, ['plugin', 'key', 'module'], 'UI draw client');
        return {
          plugin: uiString(client['plugin'], 'UI plugin'),
          key: uiString(client['key'], 'UI client key', UI_PARENT_DESCRIPTOR_STRING_LIMIT),
          module: uiString(client['module'], 'UI module id', UI_PARENT_DESCRIPTOR_STRING_LIMIT),
        };
      });
      return { type, surface: 'desktop', component: uiComponent(input['component']), instance_id: uiString(input['instance_id'], 'UI instance id'), render_revision: revision(), clients };
    }
    case 'draw_unmount':
      exactKeys(input, ['type', 'surface', 'component', 'instance_id', 'render_revision'], 'UI draw_unmount operation');
      if (input['surface'] !== 'desktop') throw new Error('invalid UI operation surface');
      return { type, surface: 'desktop', component: uiComponent(input['component']), instance_id: uiString(input['instance_id'], 'UI instance id'), render_revision: revision() };
    default:
      throw new Error('invalid UI operation type');
  }
}

function uiClientModuleResponse(value: unknown): NativeUiClientModuleResponseDto {
  const input = object(value, 'UI client module response');
  if (!Array.isArray(input['modules']) || !Array.isArray(input['files']) || input['files'].length > 512) {
    throw new Error('invalid UI client module entries');
  }
  const modules = input['modules'].map((value) => {
    const module = object(value, 'UI client module');
    return {
      module: uiString(module['module'], 'UI module'),
      entry: uiString(module['entry'], 'UI module entry'),
      component: uiString(module['component'], 'UI module component', 64),
    };
  });
  const limitsInput = object(input['limits'], 'UI client module limits');
  const limits = {
    nodes: integer(limitsInput['nodes'], 'UI node limit'),
    depth: integer(limitsInput['depth'], 'UI depth limit'),
    chars: integer(limitsInput['chars'], 'UI character limit'),
    values: integer(limitsInput['values'], 'UI value limit'),
    dataDepth: integer(limitsInput['dataDepth'], 'UI data depth limit'),
  };
  let totalBytes = 0;
  const files = input['files'].map((value) => {
    const file = object(value, 'UI client source file');
    if (typeof file['source'] !== 'string') throw new Error('invalid UI client source');
    const byteLength = new TextEncoder().encode(file['source']).byteLength;
    if (byteLength > 1024 * 1024) throw new Error('UI client source file exceeds limit');
    totalBytes += byteLength;
    if (totalBytes > 8 * 1024 * 1024) throw new Error('UI client source manifest exceeds limit');
    return { key: uiString(file['key'], 'UI source key'), source: file['source'] };
  });
  return {
    plugin: uiString(input['plugin'], 'UI plugin'),
    hash: uiString(input['hash'], 'UI module hash', 128),
    modules,
    runtime: uiString(input['runtime'], 'UI runtime name'),
    limits,
    files,
  };
}

function nativeUiControlResponse(request: NativeUiControlRequest, value: unknown): NativeUiControlResponse {
  if (request.subtype === 'ui_client_module' && value === null) return null;
  switch (request.subtype) {
    case 'ui_render': {
      const input = object(value, 'UI render response');
      let clientModules: Record<string, string> | undefined;
      if (input['client_modules'] !== undefined) {
        const raw = object(input['client_modules'], 'UI client module hashes');
        clientModules = {};
        for (const [plugin, hash] of Object.entries(raw)) clientModules[uiString(plugin, 'UI module plugin')] = uiString(hash, 'UI module hash', 128);
      }
      const response: NativeUiRenderResponseDto = {
        tree: uiJsonValue(input['tree'], 'UI render tree'),
        props: uiJsonObject(input['props'], 'UI render props'),
        rewritten: boolean(input['rewritten'], 'UI render rewritten'),
        hooked: boolean(input['hooked'], 'UI render hooked'),
        ...(clientModules === undefined ? {} : { client_modules: clientModules }),
        ...(input['bench'] === undefined ? {} : { bench: uiJsonObject(input['bench'], 'UI render bench result') }),
      };
      return response;
    }
    case 'ui_client_module':
      return uiClientModuleResponse(value);
    case 'ui_client_press': {
      const input = object(value, 'UI client press response');
      let reached: NativeUiClientPressResponseDto['reached'];
      if (input['reached'] !== undefined) {
        const raw = object(input['reached'], 'UI client reached data');
        reached = {
          element: uiString(raw['element'], 'UI reached element', 256),
          ...(raw['value'] === undefined ? {} : { value: uiText(raw['value'], 'UI reached value', 16_384) }),
        };
      }
      const response: NativeUiClientPressResponseDto = {
        handled: boolean(input['handled'], 'UI client press handled'),
        ...(reached === undefined ? {} : { reached }),
      };
      return response;
    }
    case 'ui_press': {
      const input = object(value, 'UI parent press response');
      const response: NativeUiParentPressResponseDto = {
        handled: boolean(input['handled'], 'UI parent press handled'),
        ...(input['element'] === undefined ? {} : { element: nativeUiString(input['element'], 'UI parent press element') }),
      };
      return response;
    }
    case 'ui_input': {
      const input = object(value, 'UI parent input response');
      const response: NativeUiParentInputResponseDto = {
        handled: boolean(input['handled'], 'UI parent input handled'),
        ...(input['element'] === undefined ? {} : { element: nativeUiString(input['element'], 'UI parent input element') }),
        ...(input['value'] === undefined ? {} : { value: nativeUiString(input['value'], 'UI parent input value') }),
      };
      return response;
    }
    case 'ui_select': {
      const input = object(value, 'UI parent select response');
      const response: NativeUiParentSelectResponseDto = {
        handled: boolean(input['handled'], 'UI parent select handled'),
        ...(input['element'] === undefined ? {} : { element: nativeUiString(input['element'], 'UI parent select element') }),
        ...(input['value'] === undefined ? {} : { value: nativeUiString(input['value'], 'UI parent select value') }),
      };
      return response;
    }
    case 'ui_message': {
      const input = object(value, 'UI message response');
      const response: NativeUiMessageResponseDto = {
        handled: boolean(input['handled'], 'UI message handled'),
        ...(input['props'] === undefined ? {} : { props: uiJsonValue(input['props'], 'UI message props') }),
      };
      return response;
    }
    case 'ui_client_fault': {
      const input = object(value, 'UI client fault response');
      const response: NativeUiClientFaultResponseDto = { handled: boolean(input['handled'], 'UI client fault handled') };
      return response;
    }
  }
}

export function validateNativeUiControlResponse<T extends NativeUiControlRequest>(
  request: T,
  value: unknown,
): NativeUiControlResponseFor<T> {
  return nativeUiControlResponse(request, value) as NativeUiControlResponseFor<T>;
}

export function validateNativeUiControlResponseJson<T extends NativeUiControlRequest>(
  request: T,
  value: unknown,
): NativeUiControlResponseFor<T> {
  return validateNativeUiControlResponse(request, parseUiJsonString(value, 'UI control response JSON'));
}

export function validateUiControlMetadataJson(value: unknown): UiControlMetadataDto {
  const input = object(parseUiJsonString(value, 'UI control metadata JSON'), 'UI control metadata');
  exactKeys(input, ['renderRevision', 'clientRuntimeEpochs', 'clientStateToken'], 'UI control metadata');
  if (input['renderRevision'] === undefined && input['clientRuntimeEpochs'] === undefined
    && input['clientStateToken'] === undefined) {
    throw new Error('UI control metadata has no fields');
  }
  let clientRuntimeEpochs: Record<string, number> | undefined;
  if (input['clientRuntimeEpochs'] !== undefined) {
    const epochs = object(input['clientRuntimeEpochs'], 'UI client runtime epochs');
    clientRuntimeEpochs = Object.fromEntries(Object.entries(epochs).map(([plugin, epoch]) => [
      plugin,
      positiveInteger(epoch, `UI client runtime epoch for ${plugin}`),
    ]));
  }
  return {
    ...(input['renderRevision'] === undefined ? {} : {
      renderRevision: positiveInteger(input['renderRevision'], 'UI render revision'),
    }),
    ...(clientRuntimeEpochs === undefined ? {} : { clientRuntimeEpochs }),
    ...(input['clientStateToken'] === undefined ? {} : (() => {
      const clientStateToken = string(input['clientStateToken'], 'UI client state token');
      if (!/^[0-9]{1,20}$/.test(clientStateToken)) throw new Error('invalid UI client state token');
      return { clientStateToken };
    })()),
  };
}

export function validateUiClientOperationResponse<T extends UiClientOperation>(
  operation: T,
  value: unknown,
): UiClientOperationResponseFor<T> {
  if (operation.type === 'mount' || operation.type === 'render' || operation.type === 'setProps'
    || operation.type === 'resize' || operation.type === 'pointer' || operation.type === 'key' || operation.type === 'runHeld') {
    const input = object(value, 'UI client operation response');
    const renderRevision = positiveInteger(input['renderRevision'], 'UI render revision');
    if (renderRevision !== operation.render_revision) throw new Error('UI operation response revision mismatch');
    if (Object.hasOwn(input, 'fault')) {
      exactKeys(input, ['handled', 'renderRevision', 'runtimeId', 'fault'], 'UI worker fault response');
      if (input['handled'] !== false) throw new Error('invalid UI worker fault handled flag');
      const fault = object(input['fault'], 'UI worker fault');
      exactKeys(fault, ['phase', 'reason', 'source'], 'UI worker fault');
      const phase = fault['phase'];
      if (phase !== 'load' && phase !== 'render' && phase !== 'run') {
        throw new Error('invalid UI worker fault phase');
      }
      if (fault['source'] !== 'worker') throw new Error('invalid UI worker fault source');
      return ({
        handled: false,
        renderRevision,
        ...(input['runtimeId'] === undefined ? {} : { runtimeId: uiString(input['runtimeId'], 'UI runtime id') }),
        fault: {
          phase,
          reason: uiString(fault['reason'], 'UI worker fault reason', 200),
          source: 'worker',
        },
      } satisfies UiClientFrameOperationResponseDto) as UiClientOperationResponseFor<T>;
    }
    if (Object.hasOwn(input, 'handled')) {
      exactKeys(input, ['handled', 'renderRevision'], 'UI operation no-op response');
      if (input['handled'] !== false) throw new Error('invalid UI operation no-op handled flag');
      return ({ handled: false, renderRevision } satisfies UiClientOperationNoopResponseDto) as UiClientOperationResponseFor<T>;
    }
    exactKeys(input, ['runtimeId', 'renderRevision', 'frameSequence', 'tree', 'hasPointerListener', 'hasKeyListener'], 'UI client frame');
    return ({
      runtimeId: uiString(input['runtimeId'], 'UI runtime id'),
      renderRevision,
      frameSequence: positiveInteger(input['frameSequence'], 'UI frame sequence'),
      tree: uiJsonValue(input['tree'], 'UI client tree'),
      hasPointerListener: boolean(input['hasPointerListener'], 'UI pointer listener flag'),
      hasKeyListener: boolean(input['hasKeyListener'], 'UI key listener flag'),
    } satisfies UiClientFrameDto) as UiClientOperationResponseFor<T>;
  }
  const input = object(value, 'UI handled operation response');
  exactKeys(input, ['handled', 'renderRevision'], 'UI handled operation response');
  const renderRevision = positiveInteger(input['renderRevision'], 'UI render revision');
  if (renderRevision !== operation.render_revision) throw new Error('UI operation response revision mismatch');
  return ({ handled: boolean(input['handled'], 'UI operation handled'), renderRevision } satisfies UiClientHandledOperationResponseDto) as UiClientOperationResponseFor<T>;
}

export function validateUiClientOperationResponseJson<T extends UiClientOperation>(
  operation: T,
  value: unknown,
): UiClientOperationResponseFor<T> {
  return validateUiClientOperationResponse(operation, parseUiJsonString(value, 'UI operation response JSON')) as UiClientOperationResponseFor<T>;
}

function parseUiClientFramePayload(value: unknown) {
  const input = object(value, 'UI client frame');
  const renderRevision = positiveInteger(input['renderRevision'], 'UI render revision');
  if (Object.hasOwn(input, 'fault')) {
    exactKeys(input, ['renderRevision', 'fault'], 'UI worker fault frame');
    const fault = object(input['fault'], 'UI worker fault');
    exactKeys(fault, ['phase', 'reason', 'source'], 'UI worker fault');
    const phase = fault['phase'];
    if (phase !== 'load' && phase !== 'render' && phase !== 'run') {
      throw new Error('invalid UI worker fault phase');
    }
    if (fault['source'] !== 'worker') throw new Error('invalid UI worker fault source');
    return {
      renderRevision,
      fault: {
        phase,
        reason: uiString(fault['reason'], 'UI worker fault reason', 200),
        source: 'worker',
      },
    } satisfies UiClientWorkerFaultSnapshotDto;
  }
  exactKeys(input, ['renderRevision', 'frameSequence', 'tree', 'hasPointerListener', 'hasKeyListener'], 'UI client frame');
  return {
    renderRevision,
    frameSequence: positiveInteger(input['frameSequence'], 'UI frame sequence'),
    tree: uiJsonValue(input['tree'], 'UI client tree'),
    hasPointerListener: boolean(input['hasPointerListener'], 'UI pointer listener flag'),
    hasKeyListener: boolean(input['hasKeyListener'], 'UI key listener flag'),
  };
}

export function parseUiFrameEvent(sessionId: string, runtimeId: string, frameJson: unknown) {
  return {
    sessionId,
    runtimeId: uiString(runtimeId, 'UI runtime id'),
    frame: parseUiClientFramePayload(parseUiJsonString(frameJson, 'UI client frame')),
  };
}

export function parseUiInvalidateEvent(sessionId: string, eventSessionId: string, uuid: string, instancesJson?: unknown) {
  if (eventSessionId !== sessionId) throw new Error('UI invalidation session mismatch');
  return {
    sessionId,
    ...(instancesJson === undefined ? {} : { instances: parseUiInvalidateInstances(instancesJson) }),
    uuid: uiString(uuid, 'UI invalidation UUID'),
  };
}

function parseUiInvalidateInstances(value: unknown): Array<{ surface: 'desktop' | 'mobile' | 'vscode'; component: NativeUiComponent; instance_id: string }> {
  const parsed = typeof value === 'string' ? parseUiJsonString(value, 'UI invalidation instances') : value;
  if (!Array.isArray(parsed)) throw new Error('invalid UI invalidation instances');
  return parsed.map((value) => {
    const input = object(value, 'UI invalidation instance');
    exactKeys(input, ['surface', 'component', 'instance_id'], 'UI invalidation instance');
    const surface = input['surface'];
    if (surface !== 'desktop' && surface !== 'mobile' && surface !== 'vscode') throw new Error('invalid UI invalidation surface');
    return { surface, component: uiComponent(input['component']), instance_id: uiString(input['instance_id'], 'UI invalidation instance id') };
  });
}

function sessionMode(value: unknown, name: string): 'chat' | 'code' {
  const mode = string(value, name);
  if (mode !== 'chat' && mode !== 'code') {
    throw new Error(`invalid ${name}`);
  }
  return mode;
}

export function validateClientEvent(value: unknown): ClientEvent {
  const input = object(value, 'client event');
  const type = string(input['type'], 'client event type');
  switch (type) {
    case 'ui_control_result':
      exactKeys(input, ['type', 'request_id', 'response_json', 'metadata_json', 'error'], 'client event');
      if (input['response_json'] === undefined && input['error'] === undefined) {
        throw new Error('UI control result has neither response nor error');
      }
      return {
        type,
        request_id: string(input['request_id'], 'UI request id'),
        ...(input['response_json'] === undefined ? {} : { response_json: string(input['response_json'], 'UI response JSON') }),
        ...(input['metadata_json'] === undefined ? {} : { metadata_json: string(input['metadata_json'], 'UI response metadata JSON') }),
        ...(input['error'] === undefined ? {} : { error: string(input['error'], 'UI control error') }),
      };
    case 'ui_client_frame':
      exactKeys(input, ['type', 'runtime_id', 'frame_json'], 'client event');
      {
        const runtime_id = string(input['runtime_id'], 'UI runtime id');
        const frame_json = string(input['frame_json'], 'UI frame JSON');
        parseUiClientFramePayload(parseUiJsonString(frame_json, 'UI client frame'));
        return { type, runtime_id, frame_json };
      }
    case 'visualization_block': {
      exactKeys(input, ['type', 'status', 'reference'], 'client event');
      const status = input['status'];
      if (!VISUALIZATION_BLOCK_STATUSES.includes(status as VisualizationBlockStatusDto)) {
        throw new Error('invalid visualization block status');
      }
      const reference = input['reference'] === undefined
        ? undefined
        : visualizationRef(input['reference'], 'visualization block reference');
      if ((status === 'ready') !== (reference !== undefined)) {
        throw new Error('a visualization block carries a reference exactly when it is ready');
      }
      return {
        type,
        status: status as VisualizationBlockStatusDto,
        ...(reference === undefined ? {} : { reference }),
      };
    }
    case 'ui_invalidate':
      exactKeys(input, ['type', 'instances_json', 'uuid', 'session_id'], 'client event');
      if (input['instances_json'] !== undefined) parseUiInvalidateInstances(input['instances_json']);
      return {
        type,
        ...(input['instances_json'] === undefined ? {} : { instances_json: string(input['instances_json'], 'UI invalidation instances JSON') }),
        uuid: string(input['uuid'], 'UI invalidation UUID'),
        session_id: string(input['session_id'], 'UI invalidation session id'),
      };
    case 'query_model_change':
      exactKeys(input, ['type', 'to_model'], 'client event');
      return { type, to_model: string(input['to_model'], 'fallback model') };
    case 'assistant_block_start':
      exactKeys(input, ['type', 'block_key'], 'client event');
      return { type, block_key: integer(input['block_key'], 'assistant block key') };
    case 'assistant_block_identity':
      exactKeys(input, ['type', 'block_key', 'message_uuid'], 'client event');
      return {
        type,
        block_key: integer(input['block_key'], 'assistant block key'),
        message_uuid: string(input['message_uuid'], 'assistant row UUID'),
      };
    case 'tombstone':
      exactKeys(input, ['type', 'message', 'display_only'], 'client event');
      return {
        type,
        message: fallbackTombstoneMessage(input['message']),
        display_only: boolean(input['display_only'], 'tombstone display_only'),
      };
    case 'refusal_continuation': {
      exactKeys(input, ['type', 'phase', 'salvage_text', 'join', 'replaces_uuids', 'display_salvage_text'], 'client event');
      if (input['phase'] !== 'begin') throw new Error('invalid refusal continuation phase');
      if (input['join'] !== 'exact') throw new Error('invalid refusal continuation join');
      if (typeof input['salvage_text'] !== 'string') throw new Error('invalid refusal salvage text');
      return {
        type,
        phase: 'begin',
        salvage_text: input['salvage_text'],
        join: 'exact',
        replaces_uuids: stringArray(input['replaces_uuids'], 'refusal replaces_uuids'),
        display_salvage_text: boolean(input['display_salvage_text'], 'refusal display_salvage_text'),
      };
    }
    case 'user_transcript_row_identity':
      exactKeys(input, ['type', 'row_token', 'uuid'], 'client event');
      return {
        type,
        row_token: string(input['row_token'], 'transcript row token'),
        uuid: string(input['uuid'], 'transcript row UUID'),
      };
    case 'assistant_transcript_row_uuids':
      exactKeys(input, ['type', 'message_id', 'uuids'], 'client event');
      if (!Array.isArray(input['uuids'])) throw new Error('invalid assistant transcript row uuids');
      return {
        type,
        message_id: string(input['message_id'], 'assistant response id'),
        uuids: input['uuids'].map((uuid, index) => uuid === null ? null : string(uuid, `assistant transcript UUID[${index}]`)),
      };
    case 'ui_log':
      exactKeys(input, ['type', 'plugin', 'text'], 'client event');
      return { type, plugin: string(input['plugin'], 'plugin'), text: string(input['text'], 'text') };
    case 'ui_toast':
      exactKeys(input, ['type', 'plugin', 'text', 'timeout_ms'], 'client event');
      return { type, plugin: string(input['plugin'], 'plugin'),
        text: string(input['text'], 'text'), timeout_ms: integer(input['timeout_ms'], 'timeout_ms') };
    case 'ui_status':
      exactKeys(input, ['type', 'plugin', 'text'], 'client event');
      return { type, plugin: string(input['plugin'], 'plugin'),
        text: input['text'] === null ? null : string(input['text'], 'text') };
    case 'session_agent_transcript': {
      exactKeys(input, ['type', 'session_id', 'agent_id', 'messages', 'next_message_index', 'revision'], 'client event');
      if (!Array.isArray(input['messages'])) throw new Error('invalid session agent transcript rows');
      const messages = input['messages'].map(sessionAgentMessageRow);
      const indexes = new Set<number>();
      const uuids = new Set<string>();
      for (const row of messages) {
        if (indexes.has(row.message_index)) throw new Error('duplicate session agent message index');
        if (uuids.has(row.message_uuid)) throw new Error('duplicate session agent message UUID');
        indexes.add(row.message_index);
        uuids.add(row.message_uuid);
      }
      return {
        type,
        session_id: string(input['session_id'], 'session agent transcript session id'),
        agent_id: string(input['agent_id'], 'session agent transcript agent id'),
        messages,
        next_message_index: integer(input['next_message_index'], 'next session agent message index'),
        revision: integer(input['revision'], 'session agent transcript revision'),
      };
    }
    case 'session_agent_message': {
      exactKeys(input, ['type', 'session_id', 'agent_id', 'message_index', 'message_uuid', 'message', 'api_error_json'], 'client event');
      const row = sessionAgentMessageRow({
        message_index: input['message_index'],
        message_uuid: input['message_uuid'],
        message: input['message'],
        ...(input['api_error_json'] === undefined ? {} : { api_error_json: input['api_error_json'] }),
      });
      return {
        type,
        session_id: string(input['session_id'], 'session agent message session id'),
        agent_id: string(input['agent_id'], 'session agent message agent id'),
        message_index: row.message_index,
        message_uuid: row.message_uuid,
        message: row.message,
        ...(row.api_error_json === undefined ? {} : { api_error_json: row.api_error_json }),
      };
    }
    case 'session_agent_tombstone':
      exactKeys(input, ['type', 'session_id', 'agent_id', 'message_uuid', 'display_only'], 'client event');
      return {
        type,
        session_id: string(input['session_id'], 'session agent tombstone session id'),
        agent_id: string(input['agent_id'], 'session agent tombstone agent id'),
        message_uuid: string(input['message_uuid'], 'session agent tombstone message UUID'),
        display_only: boolean(input['display_only'], 'session agent tombstone display_only'),
      };
    case 'task_row':
      exactKeys(input, ['type', 'task'], 'client event');
      return { type, task: taskRow(input['task']) };
    case 'task_list_complete':
      exactKeys(input, ['type', 'request_id', 'active_count', 'error'], 'client event');
      return { type, request_id: string(input['request_id'], 'request_id'),
        active_count: integer(input['active_count'], 'active_count'),
        ...(input['error'] === undefined ? {} : { error: string(input['error'], 'error') }) };
    case 'openai_oauth_updated': {
      exactKeys(input, ['type', 'session'], 'client event');
      const session = object(input['session'], 'OAuth session');
      exactKeys(session, ['access_token', 'refresh_token', 'expires_at', 'account_id', 'fedramp'], 'OAuth session');
      if (typeof session['expires_at'] !== 'number' || !Number.isFinite(session['expires_at']) || typeof session['fedramp'] !== 'boolean') throw new Error('invalid OAuth session');
      return { type, session: {
        access_token: string(session['access_token'], 'access_token'),
        ...(session['refresh_token'] == null ? {} : { refresh_token: string(session['refresh_token'], 'refresh_token') }),
        ...(session['account_id'] == null ? {} : { account_id: string(session['account_id'], 'account_id') }),
        expires_at: session['expires_at'], fedramp: session['fedramp'],
      } };
    }
    case 'session_started':
      exactKeys(input, ['type', 'session_id', 'mode'], 'client event');
      return {
        type,
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
      } as ClientEvent;
    case 'session_resumed':
      exactKeys(input, ['type', 'session_id', 'mode', 'messages'], 'client event');
      if (!Array.isArray(input['messages'])) throw new Error('invalid messages');
      return {
        type,
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
        messages: input['messages'] as ClientEvent & { messages: unknown[] }['messages'],
      } as ClientEvent;
    case 'session_forked':
      exactKeys(input, ['type', 'source_session_id', 'session_id', 'mode'], 'client event');
      return {
        type,
        source_session_id: string(input['source_session_id'], 'source_session_id'),
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
      } as ClientEvent;
    case 'session_list':
      exactKeys(input, ['type', 'sessions'], 'client event');
      if (!Array.isArray(input['sessions'])) throw new Error('invalid sessions');
      for (const row of input['sessions']) {
        const item = object(row, 'session row');
        exactKeys(item, ['uuid', 'mode', 'title', 'modified_rfc3339', 'message_count', 'path'], 'session row');
        string(item['uuid'], 'session row uuid');
        sessionMode(item['mode'], 'session row mode');
        string(item['title'], 'session row title');
        string(item['modified_rfc3339'], 'session row modified_rfc3339');
        integer(item['message_count'], 'session row message_count');
        string(item['path'], 'session row path');
      }
      return input as ClientEvent;
    case 'realtime_audio_event':
      exactKeys(input, ['type', 'session_id', 'event_json'], 'client event');
      return { type, session_id: string(input['session_id'], 'realtime session id'), event_json: string(input['event_json'], 'realtime event JSON') };
    case 'audio_session_context':
      exactKeys(input, ['type', 'session_id', 'profile_id', 'account_scope'], 'client event');
      return { type, session_id: string(input['session_id'], 'audio session id'), profile_id: string(input['profile_id'], 'audio profile id'), account_scope: string(input['account_scope'], 'audio account scope') };
    case 'audio_request':
      exactKeys(input, ['type', 'request'], 'client event');
      return { type, request: audioOperationRequest(input['request']) };
    case 'audio_cancel':
      exactKeys(input, ['type', 'identity'], 'client event');
      return { type, identity: audioIdentity(input['identity']) };
    case 'audio_capabilities_changed':
      exactKeys(input, ['type', 'capabilities'], 'client event');
      return { type, capabilities: audioCapabilities(input['capabilities']) };
  }
  return input as ClientEvent;
}

export function validateServerHello(value: unknown): ServerHello {
  const input = object(value, 'ServerHello');
  exactKeys(input, ['protocol_version', 'server_name', 'capabilities'], 'ServerHello');
  const capabilities = object(input['capabilities'], 'ServerHello capabilities');
  exactKeys(
    capabilities,
    ['supports_streaming', 'supports_tools', 'supports_skills', 'supports_commands', 'client_protocol_version', 'audio', 'realtime_audio'],
    'ServerHello capabilities',
  );
  return {
    protocol_version: string(input['protocol_version'], 'protocol_version'),
    server_name: string(input['server_name'], 'server_name'),
    capabilities: {
      supports_streaming: boolean(capabilities['supports_streaming'], 'supports_streaming'),
      ...(capabilities['realtime_audio'] === undefined ? {} : { realtime_audio: boolean(capabilities['realtime_audio'], 'realtime_audio') }),
      supports_tools: boolean(capabilities['supports_tools'], 'supports_tools'),
      supports_skills: boolean(capabilities['supports_skills'], 'supports_skills'),
      supports_commands: boolean(capabilities['supports_commands'], 'supports_commands'),
      client_protocol_version: string(capabilities['client_protocol_version'], 'client_protocol_version'),
      ...(capabilities['audio'] === undefined ? {} : { audio: audioCapabilities(capabilities['audio']) }),
    },
  };
}

/** A complete product roster; partial or unrelated responses cannot release ownership. */
export function validateRuntimeSnapshot(value: unknown): ClientEvent[] {
  const result = object(value, 'runtime snapshot');
  exactKeys(result, ['events'], 'runtime snapshot');
  if (!Array.isArray(result['events'])) throw new Error('invalid runtime snapshot events');
  let rosters = 0;
  let statuses = 0;
  let taskCompletions = 0;
  const tasks = new Set<string>();
  const workers = new Set<string>();
  const events = result['events'].map((value): ClientEvent => {
    const event = object(value, 'runtime snapshot event');
    switch (event['type']) {
      case 'task_row': {
        exactKeys(event, ['type', 'task'], 'task row event');
        const task = taskRow(event['task']);
        if (tasks.has(task.task_id)) throw new Error('duplicate runtime snapshot task');
        tasks.add(task.task_id);
        break;
      }
      case 'task_list_complete': {
        const completion = validateClientEvent(event);
        if (completion.type !== 'task_list_complete' || completion.request_id !== 'desktop-runtime-snapshot'
          || completion.error !== undefined) throw new Error('invalid runtime snapshot task completion');
        taskCompletions += 1;
        break;
      }
      case 'session_agent_list':
        exactKeys(event, ['type', 'session_id', 'agents'], 'session agent roster');
        string(event['session_id'], 'roster session id');
        if (!Array.isArray(event['agents'])) throw new Error('invalid session agents');
        for (const value of event['agents']) {
          const agent = object(value, 'session agent');
          exactKeys(agent, ['agent_id', 'name', 'agent_type', 'model', 'model_profile', 'status',
            'latest_activity', 'updated_at_ms'], 'session agent');
          string(agent['agent_id'], 'agent id');
          string(agent['status'], 'agent status');
          for (const key of ['name', 'agent_type']) {
            if (typeof agent[key] !== 'string') throw new Error(`invalid agent ${key}`);
          }
          for (const key of ['model', 'model_profile', 'latest_activity']) {
            if (agent[key] !== undefined && typeof agent[key] !== 'string') throw new Error(`invalid agent ${key}`);
          }
          if (agent['updated_at_ms'] !== undefined) integer(agent['updated_at_ms'], 'agent updated time');
        }
        rosters += 1;
        break;
      case 'coordinator_worker': {
        exactKeys(event, ['type', 'worker'], 'coordinator worker event');
        const worker = object(event['worker'], 'coordinator worker');
        exactKeys(worker, ['agent_id', 'name', 'agent_type', 'status'], 'coordinator worker');
        const id = string(worker['agent_id'], 'worker id');
        if (workers.has(id)) throw new Error('duplicate runtime snapshot worker');
        workers.add(id);
        string(worker['status'], 'worker status');
        for (const key of ['name', 'agent_type']) {
          if (typeof worker[key] !== 'string') throw new Error(`invalid worker ${key}`);
        }
        break;
      }
      case 'coordinator_status':
        exactKeys(event, ['type', 'active_workers', 'team'], 'coordinator status');
        integer(event['active_workers'], 'active worker count');
        if (event['team'] !== undefined && typeof event['team'] !== 'string') throw new Error('invalid coordinator team');
        statuses += 1;
        break;
      case 'error':
        exactKeys(event, ['type', 'kind', 'message'], 'runtime snapshot warning');
        string(object(event['kind'], 'snapshot warning kind')['type'], 'snapshot warning type');
        string(event['message'], 'snapshot warning message');
        break;
      default:
        throw new Error('unsupported runtime snapshot event');
    }
    return validateClientEvent(event);
  });
  if (rosters !== 1 || statuses !== 1 || taskCompletions !== 1) throw new Error('incomplete runtime snapshot');
  return events;
}

/** A `visualization` `mount` response: a ticket, or `null` for "unavailable". */
export function validateVisualizationMount(value: unknown): VisualizationMountDto | null {
  if (value === null || value === undefined) return null;
  const input = object(value, 'visualization mount');
  exactKeys(input, ['token', 'generation', 'doc_url', 'title'], 'visualization mount');
  const token = string(input['token'], 'visualization mount token');
  if (!/^[0-9a-f]{64}$/.test(token)) throw new Error('invalid visualization mount token');
  const generation = integer(input['generation'], 'visualization mount generation');
  if (generation < 1) throw new Error('invalid visualization mount generation');
  return {
    token,
    generation,
    doc_url: string(input['doc_url'], 'visualization document URL'),
    title: string(input['title'], 'visualization title'),
  };
}

/** A `visualization` `serve` response. */
export function validateVisualizationServe(value: unknown): VisualizationServeDto {
  const input = object(value, 'visualization response');
  exactKeys(input, ['status', 'headers', 'body_base64'], 'visualization response');
  const status = integer(input['status'], 'visualization response status');
  if (status !== 200 && status !== 404) throw new Error('invalid visualization response status');
  if (!Array.isArray(input['headers'])) throw new Error('invalid visualization response headers');
  const headers = input['headers'].map((entry): [string, string] => {
    if (!Array.isArray(entry) || entry.length !== 2) throw new Error('invalid visualization response header');
    return [string(entry[0], 'visualization header name'), string(entry[1], 'visualization header value')];
  });
  const body = input['body_base64'];
  if (typeof body !== 'string') throw new Error('invalid visualization response body');
  return { status, headers, body_base64: body };
}

/** A `visualization` `write_state` response. */
export function validateVisualizationStateWrite(value: unknown): VisualizationStateWriteDto {
  const input = object(value, 'visualization state write');
  exactKeys(input, ['saved', 'version', 'reason', 'current_state'], 'visualization state write');
  const saved = boolean(input['saved'], 'visualization state saved');
  const version = integer(input['version'], 'visualization state version');
  const reason = optionalString(input['reason'], 'visualization state rejection');
  if (saved === (reason !== undefined)) throw new Error('a state write is either saved or carries a reason');
  return {
    saved,
    version,
    ...(reason === undefined ? {} : { reason }),
    ...(input['current_state'] === undefined ? {} : { current_state: input['current_state'] }),
  };
}

/** A `visualization` `list` response. */
export function validateVisualizationList(value: unknown): VisualizationRevisionDto[] {
  if (!Array.isArray(value)) throw new Error('invalid visualization list');
  return value.map((entry) => {
    const input = object(entry, 'visualization revision');
    exactKeys(input, ['id', 'revision', 'title', 'created_at_ms'], 'visualization revision');
    const reference = visualizationRef({ id: input['id'], revision: input['revision'] }, 'visualization revision');
    return {
      ...reference,
      title: string(input['title'], 'visualization revision title'),
      created_at_ms: integer(input['created_at_ms'], 'visualization revision time'),
    };
  });
}
