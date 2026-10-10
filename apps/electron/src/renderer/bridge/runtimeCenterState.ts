import type { TurnFileChange } from '../model/runItem';
import type {
  AttachmentDto,
  ClientEvent,
  ImageRefDto,
  MessageDto,
  PlanTaskDto,
  SessionAgentMessageRowDto,
  PermissionRequest,
  SessionAgentSummaryDto,
  TaskRowDto,
} from '@lingxi/bridge-client';
import { fileMentionsFromPrompt } from './fileMentions';
import { emptySubmittedPlanState, latestSubmittedPlan, reduceSubmittedPlanEvent, reduceSubmittedPlanPermission, type SubmittedPlan, type SubmittedPlanState } from './submittedPlan';

export type RuntimeCenterSection = 'tasks' | 'agents' | 'todos' | 'resources' | 'plan' | 'review';

export type RuntimeCenterItemRef =
  | { kind: 'turn-review'; id: string; files: readonly TurnFileChange[]; path?: string }
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
  /** Persisted rows retain Native UUID/error metadata for exact deletion. */
  readonly rows: readonly SessionAgentMessageRowDto[];
  readonly messages: readonly MessageDto[];
  readonly revision: number;
  readonly nextMessageIndex: number;
  /** Tombstones outlive stale snapshots and late events for this agent. */
  readonly tombstonedMessageUuids: Readonly<Record<string, true>>;
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

/**
 * Active children belonging to this session, excluding the main turn and parked
 * agents.
 *
 * `pending` belongs here: it is the engine's own vocabulary (`client-protocol`'s
 * agent listing is `running`/`pending`/`completed`/`failed`/`killed`/`cancelled`/
 * `unknown`) and it is what a just-spawned subagent reports before its first
 * turn. Leaving it out made the overview count the agent as running while
 * `canStop` and `cancel()` saw an empty list — so Escape was a silent no-op for
 * exactly the window in which a user is most likely to press it.
 *
 * `idle` stays out on purpose: a parked agent is not doing work, and stopping it
 * is not what Escape means.
 *
 * The list is exactly that vocabulary's live half. A coordinator worker's
 * `working` never reaches here as itself — `coordinator_worker` normalises it to
 * `running` on the way in, and `mergeCoordinatorWorker` spreads the normalised
 * worker over every roster entry — so matching on it here would be matching a
 * value nothing stores, and `in_progress` is not in the vocabulary at all.
 */
