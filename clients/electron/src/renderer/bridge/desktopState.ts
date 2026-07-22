import type {
  ClientEvent,
  DoctorReportDto,
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
  readonly currentModel: string | null;
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
    currentModel: null,
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

export function reduceDesktopEvent(state: DesktopState, event: ClientEvent): DesktopState {
  switch (event.type) {
    case 'session_list':
      return { ...state, sessions: [...event.sessions] };
    case 'session_started':
      return { ...state, activeSessionId: event.session_id };
    case 'session_resumed':
      return { ...state, activeSessionId: event.session_id };
    case 'session_ended':
      return { ...state, activeSessionId: null };
    case 'model_list':
      return { ...state, models: [...event.models], currentModel: event.current };
    case 'model_changed':
      return { ...state, currentModel: event.model };
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
