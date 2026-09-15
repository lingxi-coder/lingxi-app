import type {
  AttachmentDto,
  ClientEvent,
  ImageRefDto,
  MessageDto,
  PlanTaskDto,
  PermissionRequest,
  SessionAgentSummaryDto,
  TaskRowDto,
} from '@lingxi/bridge-client';
import { fileMentionsFromPrompt } from './fileMentions';
import { emptySubmittedPlanState, latestSubmittedPlan, reduceSubmittedPlanEvent, reduceSubmittedPlanPermission, type SubmittedPlan, type SubmittedPlanState } from './submittedPlan';

export type RuntimeCenterSection = 'tasks' | 'agents' | 'todos' | 'resources' | 'plan' | 'review';

export type RuntimeCenterItemRef =
  | { kind: 'section'; id: 'agents' | 'todos' | 'resources' | 'plan' | 'review' }
  | { kind: 'todo'; id: string }
  | { kind: 'plan-document'; id: string }
  | { kind: 'task'; id: string }
  | { kind: 'agent'; id: string }
  | { kind: 'resource'; id: string }
  | { kind: 'plan'; id: string };

export interface RuntimeResource {
  readonly id: string;
  readonly kind: 'image' | 'file' | 'attachment';
  readonly name: string;
  readonly path?: string;
  readonly mediaType?: string;
  readonly url?: string;
  readonly size?: number;
  readonly detail?: string;
  /** Optimistic sends that currently own this not-yet-confirmed resource. */
  readonly pendingSendTokens?: readonly string[];
  /** At least one overlapping send (or engine event) confirmed the resource. */
  readonly confirmed?: true;
}

export interface AgentTranscriptState {
  readonly messages: readonly MessageDto[];
  readonly revision: number;
  readonly nextMessageIndex: number;
  /** Sparse indexes let a late snapshot retain already-arrived live tail rows. */
  readonly messageIndexes: Readonly<Record<string, number>>;
}

export interface RuntimeCenterState {
  readonly agents: Readonly<Record<string, SessionAgentSummaryDto>>;
  /** Live coordinator state is authoritative over transcript snapshots. */
  readonly coordinatorWorkers: Readonly<Record<string, SessionAgentSummaryDto>>;
  readonly transcripts: Readonly<Record<string, AgentTranscriptState>>;
  readonly resources: readonly RuntimeResource[];
  readonly plan: readonly PlanTaskDto[];
  readonly submittedPlan: SubmittedPlan | null;
  readonly submittedPlanState: SubmittedPlanState;
  readonly sections: Readonly<Record<RuntimeCenterSection, boolean>>;
  readonly overviewOpen: boolean;
  readonly tabs: readonly RuntimeCenterItemRef[];
  readonly activeItem: RuntimeCenterItemRef | null;
  readonly inspectorOpen: boolean;
}

export const RUNTIME_CENTER_SECTIONS: readonly RuntimeCenterSection[] = [
  'tasks',
  'agents',
  'todos',
  'resources',
  'plan',
  'review',
];

export function emptyRuntimeCenterState(): RuntimeCenterState {
  return {
    agents: {},
    coordinatorWorkers: {},
    transcripts: {},
    resources: [],
    plan: [],
    submittedPlan: null,
    submittedPlanState: emptySubmittedPlanState(),
    sections: { tasks: true, agents: true, todos: true, resources: true, plan: true, review: true },
    overviewOpen: false,
    tabs: [],
    activeItem: null,
    inspectorOpen: false,
  };
}

export function resetRuntimeCenterData(state: RuntimeCenterState): RuntimeCenterState {
  return {
    ...emptyRuntimeCenterState(),
    sections: state.sections,
    tabs: state.tabs,
    activeItem: state.activeItem,
    inspectorOpen: state.inspectorOpen,
  };
}

export function runtimeCenterItemKey(item: RuntimeCenterItemRef): string {
  return `${item.kind}:${item.id}`;
}

export function planRuntimeItemId(
  task: PlanTaskDto,
  index: number,
  plan: readonly PlanTaskDto[],
): string {
  if (task.id) return `id:${task.id}`;
  const occurrence = plan.slice(0, index).filter((entry) => entry.subject === task.subject).length;
  return `legacy:${encodeURIComponent(task.subject)}:${occurrence}`;
}

export function sameRuntimeCenterItem(
  left: RuntimeCenterItemRef | null | undefined,
  right: RuntimeCenterItemRef | null | undefined,
): boolean {
  return left !== null && left !== undefined && right !== null && right !== undefined
    && left.kind === right.kind && left.id === right.id;
}

