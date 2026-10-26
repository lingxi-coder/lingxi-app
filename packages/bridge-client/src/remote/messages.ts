import type { FileOffer, FileReceipt, FileCheckpoint } from './files.js';
/**
 * Parameter and result types of the `lingxi-rc/1` RPC methods, and the
 * `RemoteRecord` union that carries session content. Plain types only: the
 * Electron session layer (P3b) and the viewers fill in the semantics.
 *
 * Shapes the plan does not fix are marked `TBD`; they are the smallest thing
 * that works and may change before P3b lands. Wherever the plan's table names
 * a field, the name here is the same (snake_case, like `ClientEvent`).
 * `ClientEvent` and the DTOs it uses come from `protocol.ts` unchanged.
 */

import type {
  AskUserQuestionRequestDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ConversationControlsDto,
  ImageRefDto,
  MessageDto,
  ModelDetailsDto,
  PermissionModeId,
  PermissionRequest,
  PermissionResponseDto,
  ReasoningSelectionDto,
  SessionModeDto,
} from '../protocol.js';
import type { DevicePlatform } from './signaling.js';

/** A result with nothing in it; the confirmation that a command was accepted. */
export type EmptyResult = Record<string, never>;

// ─────────────────────────────────────────────────────────────────────────────
// Handshake
// ─────────────────────────────────────────────────────────────────────────────

/** Name and build of the viewer app, sent in `initialize`. */
export interface RemoteAppInfo {
  version: string;
  platform: DevicePlatform;
}

/** The desktop, as described to an approved viewer. */
export interface RemoteHostInfo {
  name: string;
  /** Operating-system family; the same value as `HostRecord.platform`. */
  platform: string;
  app_version: string;
}

/** `initialize`: the first request on every connection. */
export interface InitializeParams {
  /** `REMOTE_PROTOCOL_MAJOR`; a different major is answered with `upgrade_required`. */
  protocol: number;
  /** `CLIENT_PROTOCOL_MAJOR` of the viewer's `ClientEvent` decoder. */
  client_protocol_version: number;
  app: RemoteAppInfo;
  /** Push registration of this install, so the host can address and exclude it. */
  push_installation_id?: string;
}

/**
 * Answer to `initialize`. A device the desktop has not approved gets
 * `pending_approval`, and until it is approved every method except
 * `initialize` and `ping` fails with `not_approved`.
 */
export type InitializeResult =
  | { status: 'approved'; host: RemoteHostInfo }
  | { status: 'pending_approval'; pairing_code: string };

/** `auth.approved` (host to viewer): the pending device was approved. */
export interface AuthApprovedParams {
  /** TBD: repeats `InitializeResult.host` so the viewer need not keep the first answer. */
  host: RemoteHostInfo;
}

/** `auth.denied` (host to viewer): the user said no; the host closes the connection after it. */
export interface AuthDeniedParams {
  /** TBD: the plan names the field but not its values (e.g. `denied`, `rate_limited`). */
  reason: string;
}

/**
 * `ping` / `pong` (notifications, either direction): liveness for idle links; any message counts. `RpcPeer`
 * answers a `ping` with a bare `pong` itself and passes neither on, so applications never handle them.
 */
export interface PingParams {
  /** TBD: unused in v1; reserved so a `pong` could carry the sender's clock back for a round-trip time. */
  ts?: number;
}
export type PongParams = PingParams;

// ─────────────────────────────────────────────────────────────────────────────
// Session list
// ─────────────────────────────────────────────────────────────────────────────

/** Where a remote-enabled session stands. */
export type RemoteSessionStatus = 'running' | 'requires_action' | 'idle' | 'dormant';

/** What a session is waiting on while `requires_action`. TBD: the plan names the field, not the values. */
export type RemotePendingAction = 'permission' | 'question' | 'computer_access';

/** One remote-enabled session in `sessions.list` / `sessions.changed`. */
export interface RemoteSessionSummary {
  session_id: string;
  title: string;
  /** Basename of the project directory. */
  project_label: string;
  status: RemoteSessionStatus;
  pending_action?: RemotePendingAction;
  model?: string;
  /** Number of viewers attached to the session right now. */
  viewers: number;
}

/** `sessions.list` (request). */
export type SessionsListParams = Record<string, never>;
export interface SessionsListResult {
  sessions: RemoteSessionSummary[];
}

/** `sessions.watch` (request): the answer is the current list; later changes arrive as `sessions.changed`. */
export type SessionsWatchParams = Record<string, never>;
export type SessionsWatchResult = SessionsListResult;

/** `sessions.changed` (host to viewer). TBD: always the full list; a delta form is possible if lists grow. */
export interface SessionsChangedParams {
  sessions: RemoteSessionSummary[];
}

// ─────────────────────────────────────────────────────────────────────────────
// Session content
// ─────────────────────────────────────────────────────────────────────────────

/** A position in a session's record stream: `epoch` identifies the base, `seq` counts records after it. */
export interface RemoteCursor {
  /** `runtimeId` plus the sequence of the base event; a new base (e.g. `/clear`) is a new epoch. */
  epoch: string;
  seq: number;
}

