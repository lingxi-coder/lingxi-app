import type {
  ClientEvent,
  DoctorReportDto,
  ConversationControlsDto,
  ModelDetailsDto,
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
  readonly currentModel: string | null;
  readonly conversationControls: ConversationControlsDto | null;
  readonly fastMode: boolean;
  readonly permissionMode: PermissionModeId;
  readonly slashCommands: SlashCommandDto[];
  readonly tasks: Readonly<Record<string, TaskRowDto>>;
  readonly taskOutput: Readonly<Record<string, TaskOutputState>>;
  readonly status: StatusSnapshotDto | null;
  readonly doctor: DoctorReportDto | null;
}

export function emptyDesktopState(): DesktopState {
  return {
    sessions: [],
    activeSessionId: null,
    models: [],
    modelDetails: [],
    currentModel: null,
    conversationControls: null,
    fastMode: false,
    permissionMode: 'default',
    slashCommands: [],
    tasks: {},
    taskOutput: {},
    status: null,
    doctor: null,
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

function startSession(state: DesktopState, sessionId: string): DesktopState {
  const existing = state.sessions.find((session) => session.uuid === sessionId);
  const sessions = existing
    ? state.sessions
    : [{
        uuid: sessionId,
        title: 'New session',
        modified_rfc3339: new Date().toISOString(),
        message_count: 0,
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
      return startSession(state, event.session_id);
    case 'session_resumed':
      return { ...state, activeSessionId: event.session_id };
    case 'session_ended':
      return { ...state, activeSessionId: null };
    case 'model_list':
      return { ...state, models: [...event.models], modelDetails: [...(event.details ?? [])], currentModel: event.current };
    case 'model_changed':
      return { ...state, currentModel: event.model };
    case 'conversation_controls_changed':
      return { ...state, conversationControls: event.controls };
    case 'fast_mode_changed':
      return { ...state, fastMode: event.enabled };
    case 'permission_mode_changed':
      return { ...state, permissionMode: event.mode };
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
    case 'slash_command_catalog':
    case 'commands_changed':
      return { ...state, slashCommands: [...event.commands] };
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