export function openRuntimeCenterItem(
  state: RuntimeCenterState,
  item: RuntimeCenterItemRef,
): RuntimeCenterState {
  const key = runtimeCenterItemKey(item);
  const tabs = state.tabs.some((tab) => runtimeCenterItemKey(tab) === key)
    ? state.tabs
    : [...state.tabs, item];
  return { ...state, tabs, activeItem: item, inspectorOpen: true };
}

export function closeRuntimeCenterItem(
  state: RuntimeCenterState,
  item: RuntimeCenterItemRef,
): RuntimeCenterState {
  const key = runtimeCenterItemKey(item);
  const index = state.tabs.findIndex((tab) => runtimeCenterItemKey(tab) === key);
  if (index < 0) return state;
  const tabs = state.tabs.filter((tab) => runtimeCenterItemKey(tab) !== key);
  if (!sameRuntimeCenterItem(state.activeItem, item)) return { ...state, tabs };
  const fallback = tabs[Math.min(index, tabs.length - 1)] ?? null;
  return {
    ...state,
    tabs,
    activeItem: fallback,
    inspectorOpen: state.inspectorOpen,
  };
}

export function toggleRuntimeCenterSection(
  state: RuntimeCenterState,
  section: RuntimeCenterSection,
): RuntimeCenterState {
  return {
    ...state,
    sections: { ...state.sections, [section]: !state.sections[section] },
  };
}

export function setRuntimeCenterOverviewOpen(
  state: RuntimeCenterState,
  open: boolean,
): RuntimeCenterState {
  return state.overviewOpen === open ? state : { ...state, overviewOpen: open };
}

export function setRuntimeInspectorOpen(
  state: RuntimeCenterState,
  open: boolean,
): RuntimeCenterState {
  return state.inspectorOpen === open ? state : { ...state, inspectorOpen: open };
}

function messageKey(index: number): string {
  return String(index);
}

function transcriptFromEntries(
  entries: Readonly<Record<string, MessageDto>>,
  revision: number,
  nextMessageIndex: number,
): AgentTranscriptState {
  const ordered = Object.entries(entries)
    .map(([index, message]) => [Number(index), message] as const)
    .filter(([index]) => Number.isSafeInteger(index) && index >= 0)
    .sort(([left], [right]) => left - right);
  return {
    messages: ordered.map(([, message]) => message),
    revision,
    nextMessageIndex,
    messageIndexes: Object.fromEntries(ordered.map(([index]) => [messageKey(index), index])),
  };
}

function entriesFromTranscript(transcript: AgentTranscriptState): Record<string, MessageDto> {
  const entries: Record<string, MessageDto> = {};
  const indexes = Object.entries(transcript.messageIndexes).sort(([, left], [, right]) => left - right);
  transcript.messages.forEach((message, position) => {
    const mapped = indexes[position]?.[0];
    entries[mapped ?? messageKey(position)] = message;
  });
  return entries;
}

function mergeTranscriptSnapshot(
  previous: AgentTranscriptState | undefined,
  messages: readonly MessageDto[],
  revision: number,
  nextMessageIndex: number,
): AgentTranscriptState {
  if (previous && revision < previous.revision) return previous;
  const incoming: Record<string, MessageDto> = {};
  messages.forEach((message, index) => { incoming[messageKey(index)] = message; });
  // A revision-aware poll can race a live message event. Keep only the live
  // tail beyond the snapshot's watermark; rows covered by the snapshot are
  // replaced authoritatively.
  if (previous) {
    const previousEntries = entriesFromTranscript(previous);
    for (const [index, message] of Object.entries(previousEntries)) {
      const numeric = Number(index);
      if (numeric >= nextMessageIndex && incoming[index] === undefined) incoming[index] = message;
    }
  }
  return transcriptFromEntries(incoming, revision, Math.max(nextMessageIndex, previous?.nextMessageIndex ?? 0));
}

function mergeTranscriptMessage(
  previous: AgentTranscriptState | undefined,
  index: number,
  message: MessageDto,
): AgentTranscriptState {
  if (!Number.isSafeInteger(index) || index < 0) return previous ?? {
    messages: [], revision: 0, nextMessageIndex: 0, messageIndexes: {},
  };
  const entries = previous ? entriesFromTranscript(previous) : {};
  const key = messageKey(index);
  if (entries[key] !== undefined && JSON.stringify(entries[key]) === JSON.stringify(message)) {
    return previous as AgentTranscriptState;
  }
  entries[key] = message;
  return transcriptFromEntries(entries, previous?.revision ?? 0, Math.max(index + 1, previous?.nextMessageIndex ?? 0));
}