/** An image the host keeps and the viewer fetches with `image.get`. */
export interface RemoteImageRef {
  ref: string;
  media_type: string;
}

/** Who started a prompt. */
export type RemotePromptOrigin = 'desktop' | 'device' | 'scheduled';

/**
 * One entry of a session's record stream. Prompts are records of their own
 * because the engine's event stream does not contain the user's prompts.
 */
export type RemoteRecord =
  /** An engine event, passed through the host's remote filter. `truncated`: large fields were cut. */
  | { type: 'event'; event: ClientEvent; truncated?: boolean }
  /** A user prompt, from the host's prompt journal. */
  | {
      type: 'prompt';
      turn_id: number;
      text: string;
      images?: RemoteImageRef[];
      origin: RemotePromptOrigin;
      /** TBD: name of the sending device when `origin` is `device` ("from iPhone"). */
      device_name?: string;
    }
  | { type: 'permission_request'; request: PermissionRequest }
  /** Shown only; computer access can be granted on the desktop alone. */
  | { type: 'computer_access_request'; request: ComputerAccessRequestDto }
  /** An event too large to send (over 1 MiB). TBD: what is worth saying about it. */
  | { type: 'omitted'; event_type?: string; size_bytes?: number };

/** A record with its position in the stream. */
export interface RemoteSeqRecord {
  seq: number;
  record: RemoteRecord;
}

/** Everything the session is waiting for an answer to; always the complete set. */
export interface RemotePending {
  permissions: PermissionRequest[];
  questions: AskUserQuestionRequestDto[];
  /** Display only. */
  computer_access: ComputerAccessRequestDto[];
}

/** Whether a turn is running. TBD: the plan says `end` carries `turn` without fixing its shape. */
export interface RemoteTurnState {
  running: boolean;
  turn_id?: number;
}

/** `session.attach` (request). */
export interface SessionAttachParams {
  session_id: string;
  /** What the viewer already has; lets the host answer with a delta instead of a snapshot. */
  resume?: { epoch: string; after_seq: number };
}

/**
 * Answer to `session.attach`. `delta`: the viewer's state is still valid;
 * records after `resume.after_seq` follow as `session.event`, and `pending` is
 * the full pending set. `snapshot`: `session.snapshot` notifications follow.
 */
export type SessionAttachResult =
  | { mode: 'delta'; epoch: string; pending: RemotePending }
  | { mode: 'snapshot'; epoch: string };

/**
 * `session.snapshot` (host to viewer): a snapshot is `begin`, then `messages`
 * pages, then `records` pages, then `end`, all in order on the one connection.
 */
export type SessionSnapshotParams =
  | {
      phase: 'begin';
      session_id: string;
      epoch: string;
      session_mode?: SessionModeDto;
      /** TBD: index of the first message sent; above 0 when older history must be paged with `session.history`. */
      first_message_index: number;
    }
  /** The base conversation (`session_resumed.messages`), a page at a time. */
  | { phase: 'messages'; session_id: string; messages: MessageDto[] }
  /** Records after the base, a page at a time. */
  | { phase: 'records'; session_id: string; records: RemoteSeqRecord[] }
  | {
      phase: 'end';
      session_id: string;
      cursor: RemoteCursor;
      pending: RemotePending;
      turn: RemoteTurnState;
      /** Absent while the session has no controls yet. */
      controls?: ConversationControlsDto;
    };

/** `session.event` (host to viewer): the next record of a session the viewer is attached to. */
export interface SessionEventParams {
  session_id: string;
  seq: number;
  record: RemoteRecord;
}

/** `session.pending` (host to viewer): the pending set changed (replaces the previous one). */
export interface SessionPendingParams {
  session_id: string;
  pending: RemotePending;
}

/** `session.reset` (host to viewer): the base changed (e.g. `/clear`); a fresh snapshot follows. */
export interface SessionResetParams {
  session_id: string;
}

/** `session.closed` (host to viewer): the session is no longer remote-enabled or was ended. */
export interface SessionClosedParams {
  session_id: string;
  /** TBD: values not fixed by the plan. */
  reason?: string;
}

/** `session.detach` (request). */
export interface SessionDetachParams {
  session_id: string;
}

/** `session.presence` (notification, viewer to host): whether this viewer is looking at the session. */
export interface SessionPresenceParams {
  session_id: string;
  foreground: boolean;
}

/** `session.history` (request): older messages than the snapshot included. */
export interface SessionHistoryParams {
  session_id: string;
  /** Index into the session's base messages; the page ends just before it. */
  before_index: number;
  limit: number;
}
export interface SessionHistoryResult {
  messages: MessageDto[];
  /** Index of `messages[0]` in the base conversation. */
  first_index: number;
  /** TBD: whether anything older remains. */
  has_more: boolean;
}

/** `image.get` (request): fetch an image by the ref a record carries. */
export interface ImageGetParams {
  ref: string;
}
export interface ImageGetResult {
  media_type: string;
  base64: string;
}