export function runningSubagentIds(state: Pick<RuntimeCenterState, 'agents'> | undefined): string[] {
  return Object.values(state?.agents ?? {})
    .filter((agent) => agent.agent_id !== 'main' && agent.agent_type !== 'side_question' && ['running', 'pending'].includes(agent.status))
    .map((agent) => agent.agent_id);
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
    ? state.tabs.map(tab => item.kind === 'turn-review' && runtimeCenterItemKey(tab) === key ? item : tab)
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

function hasOwn(record: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(record, key);
}

function emptyAgentTranscript(): AgentTranscriptState {
  return { rows: [], messages: [], revision: 0, nextMessageIndex: 0, tombstonedMessageUuids: {} };
}

function transcriptFromEntries(
  entries: Readonly<Record<string, SessionAgentMessageRowDto>>,
  revision: number,
  nextMessageIndex: number,
  tombstonedMessageUuids: Readonly<Record<string, true>>,
): AgentTranscriptState {
  const ordered = Object.entries(entries)
    .map(([, row]) => [row.message_index, row] as const)
    .filter(([index, row]) => Number.isSafeInteger(index) && index >= 0
      && !hasOwn(tombstonedMessageUuids, row.message_uuid))
    .sort(([left], [right]) => left - right);
  const rows = ordered.map(([, row]) => row);
  return {
    rows,
    messages: rows.map((row) => row.message),
    revision,
    nextMessageIndex,
    tombstonedMessageUuids,
  };
}

function entriesFromTranscript(transcript: AgentTranscriptState): Record<string, SessionAgentMessageRowDto> {
  return Object.fromEntries(transcript.rows.map((row) => [messageKey(row.message_index), row]));
}

function mergeTranscriptSnapshot(
  previous: AgentTranscriptState | undefined,
  messages: readonly SessionAgentMessageRowDto[],
  revision: number,
  nextMessageIndex: number,
): AgentTranscriptState {
  if (previous && revision < previous.revision) return previous;
  const tombstones = previous?.tombstonedMessageUuids ?? {};
  const incoming: Record<string, SessionAgentMessageRowDto> = {};
  let observedNextMessageIndex = nextMessageIndex;
  messages.forEach((row) => {
    observedNextMessageIndex = Math.max(
      observedNextMessageIndex,
      Math.min(Number.MAX_SAFE_INTEGER, row.message_index + 1),
    );
    if (!hasOwn(tombstones, row.message_uuid)) incoming[messageKey(row.message_index)] = row;
  });
  // A revision-aware poll can race a live message event. Keep only the live
  // tail beyond the snapshot's watermark; rows covered by the snapshot are
  // replaced authoritatively.
  if (previous) {
    const previousEntries = entriesFromTranscript(previous);
    for (const [index, message] of Object.entries(previousEntries)) {
      const numeric = Number(index);
      if (numeric >= nextMessageIndex && incoming[index] === undefined
        && !hasOwn(tombstones, message.message_uuid)) incoming[index] = message;
    }
  }
  return transcriptFromEntries(
    incoming,
    revision,
    Math.max(observedNextMessageIndex, previous?.nextMessageIndex ?? 0),
    tombstones,
  );
}

function mergeTranscriptMessage(
  previous: AgentTranscriptState | undefined,
  index: number,
  row: SessionAgentMessageRowDto,
): AgentTranscriptState {
  if (!Number.isSafeInteger(index) || index < 0 || row.message_index !== index) return previous ?? emptyAgentTranscript();
  const tombstones = previous?.tombstonedMessageUuids ?? {};
  if (hasOwn(tombstones, row.message_uuid)) return previous ?? emptyAgentTranscript();
  const entries = previous ? entriesFromTranscript(previous) : {};
  const key = messageKey(index);
  if (entries[key] !== undefined && JSON.stringify(entries[key]) === JSON.stringify(row)) {
    return previous as AgentTranscriptState;
  }
  for (const [existingIndex, existingRow] of Object.entries(entries)) {
    if (existingIndex !== key && existingRow.message_uuid === row.message_uuid) delete entries[existingIndex];
  }
  entries[key] = row;
  return transcriptFromEntries(
    entries,
    previous?.revision ?? 0,
    Math.max(index + 1, previous?.nextMessageIndex ?? 0),
    tombstones,
  );
}

function removeTranscriptMessage(
  previous: AgentTranscriptState | undefined,
  messageUuid: string,
): AgentTranscriptState {
  const transcript = previous ?? emptyAgentTranscript();
  if (hasOwn(transcript.tombstonedMessageUuids, messageUuid)
    && !transcript.rows.some((row) => row.message_uuid === messageUuid)) return transcript;
  const entries = entriesFromTranscript(transcript);
  for (const [index, row] of Object.entries(entries)) {
    if (row.message_uuid === messageUuid) delete entries[index];
  }
  const tombstonedMessageUuids = { ...transcript.tombstonedMessageUuids, [messageUuid]: true as const };
  return transcriptFromEntries(entries, transcript.revision, transcript.nextMessageIndex, tombstonedMessageUuids);
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
      // /btw uses the existing slash handler, so its inspector transcript is local.
      for (const agent of Object.values(state.agents)) {
        if (agent.agent_type === 'side_question') agents[agent.agent_id] = agent;
      }
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
          [event.agent_id]: mergeTranscriptMessage(state.transcripts[event.agent_id], event.message_index, {
            message_index: event.message_index,
            message_uuid: event.message_uuid,
            message: event.message,
            ...(event.api_error_json === undefined ? {} : { api_error_json: event.api_error_json }),
          }),
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
    case 'session_agent_tombstone': {
      if (event.session_id !== sessionId) return state;
      return {
        ...state,
        transcripts: {
          ...state.transcripts,
          [event.agent_id]: removeTranscriptMessage(state.transcripts[event.agent_id], event.message_uuid),
        },
      };
    }
    case 'attachment':
      return addRuntimeResources(state, [resourceFromAttachment(event.attachment)]);
    case 'plan_updated':
      return { ...state, plan: event.tasks };
    case 'session_resumed':
      return event.session_id === sessionId
        ? { ...state, agents: interruptedSideQuestionAgents(state), coordinatorWorkers: {} }
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
    agents: interruptedSideQuestionAgents(state),
    coordinatorWorkers: {},
    submittedPlanState,
    submittedPlan: latestSubmittedPlan(submittedPlanState),
  };
}

export function interruptedSideQuestionAgents(state: RuntimeCenterState): RuntimeCenterState['agents'] {
  return Object.fromEntries(Object.values(state.agents)
    .filter((agent) => agent.agent_type === 'side_question')
    .map((agent) => [agent.agent_id, agent.status === 'running'
      ? { ...agent, status: 'failed', latest_activity: 'Connection lost while answering the side question.' }
      : agent]));
}
