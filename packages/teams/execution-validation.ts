import type { MemberExecutionCommand } from './execution.js';

type Check = (value: unknown) => boolean;
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
const id: Check = v => typeof v === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(v);
const integer: Check = v => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
const positive: Check = v => integer(v) && (v as number) > 0;
const uid: Check = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(v);
const optional = (check: Check): Check => v => v === undefined || check(v);
const list = (check: Check, min: number, max: number): Check => v => Array.isArray(v) && v.length >= min && v.length <= max && new Set(v).size === v.length && v.every(check);
function shape(fields: Record<string, Check>): Check {
  return v => record(v) && Object.keys(v).every(key => Object.prototype.hasOwnProperty.call(fields, key)) && Object.entries(fields).every(([key, check]) => check(v[key]));
}
const payloads: Record<string, Check> = {
  listSharedBots: shape({}),
  createConversation: shape({ conversationId: id, kind: v => ['bot_dm', 'group', 'channel'].includes(String(v)), title: v => typeof v === 'string' && v.trim().length >= 1 && v.trim().length <= 120, participantUids: list(uid, 1, 200), sharedBotIds: list(id, 1, 8) }),
  submit: shape({ conversationId: id, messageId: id, changeId: id, text: v => typeof v === 'string' && v.length >= 1 && new TextEncoder().encode(v).length <= 8192, targets: list(id, 1, 8), replyTo: optional(id) }),
  pauseSharedBot: shape({ sharedBotId: id, expectedRevision: positive }),
  getTask: shape({ taskId: id }),
  listTasks: shape({ conversationId: optional(id), after: optional(id) }),
  readArtifact: shape({ artifactId: id }),
  setConversationPolicy: shape({ conversationId: id, expectedRevision: integer, targets: list(id, 0, 8) }),
};
/** Only the current member execution envelope. Unknown fields and retired commands fail closed. */
export function isMemberExecutionCommand(value: unknown): value is MemberExecutionCommand {
  return shape({ schemaVersion: v => v === 1, commandId: id, teamId: id, generation: positive,
    action: v => typeof v === 'string' && Object.prototype.hasOwnProperty.call(payloads, v), payload: v => record(v) })(value)
    && record(value) && payloads[value.action as string]!(value.payload);
}
