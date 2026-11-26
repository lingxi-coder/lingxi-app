import { executionCommand, type ExecutionScope, type HeldReply, type HeldReplyDecision, type TaskState, type TeamDocument } from './execution.js';
import type { TeamsApi } from './index.js';

/**
 * The replies of a Bot that wait for its maintainer to let them go (Bot v3, B6). The service keeps a held reply where only the Bot's maintainer can read it,
 * so these three reads and writes are the one way a client ever sees one; every other member sees no more than that the task awaits approval. A reply lives ten
 * minutes: it is posted by an approval, and dropped by a rejection, by the time running out, and, when nobody asked the Bot, by a newer message of the Chat.
 */

const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const word = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
const moment = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= 8.64e15;
const generation = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

function teamDocument(value: unknown): TeamDocument | undefined {
  return record(value) && word(value.title) && typeof value.body === 'string' ? { title: value.title, body: value.body } : undefined;
}

/** One held reply as the service sent it, or nothing when this version cannot read it: a reply is never half shown, and one that is not understood does not hide the others. */
export function heldReply(value: unknown): HeldReply | undefined {
  if (!record(value) || !record(value.requester)) return undefined;
  const { requester } = value, shown = value.document === undefined ? undefined : teamDocument(value.document);
  if (!word(value.taskId) || !word(value.conversationId) || !word(value.messageId) || !word(requester.uid) || !generation(requester.generation)) return undefined;
  if (typeof value.unasked !== 'boolean' || !moment(value.heldAt) || !moment(value.expiresAt) || !word(value.text)) return undefined;
  // A document that cannot be read is no reply to approve: the maintainer would be letting go of something they did not see.
  if (value.document !== undefined && !shown) return undefined;
  return {
    taskId: value.taskId, conversationId: value.conversationId, messageId: value.messageId, requester: { uid: requester.uid, generation: requester.generation },
    unasked: value.unasked, heldAt: value.heldAt, expiresAt: value.expiresAt, text: value.text,
    ...(shown ? { document: shown } : {}), ...(word(value.conversationTitle) ? { conversationTitle: value.conversationTitle } : {}),
  };
}
/** The answer to `listHeldReplies`, newest first. An answer that is not a list at all is `INVALID_DATA`, never an empty one. */
export function heldReplies(value: unknown): HeldReply[] {
  if (!record(value) || !Array.isArray(value.replies)) throw new Error('INVALID_DATA');
  return value.replies.map(heldReply).filter((reply): reply is HeldReply => reply !== undefined);
}
/** What a decision came to; an answer that says nothing of the task is `INVALID_DATA`. */
export function heldReplyDecision(value: unknown): HeldReplyDecision {
  if (!record(value) || !word(value.taskId) || !word(value.state)) throw new Error('INVALID_DATA');
  const { declineReason, replyId, artifactId } = value;
  return {
    taskId: value.taskId, state: value.state as TaskState,
    ...(declineReason === 'approval_rejected' || declineReason === 'revoked' ? { declineReason } : {}),
    ...(word(replyId) || replyId === null ? { replyId } : {}), ...(word(artifactId) || artifactId === null ? { artifactId } : {}),
  };
}

/** The replies of one Bot that wait for this person, optionally in one Chat. `MAINTAINER_REQUIRED` when they do not maintain the Bot. */
export async function readHeldReplies(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, request: { sharedBotId: string; conversationId?: string }): Promise<HeldReply[]> {
  return heldReplies(await api.messages<unknown>(executionCommand(scope, 'listHeldReplies', {
    sharedBotId: request.sharedBotId, ...(request.conversationId ? { conversationId: request.conversationId } : {}),
  })));
}
/**
 * Let a held reply go: as the Bot made it, or with `text` as the maintainer changed it to. `commandId` is that of the first attempt when this one retries it, so
 * that an approval whose answer was lost is answered again and not decided twice. A reply that is no longer held is refused with `REPLY_APPROVED`, `REPLY_REJECTED`,
 * `REPLY_EXPIRED` or `REPLY_SUPERSEDED`, for whichever came first.
 */
export async function approveHeldReply(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, taskId: string, options: { text?: string; commandId?: string } = {}): Promise<HeldReplyDecision> {
  return heldReplyDecision(await api.messages<unknown>(executionCommand(scope, 'approveReply', { taskId, ...(options.text === undefined ? {} : { text: options.text }) }, options.commandId)));
}
/** Drop a held reply: nothing is posted. Refused like an approval when the reply is no longer held. */
export async function rejectHeldReply(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, taskId: string, options: { commandId?: string } = {}): Promise<HeldReplyDecision> {
  return heldReplyDecision(await api.messages<unknown>(executionCommand(scope, 'rejectReply', { taskId }, options.commandId)));
}