// ─────────────────────────────────────────────────────────────────────────────
// Commands
// ─────────────────────────────────────────────────────────────────────────────
//
// TBD: the plan's table names `session_id` only on some of these; every
// session-scoped command carries it here, since request ids are per session.

/** `prompt.send` (request). While a turn runs the host takes plain text only. */
export interface PromptSendParams {
  file_refs?: string[];
  session_id: string;
  text: string;
  /** At most 4, each already shrunk to 2 MiB or less by the viewer. */
  images?: ImageRefDto[];
  /** Chosen by the viewer; makes a retry after a reconnect recognizable. */
  client_request_id: string;
}
export interface PromptSendResult {
  /** Assigned by the host (a random 53-bit integer). */
  turn_id: number;
}

/** `turn.cancel` (request). */
export interface TurnCancelParams {
  session_id: string;
  turn_id?: number;
}

/** `permission.respond` (request). First answer wins; a late one fails with `not_pending`. */
export interface PermissionRespondParams {
  session_id: string;
  request_id: number;
  /** The host refuses `allow_always` unless enabled, and never switches to `bypassPermissions`. */
  decision: PermissionResponseDto;
}

/** `question.answer` (request): `answers` maps question text to the chosen label(s), as in `answer_ask_user_question`. */
export interface QuestionAnswerParams {
  session_id: string;
  request_id: number;
  answers: Record<string, string>;
}

/** `question.cancel` (request). */
export interface QuestionCancelParams {
  session_id: string;
  request_id: number;
}

/** `controls.set_model` (request). */
export interface ControlsSetModelParams {
  session_id: string;
  model: string;
}

/** `controls.set_reasoning` (request): reasoning/effort selection. */
export interface ControlsSetReasoningParams {
  session_id: string;
  selection: ReasoningSelectionDto;
}

/** `controls.set_permission_mode` (request); `bypassPermissions` is refused with `forbidden`. */
export interface ControlsSetPermissionModeParams {
  session_id: string;
  mode: PermissionModeId;
}

/** `models.list` (request). */
export interface ModelsListParams {
  /** TBD: the session whose available models are wanted, if they differ per session. */
  session_id?: string;
}
export interface ModelsListResult {
  /** Same shape as the engine's `model_list` event. */
  models: string[];
  current: string;
  details?: ModelDetailsDto[];
}

// ─────────────────────────────────────────────────────────────────────────────
// Method tables
// ─────────────────────────────────────────────────────────────────────────────

/** Requests a viewer sends to the host: method name to its params and result. */
export interface ViewerRequests {
  'file.offer': { params: FileOffer; result: FileCheckpoint };
  'file.commit': { params: { session_id: string; transfer_id: string }; result: FileReceipt };
  'file.cancel': { params: { session_id: string; transfer_id: string }; result: EmptyResult };
  'file.list': { params: { session_id: string }; result: { files: FileReceipt[] } };
  'file.download': { params: { session_id: string; transfer_id: string; offset: number }; result: FileReceipt };
  initialize: { params: InitializeParams; result: InitializeResult };
  'sessions.list': { params: SessionsListParams; result: SessionsListResult };
  'sessions.watch': { params: SessionsWatchParams; result: SessionsWatchResult };
  'session.attach': { params: SessionAttachParams; result: SessionAttachResult };
  'session.detach': { params: SessionDetachParams; result: EmptyResult };
  'session.history': { params: SessionHistoryParams; result: SessionHistoryResult };
  'image.get': { params: ImageGetParams; result: ImageGetResult };
  'prompt.send': { params: PromptSendParams; result: PromptSendResult };
  'turn.cancel': { params: TurnCancelParams; result: EmptyResult };
  'permission.respond': { params: PermissionRespondParams; result: EmptyResult };
  'question.answer': { params: QuestionAnswerParams; result: EmptyResult };
  'question.cancel': { params: QuestionCancelParams; result: EmptyResult };
  'controls.set_model': { params: ControlsSetModelParams; result: EmptyResult };
  'controls.set_reasoning': { params: ControlsSetReasoningParams; result: EmptyResult };
  'controls.set_permission_mode': { params: ControlsSetPermissionModeParams; result: EmptyResult };
  'models.list': { params: ModelsListParams; result: ModelsListResult };
}

/** Notifications a viewer sends to the host: method name to params. */
export interface ViewerNotifications {
  ping: PingParams;
  pong: PongParams;
  'session.presence': SessionPresenceParams;
}

/** Notifications the host sends to a viewer: method name to params. */
export interface HostNotifications {
  'auth.approved': AuthApprovedParams;
  'auth.denied': AuthDeniedParams;
  ping: PingParams;
  pong: PongParams;
  'sessions.changed': SessionsChangedParams;
  'session.snapshot': SessionSnapshotParams;
  'session.event': SessionEventParams;
  'session.pending': SessionPendingParams;
  'session.reset': SessionResetParams;
  'session.closed': SessionClosedParams;
}
