import { isMentionEntity, MAX_MEMBER_MENTIONS } from './mentions.js';
import type { MemberExecutionCommand, SharedListWrite } from './execution.js';
import { BOT_SCOPE_ENTRIES, type BotScopeNarrowing, type PublishedBotScope } from './bot-scope.js';

type Check = (value: unknown) => boolean;
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
const id: Check = v => typeof v === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(v);
const integer: Check = v => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
const positive: Check = v => integer(v) && (v as number) > 0;
const uid: Check = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(v);
const optional = (check: Check): Check => v => v === undefined || check(v);
const oneOf = (...values: string[]): Check => v => typeof v === 'string' && values.includes(v);
const bytes = (v: string) => new TextEncoder().encode(v).length;
const list = (check: Check, min: number, max: number): Check => v => Array.isArray(v) && v.length >= min && v.length <= max && new Set(v).size === v.length && v.every(check);
function shape(fields: Record<string, Check>): Check {
  return v => record(v) && Object.keys(v).every(key => Object.prototype.hasOwnProperty.call(fields, key)) && Object.entries(fields).every(([key, check]) => check(v[key]));
}
/** The largest shared document body the service stores, in UTF-8 bytes. */
export const DOCUMENT_BYTES = 256 * 1024;
/** The longest text of one shared-list item, in UTF-8 bytes, and the most changes one write carries. */
export const LIST_ITEM_BYTES = 500, LIST_CHANGES = 20;
/** The most rows one page of a Bot's auto-reply log has. */
export const TRIAGE_LOG_LIMIT = 50;
const itemText: Check = v => typeof v === 'string' && v.trim().length > 0 && bytes(v) <= LIST_ITEM_BYTES && !v.includes('\0');
const listChange: Check = v => record(v) && (v.op === 'add' ? shape({ op: oneOf('add'), text: itemText })(v)
  : v.op === 'update' ? shape({ op: oneOf('update'), itemId: id, text: optional(itemText), done: optional(d => typeof d === 'boolean') })(v) && (v.text !== undefined || v.done !== undefined)
  : shape({ op: oneOf('remove'), itemId: id })(v));
const listWrite = { expectedDataRevision: integer, changes: (v: unknown) => Array.isArray(v) && v.length >= 1 && v.length <= LIST_CHANGES && v.every(listChange) };
/** One write to a shared list exactly as the service accepts it, from a member or from a Bot's run. */
export const isSharedListWrite = (value: unknown): value is SharedListWrite => shape(listWrite)(value) && record(value) && Object.keys(value).length === 2;
const bool: Check = v => typeof v === 'boolean';
// A Local App id, a dataset id and the source it is read through, as the service accepts them. A dataset appears once: a grant names it by its id alone.
const appId: Check = v => typeof v === 'string' && /^[A-Za-z0-9._:-]{1,128}$/u.test(v);
const dataId: Check = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,64}$/u.test(v);
const scopeName: Check = v => typeof v === 'string' && v.trim().length >= 1 && v.trim().length <= 100 && !/[\u0000-\u001f\u007f]/u.test(v.trim());
const distinct = (key: string, check: Check): Check => v => Array.isArray(v) && v.length <= BOT_SCOPE_ENTRIES && v.every(check) && new Set(v.map(row => (row as Record<string, unknown>)[key])).size === v.length;
/** What a Bot is published with of its maintainer's Desktop, exactly as the service accepts it. */
export const isPublishedBotScope = (value: unknown): value is PublishedBotScope => shape({
  localApps: distinct('id', shape({ id: appId, name: scopeName })),
  datasets: distinct('dataset', shape({ source: dataId, dataset: dataId, write: bool, reportReview: bool })) })(value);
/** A Chat's narrowing of one Bot: a list that is left out does not narrow that kind. */
export const isBotScopeNarrowing = (value: unknown): value is BotScopeNarrowing => shape({
  localApps: optional(list(appId, 0, BOT_SCOPE_ENTRIES)),
  datasets: optional(distinct('dataset', shape({ source: dataId, dataset: dataId }))) })(value);
