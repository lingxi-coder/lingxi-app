import type {
  AgentDto,
  AuthStateDto,
  ClientEvent,
  CostDto,
  DoctorReportDto,
  ConversationControlsDto,
  HookDto,
  ModelDetailsDto,
  ProviderModelCatalogEntryDto,
  SlashCommandDto,
  PermissionModeId,
  SessionRowDto,
  StatusSnapshotDto,
  TaskRowDto,
} from '@lingxi/bridge-client';

export interface TaskOutputState {
  readonly content: string;
  readonly totalLines: number;
  readonly truncated: boolean;
}

export interface DesktopState {
  readonly sessions: SessionRowDto[];
  readonly activeSessionId: string | null;
  readonly models: string[];
  readonly modelDetails: ModelDetailsDto[];
  readonly providerModelCatalog: ProviderModelCatalogEntryDto[];
  readonly currentModel: string | null;
  readonly conversationControls: ConversationControlsDto | null;
  readonly fastMode: boolean;
  readonly permissionMode: PermissionModeId;
  readonly slashCommands: SlashCommandDto[];
  readonly tasks: Readonly<Record<string, TaskRowDto>>;
  readonly taskOutput: Readonly<Record<string, TaskOutputState>>;
  readonly status: StatusSnapshotDto | null;
  readonly doctor: DoctorReportDto | null;
  readonly auth: AuthStateDto | null;
  readonly hooks: HookDto[];
  readonly agents: AgentDto[];
  readonly lastCost: CostDto | null;
  readonly lastCompaction: Extract<ClientEvent, { type: 'compaction_completed' }> | null;
  readonly lastPermissionResolution: Extract<ClientEvent, { type: 'permission_request_resolved' }> | null;
  readonly lastApiRetry: Extract<ClientEvent, { type: 'api_retry' }> | null;
}

export function emptyDesktopState(): DesktopState {
  return {
    sessions: [],
    activeSessionId: null,
    models: [],
    modelDetails: [],
    providerModelCatalog: [],
    currentModel: null,
    conversationControls: null,
    fastMode: false,
    permissionMode: 'auto',
    slashCommands: [],
    tasks: {},
    taskOutput: {},
    status: null,
    doctor: null,
    auth: null,
    hooks: [],
    agents: [],
    lastCost: null,
    lastCompaction: null,
    lastPermissionResolution: null,
    lastApiRetry: null,
  };
}

export function beginTaskRefresh(state: DesktopState): DesktopState {
  if (Object.keys(state.tasks).length === 0 && Object.keys(state.taskOutput).length === 0) return state;
  return { ...state, tasks: {}, taskOutput: {} };
}

function mergeSessionCatalog(
  previous: readonly SessionRowDto[],
  incoming: readonly SessionRowDto[],
): SessionRowDto[] {
  const durableIds = new Set(incoming.map((session) => session.uuid));
  const provisional = previous.filter((session) => session.path === '' && !durableIds.has(session.uuid));
  return [...provisional, ...incoming];
}

function startSession(
  state: DesktopState,
  sessionId: string,
  mode: SessionRowDto['mode'] = 'code',
): DesktopState {
  const existing = state.sessions.find((session) => session.uuid === sessionId);
  const sessions = existing
      ? state.sessions
      : [{
        uuid: sessionId,
        title: 'New chat',
        modified_rfc3339: new Date().toISOString(),
        message_count: 0,
        mode,
        // Empty path marks a renderer-side row that SessionList replaces once
        // the first turn creates the durable JSONL transcript.
        path: '',
      }, ...state.sessions];
  return { ...state, sessions, activeSessionId: sessionId };
}

export function reduceDesktopEvent(state: DesktopState, event: ClientEvent): DesktopState {
  switch (event.type) {
    case 'session_list':
      return { ...state, sessions: mergeSessionCatalog(state.sessions, event.sessions) };
    case 'session_started':
      return startSession({ ...state, lastCost: null, status: null }, event.session_id, event.mode);
    case 'session_resumed':
      return { ...state, activeSessionId: event.session_id, lastCost: null, status: null };
    case 'session_ended':
      return { ...state, activeSessionId: null, lastCost: null, status: null };
    case 'model_list':
      return { ...state, models: [...event.models], modelDetails: [...(event.details ?? [])], currentModel: event.current };
    case 'provider_model_catalog':
      return { ...state, providerModelCatalog: [...event.providers] };
    case 'model_changed':
      return { ...state, currentModel: event.model };
    case 'conversation_controls_changed':
      return { ...state, conversationControls: event.controls };
    case 'permission_request_resolved':
      return { ...state, lastPermissionResolution: event };
    case 'fast_mode_changed':
      return { ...state, fastMode: event.enabled };
    case 'permission_mode_changed':
      return { ...state, permissionMode: event.mode };
    case 'turn_ended':
      // Both events carry authoritative session totals, never per-turn deltas.
      return { ...state, lastCost: event.cost };
    case 'cost_update':
      return {
        ...state,
        lastCost: {
          total_usd: event.total_usd,
          input_tokens: event.input_tokens,
          output_tokens: event.output_tokens,
          api_calls: event.api_calls,
          session_duration_secs: event.session_duration_secs,
          formatted: event.formatted,
        },
      };
    case 'compaction_completed':
      return { ...state, lastCompaction: event };
    case 'task_row':
      return { ...state, tasks: { ...state.tasks, [event.task.task_id]: event.task } };
    case 'task_status_changed': {
      const task = state.tasks[event.task_id];
      if (!task) return state;
      return {
        ...state,
        tasks: { ...state.tasks, [event.task_id]: { ...task, status: event.status } },
      };
    }
    case 'task_output_chunk':
      return {
        ...state,
        taskOutput: {
          ...state.taskOutput,
          [event.task_id]: {
            content: event.content,
            totalLines: event.total_lines,
            truncated: event.truncated,
          },
        },
      };
    case 'status_snapshot':
      return { ...state, status: event.snapshot };
    case 'doctor_report':
      return { ...state, doctor: event.report };
    case 'auth_state':
      return { ...state, auth: event.state };
    case 'hooks':
      return { ...state, hooks: [...event.hooks] };
    case 'agents':
      return { ...state, agents: [...event.agents] };
    case 'slash_command_catalog':
    case 'commands_changed':
      return { ...state, slashCommands: [...event.commands] };
    case 'api_retry':
      return { ...state, lastApiRetry: event };
    default:
      return state;
  }
}

export function reduceDesktopEvents(state: DesktopState, events: readonly ClientEvent[]): DesktopState {
  return events.reduce(reduceDesktopEvent, state);
}

export function orderedTasks(state: DesktopState): TaskRowDto[] {
  const priority = new Map([
    ['running', 0],
    ['pending', 1],
    ['failed', 2],
    ['completed', 3],
    ['cancelled', 4],
  ]);
  return Object.values(state.tasks).sort((left, right) => {
    const leftRank = priority.get(left.status.type) ?? 99;
    const rightRank = priority.get(right.status.type) ?? 99;
    return leftRank - rightRank || left.task_id.localeCompare(right.task_id);
  });
}