export function resourceFromAttachment(attachment: AttachmentDto): RuntimeResource {
  if (attachment.type === 'nested_memory') {
    const path = attachment.display_path;
    return {
      id: `file:${path}`,
      kind: 'file',
      name: path.split(/[\\/]/).filter(Boolean).at(-1) ?? path,
      path,
      detail: 'Workspace attachment',
    };
  }
  return {
    id: `attachment:${JSON.stringify(attachment)}`,
    kind: 'attachment',
    name: 'Attachment',
    detail: 'Engine attachment',
  };
}

export function imageResource(
  id: string,
  name: string,
  mediaType: string,
  url: string,
  pendingSendToken?: string,
): RuntimeResource {
  return {
    id,
    kind: 'image',
    name: name.trim() || 'Attached image',
    mediaType,
    url,
    ...(pendingSendToken ? { pendingSendTokens: [pendingSendToken] } : {}),
  };
}

export function addRuntimeResources(
  state: RuntimeCenterState,
  resources: readonly RuntimeResource[],
): RuntimeCenterState {
  if (resources.length === 0) return state;
  const next = [...state.resources];
  const indexes = new Map(next.map((resource, index) => [resource.id, index]));
  let changed = false;
  for (const resource of resources) {
    const index = indexes.get(resource.id);
    if (index === undefined) {
      indexes.set(resource.id, next.length);
      next.push(resource);
      changed = true;
      continue;
    }
    const existing = next[index]!;
    if (!resource.pendingSendTokens?.length) {
      if (existing.pendingSendTokens?.length) {
        next[index] = { ...existing, ...resource, pendingSendTokens: undefined, confirmed: true };
        changed = true;
      }
      continue;
    }
    if (!existing.pendingSendTokens?.length) continue;
    const tokens = [...new Set([...existing.pendingSendTokens, ...resource.pendingSendTokens])];
    if (tokens.length !== existing.pendingSendTokens.length) {
      next[index] = { ...existing, pendingSendTokens: tokens };
      changed = true;
    }
  }
  return changed ? { ...state, resources: next } : state;
}

export function promptRuntimeResources(
  sessionId: string,
  sendToken: string,
  images: readonly ImageRefDto[],
  imageNames: readonly string[],
  filePaths: readonly string[],
): RuntimeResource[] {
  const imageResources = images.map((image, index) => imageResource(
    `image:${sessionId}:pending:${sendToken}:${index}`,
    imageNames[index] ?? `Attached image ${index + 1}`,
    image.media_type,
    `data:${image.media_type};base64,${image.base64}`,
    sendToken,
  ));
  const fileResources = [...new Set(filePaths)].map((path) => ({
    id: `file:${path}`,
    kind: 'file' as const,
    name: path.split(/[\\/]/).filter(Boolean).at(-1) ?? path,
    path,
    detail: 'Mentioned workspace file',
    pendingSendTokens: [sendToken],
  }));
  return [...imageResources, ...fileResources];
}

export function commitRuntimeResources(
  state: RuntimeCenterState,
  sendToken: string,
): RuntimeCenterState {
  let changed = false;
  const resources = state.resources.map((resource): RuntimeResource => {
    if (!resource.pendingSendTokens?.includes(sendToken)) return resource;
    changed = true;
    const remaining = resource.pendingSendTokens.filter((token) => token !== sendToken);
    return {
      ...resource,
      pendingSendTokens: remaining.length > 0 ? remaining : undefined,
      confirmed: true,
    };
  });
  return changed ? { ...state, resources } : state;
}

export function rollbackRuntimeResources(
  state: RuntimeCenterState,
  sendToken: string,
): RuntimeCenterState {
  let changed = false;
  const resources = state.resources.flatMap((resource): RuntimeResource[] => {
    if (!resource.pendingSendTokens?.includes(sendToken)) return [resource];
    changed = true;
    const remaining = resource.pendingSendTokens.filter((token) => token !== sendToken);
    if (remaining.length > 0) return [{ ...resource, pendingSendTokens: remaining }];
    return resource.confirmed ? [{ ...resource, pendingSendTokens: undefined }] : [];
  });
  return changed ? { ...state, resources } : state;
}

export function resourcesFromRestoredMessages(
  sessionId: string,
  messages: readonly MessageDto[],
): RuntimeResource[] {
  const resources: RuntimeResource[] = [];
  messages.forEach((message, messageIndex) => {
    (message.images ?? []).forEach((image, imageIndex) => {
      resources.push(imageResource(
        `image:${sessionId}:restored:${messageIndex}:${imageIndex}`,
        `Attached image ${imageIndex + 1}`,
        image.media_type,
        image.url,
      ));
    });
    if (message.role !== 'user') return;
    for (const block of message.blocks) {
      if (block.type !== 'text') continue;
      for (const path of fileMentionsFromPrompt(block.text)) {
        resources.push({
          id: `file:${path}`,
          kind: 'file',
          name: path.split(/[\\/]/).filter(Boolean).at(-1) ?? path,
          path,
          detail: 'Mentioned workspace file',
        });
      }
    }
  });
  return resources;
}