const sha256: Check = v => typeof v === 'string' && /^[a-f0-9]{64}$/u.test(v);
/** A 10 MiB file is fifteen parts of 700 KiB. */
const partIndex: Check = v => integer(v) && (v as number) <= 14;
const payloads: Record<string, Check> = {
  listSharedBots: shape({}),
  createConversation: shape({ conversationId: id, kind: v => ['bot_dm', 'group', 'channel'].includes(String(v)), title: v => typeof v === 'string' && v.trim().length >= 1 && v.trim().length <= 120, participantUids: list(uid, 1, 200), sharedBotIds: list(id, 1, 8) }),
  // Text for Bots, files, or both: a message with neither is an ordinary message and is not a command.
  submit: v => shape({ conversationId: id, messageId: id, changeId: id, text: t => typeof t === 'string' && bytes(t) <= 8192, targets: list(id, 0, 8), replyTo: optional(id), attachments: optional(list(id, 1, 5)), mentionEntities: optional(list(isMentionEntity, 0, MAX_MEMBER_MENTIONS)), privateDataSource: optional(t => typeof t === 'string' && /^[A-Za-z0-9_-]{1,64}$/u.test(t)) })(v)
    && record(v) && (v.attachments !== undefined || ((v.targets as unknown[]).length >= 1 && (v.text as string).length >= 1)),
  createUpload: shape({ conversationId: id, uploadId: id, name: v => typeof v === 'string' && v.length >= 1 && v.length <= 120 && v === v.trim() && !/[/\\\u0000-\u001f\u007f]/u.test(v) && v !== '.' && v !== '..',
    mime: oneOf('image/png', 'image/jpeg', 'application/pdf', 'text/plain'), byteLength: v => positive(v) && (v as number) <= 10 * 1024 * 1024, sha256 }),
  putUploadPart: shape({ uploadId: id, index: partIndex, data: v => typeof v === 'string' && v.length >= 4 && v.length <= 955736 && /^[A-Za-z0-9+/]+={0,2}$/u.test(v) }),
  finalizeUpload: shape({ uploadId: id }),
  cancelUpload: shape({ uploadId: id }),
  readAttachmentPart: shape({ attachmentId: id, index: partIndex }),
  revokeAttachment: shape({ attachmentId: id, changeId: id }),
  pauseSharedBot: shape({ sharedBotId: id, expectedRevision: positive }),
  answerWait: shape({ conversationId: id, waitId: id, messageId: id, changeId: id, text: v => typeof v === 'string' && v.length >= 1 && new TextEncoder().encode(v).length <= 8192 }),
  decideWait: shape({ conversationId: id, waitId: id, messageId: id, changeId: id, decision: v => v === 'approve' || v === 'deny' }),
  cancelTask: shape({ taskId: id }),
  getTask: shape({ taskId: id }),
  listTasks: shape({ conversationId: optional(id), after: optional(id) }),
  listTriage: shape({ sharedBotId: id, conversationId: optional(id), cursor: optional(id), limit: optional(v => integer(v) && (v as number) >= 1 && (v as number) <= TRIAGE_LOG_LIMIT) }),
  readArtifact: shape({ artifactId: id, revision: optional(positive) }),
  listArtifactRevisions: shape({ artifactId: id }),
  reviseArtifact: shape({ artifactId: id, expectedRevision: positive, operationId: id, document: shape({
    title: v => typeof v === 'string' && v.trim().length >= 1 && v.trim().length <= 120,
    body: v => typeof v === 'string' && v.length >= 1 && bytes(v) <= DOCUMENT_BYTES }) }),
  listOperations: shape({ taskId: id }),
  readList: shape({ sharedBotId: id }),
  writeList: shape({ sharedBotId: id, operationId: id, ...listWrite }),
  listHistory: shape({ sharedBotId: id, beforeRevision: optional(positive) }),
  resolveOperation: shape({ taskId: id, operationId: v => typeof v === 'string' && /^[a-f0-9]{64}$/u.test(v), resolution: oneOf('succeeded', 'failed') }),
  setConversationPolicy: shape({ conversationId: id, expectedRevision: integer, targets: list(id, 0, 8) }),
  setConversationAutoReply: shape({ conversationId: id, expectedRevision: integer, targets: list(id, 0, 8) }),
  setConversationBotScope: shape({ conversationId: id, sharedBotId: id, expectedRevision: integer, scope: v => v === null || isBotScopeNarrowing(v) }),
  endTransfer: shape({ transferId: id }),
};
/** Only the current member execution envelope. Unknown fields and retired commands fail closed. */
export function isMemberExecutionCommand(value: unknown): value is MemberExecutionCommand {
  return shape({ schemaVersion: v => v === 1, commandId: id, teamId: id, generation: positive,
    action: v => typeof v === 'string' && Object.prototype.hasOwnProperty.call(payloads, v), payload: v => record(v) })(value)
    && record(value) && payloads[value.action as string]!(value.payload);
}