const terminalAgentStatuses = new Set(['completed', 'failed', 'killed', 'cancelled']);

function mergeCoordinatorWorker(state: RuntimeCenterState, agent: SessionAgentSummaryDto): SessionAgentSummaryDto {
  const worker = state.coordinatorWorkers[agent.agent_id];
  return worker ? { ...agent, ...worker } : agent;
}

export function reduceRuntimeCenterEvent(
  state: RuntimeCenterState,
  event: ClientEvent,
  sessionId: string,
): RuntimeCenterState {
  const submittedPlanState = reduceSubmittedPlanEvent(state.submittedPlanState, event, sessionId);
  if (submittedPlanState !== state.submittedPlanState) {
    state = { ...state, submittedPlanState, submittedPlan: latestSubmittedPlan(submittedPlanState) };
  }
  switch (event.type) {
    case 'coordinator_worker': {
      const worker = { ...event.worker, status: event.worker.status === 'working' ? 'running' : event.worker.status };
      return {
        ...state,
        coordinatorWorkers: { ...state.coordinatorWorkers, [worker.agent_id]: worker },
        agents: { ...state.agents, [worker.agent_id]: { ...state.agents[worker.agent_id], ...worker } },
      };
    }
    case 'session_agent_list': {
      if (event.session_id !== sessionId) return state;
      const agents = Object.fromEntries(event.agents.map((agent) => [agent.agent_id, mergeCoordinatorWorker(state, agent)]));
      for (const worker of Object.values(state.coordinatorWorkers)) {
        if (!agents[worker.agent_id] && !terminalAgentStatuses.has(worker.status)) {
          agents[worker.agent_id] = { ...state.agents[worker.agent_id], ...worker };
        }
      }
      return { ...state, agents };
    }
    case 'session_agent_updated':
      if (event.session_id !== sessionId) return state;
      return { ...state, agents: { ...state.agents, [event.agent.agent_id]: mergeCoordinatorWorker(state, event.agent) } };
    case 'session_agent_message': {
      if (event.session_id !== sessionId) return state;
      return {
        ...state,
        transcripts: {
          ...state.transcripts,
          [event.agent_id]: mergeTranscriptMessage(state.transcripts[event.agent_id], event.message_index, event.message),
        },
      };
    }
    case 'session_agent_transcript':
      if (event.session_id !== sessionId) return state;
      return {
        ...state,
        transcripts: {
          ...state.transcripts,
          [event.agent_id]: mergeTranscriptSnapshot(
            state.transcripts[event.agent_id],
            event.messages,
            event.revision,
            event.next_message_index,
          ),
        },
      };
    case 'attachment':
      return addRuntimeResources(state, [resourceFromAttachment(event.attachment)]);
    case 'plan_updated':
      return { ...state, plan: event.tasks };
    case 'session_resumed':
      return event.session_id === sessionId
        ? { ...state, agents: {}, coordinatorWorkers: {} }
        : state;
    case 'session_started':
      return event.session_id === sessionId ? resetRuntimeCenterData(state) : state;
    case 'session_ended':
      return emptyRuntimeCenterState();
    default:
      return state;
  }
}

export function tasksToRuntimeItems(tasks: readonly TaskRowDto[]): RuntimeCenterItemRef[] {
  return tasks.map((task) => ({ kind: 'task', id: task.task_id }));
}

export function reduceRuntimeCenterPermission(
  state: RuntimeCenterState, request: PermissionRequest, sessionId: string,
): RuntimeCenterState {
  const submittedPlanState = reduceSubmittedPlanPermission(state.submittedPlanState, request, sessionId);
  return submittedPlanState === state.submittedPlanState ? state : {
    ...state, submittedPlanState, submittedPlan: latestSubmittedPlan(submittedPlanState),
  };
}

/** A respawn replaces live workers and the permission gate; history remains inspectable. */
export function resetRuntimeCenterConnection(state: RuntimeCenterState): RuntimeCenterState {
  const submittedPlanState: SubmittedPlanState = {
    ...emptySubmittedPlanState(),
    calls: state.submittedPlanState.calls.map(({ requestId: _requestId, ...call }) => ({
      ...call,
      ...(!call.finished && (call.status === 'pending' || call.status === 'submitted')
        ? { status: 'failed' as const, finished: true }
        : {}),
    })),
  };
  return {
    ...state,
    overviewOpen: false,
    agents: {},
    coordinatorWorkers: {},
    submittedPlanState,
    submittedPlan: latestSubmittedPlan(submittedPlanState),
  };
}
