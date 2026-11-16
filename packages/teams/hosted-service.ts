import { TeamsApiError } from './index.js';
import { chatSourceMessage, type ChatReadRequest, type ChatReadResult, type ChatSourceMessage } from './chat-source.js';
import { executionCommand, type LocalPublishableBot, type MemberExecutionPayloads, type PublishLocalBotInput, type SharedHostStatus } from './execution.js';
import { isMemberExecutionCommand } from './execution-validation.js';

/**
 * A Bot offered to people outside its publisher's Teams (a hosted Bot service). A service is a scope of its own: it is
 * not a Team, its callers are not members of anything, and no client reads its records directly. Everything goes through
 * the API: `/v1/services` for the service itself, and the Team message and task routes with `serviceId` for its sessions.
 */

/** The id space of a service. No Team has such an id, so an id alone says which scope it names. */
export const SERVICE_PREFIX = 's_';
export const isServiceId = (value: unknown): value is string => typeof value === 'string' && /^s_[A-Za-z0-9_-]{6,62}$/u.test(value);
export function newServiceId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return SERVICE_PREFIX + btoa(String.fromCharCode(...bytes)).replace(/\+/gu, '-').replace(/\//gu, '_').replace(/=/gu, '');
}

export type ServiceAudience = { mode: 'public' } | { mode: 'users'; uids: string[] };
/** How many requests of one caller, and of everyone, may be under way at once. */
export interface ServiceLimits { userConcurrent: number; serviceConcurrent: number }
/** Why a request would not be answered now, or `available`. */
export type ServiceAvailability = 'withdrawn' | 'disabled' | 'not_ready' | 'host_offline' | 'service_quota_exhausted' | 'user_quota_exhausted' | 'available';
/** What a person who may use a service is told about it. */
export interface ServiceView {
  serviceId: string; name: string; description: string; publisherName: string; sharedBotId: string;
  availability: ServiceAvailability; entered: boolean; generation?: number;
  /** Who pays for the model and the tools, who can read a session, and what the publisher says is kept. */
  cost: 'publisher'; visibility: 'requester_and_publisher'; retention: string;
  limits: { userConcurrent: number; userDaily?: number; userDailyLeft?: number };
}
/** The publisher's own view: everything they set. */
export interface PublishedService {
  serviceId: string; state: 'active' | 'withdrawn'; online: boolean; publisher: string; publisherName: string; sharedBotId: string;
  name: string; description: string; audience: ServiceAudience; limits: ServiceLimits; retention: string;
  cost: 'publisher'; visibility: 'requester_and_publisher'; revision: number; createdAt: number; updatedAt: number;
}
export type ServiceReportReason = 'harmful' | 'deceptive' | 'privacy' | 'spam' | 'other';
export type ServiceCommand =
  | { action: 'createService'; serviceId: string; sharedBotId: string; name: string; description: string; audience: ServiceAudience; limits: ServiceLimits; retention: string }
  | { action: 'updateService'; serviceId: string; expectedRevision: number; name?: string; description?: string; audience?: ServiceAudience; limits?: ServiceLimits; retention?: string }
  | { action: 'setServiceOnline'; serviceId: string; online: boolean }
  | { action: 'withdrawService'; serviceId: string }
  | { action: 'getService'; serviceId: string }
  /** `acknowledged` says the caller was shown who pays, who can read the session and what is kept. */
  | { action: 'enterService'; serviceId: string; acknowledged: true }
  | { action: 'leaveService'; serviceId: string }
  | { action: 'reportService'; serviceId: string; reportId: string; reason: ServiceReportReason; text: string; sessionId?: string }
  | { action: 'listServices' };
export interface ServiceList { published: PublishedService[]; entered: ServiceView[] }

/** `teamsTransport` for `/v1/services`: every action is `services(command)`. */
export function servicesTransport(base: string, token: () => Promise<string>, fetcher: typeof fetch = fetch) {
  return async <T>(command: ServiceCommand): Promise<T> => {
    const credential = await token();
    const response = await fetcher(`${base}/v1/services`, { method: 'POST',
      headers: { Authorization: `Bearer ${credential}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(command), cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(20000) });
    const value = await response.json();
    if (!response.ok) throw new TeamsApiError(value?.error?.code ?? 'SERVICE_UNAVAILABLE', response.status);
    return value as T;
  };
}

const fail = (code: string): never => { throw new Error(code); };
const counter = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
/** The API gives times as ISO text; the message check takes what Firestore gives. */
const stamp = (value: unknown) => {
  const at = typeof value === 'string' ? Date.parse(value) : NaN;
  return Number.isSafeInteger(at) && at >= 0 ? { toMillis: () => at } : fail('INVALID_SOURCE_TIME');
};
interface Session { after: number; lastSeq: number; messages: Map<string, ChatSourceMessage>; changed: Array<{ seq: number; messageId: string }> }

/**
 * Reads a service session's messages for the Bot that answers in it, with the same requests and the same checks as the
 * Team reader (`createChatSourceReader`), but through the API: a service's records cannot be read directly. `request.teamId`
 * is the service's id. A session is followed by its change log from the start and kept in memory, so a later read asks
 * only for what changed since.
 */
export function createServiceChatReader(call: (command: Record<string, unknown>) => Promise<unknown>, sessionLimit = 32) {
  const sessions = new Map<string, Session>(), queues = new Map<string, Promise<unknown>>();
  async function follow(request: ChatReadRequest, key: string): Promise<Session & { lastChangeSeq: number }> {
    const session = sessions.get(key) ?? { after: 0, lastSeq: 0, messages: new Map<string, ChatSourceMessage>(), changed: [] };
    for (;;) {
      const page = await call({ action: 'sync', serviceId: request.teamId, generation: request.generation, conversationId: request.conversationId, after: session.after }) as
        { conversation?: { lastSeq?: unknown; lastChangeSeq?: unknown }; changes?: unknown; messages?: unknown; hasMore?: unknown };
      const lastSeq = page?.conversation?.lastSeq, lastChangeSeq = page?.conversation?.lastChangeSeq;
      if (!counter(lastSeq) || !counter(lastChangeSeq) || !Array.isArray(page.changes) || !Array.isArray(page.messages) || page.changes.length > 100) return fail('INVALID_DATA');
      const sent = new Map<string, Record<string, unknown>>();
      for (const item of page.messages as Array<Record<string, unknown> | null>) { if (!item || typeof item.id !== 'string') return fail('INVALID_DATA'); sent.set(item.id, item); }
      for (const [index, change] of (page.changes as Array<{ seq?: unknown; messageId?: unknown } | null>).entries()) {
        if (change?.seq !== session.after + index + 1 || typeof change.messageId !== 'string') return fail('CHANGE_GAP');
        const data = sent.get(change.messageId); if (!data) return fail('MESSAGE_GAP');
        const { id: _id, editedAt, createdAt, ...rest } = data;
        session.messages.set(change.messageId, chatSourceMessage(change.messageId, { ...rest, createdAt: stamp(createdAt), ...(editedAt == null ? {} : { editedAt: stamp(editedAt) }) }));
        session.changed.push({ seq: change.seq as number, messageId: change.messageId });
      }
      session.after += page.changes.length; session.lastSeq = lastSeq;
      if (session.after > lastChangeSeq || (!page.changes.length && session.after !== lastChangeSeq)) return fail('CHANGE_GAP');
      if (session.after === lastChangeSeq) {
        sessions.delete(key); sessions.set(key, session);
        for (const oldest of sessions.keys()) { if (sessions.size <= sessionLimit) break; sessions.delete(oldest); }
        return { ...session, lastChangeSeq };
      }
    }
  }
  async function perform(request: ChatReadRequest, key: string): Promise<ChatReadResult> {
    // A read that stopped half way leaves nothing behind: the next one follows the session from its start.
    const session = await follow(request, key).catch((error: unknown) => { sessions.delete(key); throw error; });
    const all = [...session.messages.values()].sort((a, b) => a.seq - b.seq);
    let messages: ChatSourceMessage[] = [], hasMore = false;
    if (request.mode === 'changes') {
      if (request.afterChangeSeq > session.lastChangeSeq || request.throughMessageSeq > session.lastSeq) fail('INVALID_READ_CURSOR');
      const ids = new Set(session.changed.filter(change => change.seq > request.afterChangeSeq).map(change => change.messageId));
      messages = all.filter(message => ids.has(message.messageId) && message.seq <= request.throughMessageSeq);
    } else if (request.mode === 'byId') {
      const found = session.messages.get(request.messageId); if (found) messages = [found];
    } else if (request.mode === 'before') {
      const size = request.limit ?? 200, earlier = all.filter(message => message.seq < request.beforeSeq);
      hasMore = earlier.length > size; messages = earlier.slice(-size);
    } else {
      const to = request.mode === 'recent' ? request.throughSeq : request.toSeqInclusive;
      const from = request.mode === 'recent' ? Math.max(0, to - 200) : request.fromSeqExclusive;
      if (to > session.lastSeq || to <= from) fail('INVALID_READ_CURSOR');
      messages = all.filter(message => message.seq > from && message.seq <= to);
      if (messages.length !== to - from || messages.some((message, index) => message.seq !== from + index + 1)) fail('MESSAGE_GAP');
      hasMore = from > 0;
    }
    return { messages, lastSeq: session.lastSeq, lastChangeSeq: session.lastChangeSeq, hasMore };
  }
  return {
    /** One read of a session at a time: each builds on what the one before it followed. */
    read(request: ChatReadRequest): Promise<ChatReadResult> {
      const key = JSON.stringify([request.teamId, request.generation, request.conversationId]);
      const next = (queues.get(key) ?? Promise.resolve()).catch(() => undefined).then(() => perform(request, key));
      queues.set(key, next);
      void next.catch(() => undefined).finally(() => { if (queues.get(key) === next) queues.delete(key); });
      return next;
    },
  };
}

/** A caller's entitlement in a service: what every session and task command names. */
export interface ServiceScope { serviceId: string; generation: number }
/** A session of a service as the API lists it. */
export interface ServiceSession { id: string; kind: 'bot_dm'; title: string; lastSeq: number; lastChangeSeq: number; lastReadSeq: number }
/** A message of a session as the API's `sync` gives it. Times are ISO text. */
export interface ServiceMessage {
  id: string; seq: number; changeSeq: number; text: string; createdAt: string | null; recalled?: boolean; waitId?: string;
  author: { kind: 'user'; uid: string; displayName?: string } | { kind: 'shared_bot'; sharedBotId: string; displayName?: string };
}
export interface ServiceSync { conversation: ServiceSession; changes: Array<{ seq: number; messageId: string }>; messages: ServiceMessage[]; hasMore: boolean }
/** What a client needs to use services. The last three exist only where Bots are hosted (Desktop), for the publisher. */
export interface HostedServicesApi {
  services<T>(command: ServiceCommand): Promise<T>;
  /** A session or task command in a service's scope: the Team message route with `serviceId`. */
  serviceMessages<T>(command: Record<string, unknown> & { serviceId: string; generation: number; action: string }): Promise<T>;
  localBots?(): Promise<LocalPublishableBot[]>;
  publishLocalBot?(input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }>;
  hostStatus?(): Promise<SharedHostStatus>;
}
/** The caller's actions in a service's scope; the service refuses every other one. */
export const SERVICE_READ_ACTIONS = ['list', 'sync', 'read'] as const;
export const SERVICE_TASK_ACTIONS = ['createConversation', 'submit', 'answerWait', 'decideWait', 'cancelTask', 'getTask', 'listTasks'] as const;
/** Whether a value is a command a caller may send in a service's scope. Hosts check this before sending what a page asked for. */
export function isServiceMessageCommand(value: unknown): value is Record<string, unknown> & { serviceId: string; generation: number; action: string } {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const command = value as Record<string, unknown>;
  if (!isServiceId(command.serviceId) || 'teamId' in command || !Number.isSafeInteger(command.generation) || (command.generation as number) < 1) return false;
  if (!('schemaVersion' in command)) return (SERVICE_READ_ACTIONS as readonly unknown[]).includes(command.action);
  // The same shape as its Team counterpart, with the scope named differently.
  const { serviceId, ...rest } = command;
  return (SERVICE_TASK_ACTIONS as readonly unknown[]).includes(command.action) && isMemberExecutionCommand({ ...rest, teamId: serviceId });
}
/** A task command in a service's scope, checked like its Team counterpart. */
export function serviceCommand<A extends typeof SERVICE_TASK_ACTIONS[number]>(scope: ServiceScope, action: A, payload: MemberExecutionPayloads[A]) {
  const { teamId, ...command } = executionCommand({ teamId: scope.serviceId, generation: scope.generation }, action, payload) as
    { teamId: string; schemaVersion: 1; commandId: string; generation: number; action: A; payload: MemberExecutionPayloads[A] };
  return { ...command, serviceId: teamId };
}
/** The service a link or a pasted id names: `…/services/<id>` or the id itself. */
export function serviceIdFrom(input: string): string | null {
  const value = input.trim();
  if (isServiceId(value)) return value;
  try { const found = /^\/services\/([^/]+)\/?$/u.exec(new URL(value).pathname)?.[1]; return isServiceId(found) ? found : null; } catch { return null; }
}
export const serviceLink = (origin: string, serviceId: string): string => `${origin}/services/${serviceId}`;

/** Why a service would not answer now, in words. */
export function availabilityText(availability: ServiceAvailability, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  switch (availability) {
    case 'available': return t('Available', '可用');
    case 'disabled': return t('Turned off by its publisher', '发布者已关闭');
    case 'not_ready': return t('Not set up yet', '尚未就绪');
    case 'host_offline': return t('The publisher’s computer is offline', '发布者的电脑不在线');
    case 'service_quota_exhausted': return t('Today’s requests for this service are used up', '该服务今日的请求次数已用完');
    case 'user_quota_exhausted': return t('You have used today’s requests', '你今日的请求次数已用完');
    case 'withdrawn': return t('Withdrawn', '已下线');
  }
}
/** What a refusal means to the person who met it. Unknown codes are shown as they are. */
export function serviceErrorText(code: string, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  const known: Record<string, string> = {
    NOT_FOUND: t('There is no such service, or it is not offered to this account.', '没有这个服务，或它未向此账号开放。'),
    SERVICE_ENTRY_REQUIRED: t('Open the service and agree to its terms first.', '请先打开该服务并同意其说明。'),
    SERVICE_ACCESS_REQUIRED: t('This service is no longer offered to this account.', '该服务已不再向此账号开放。'),
    STALE_MEMBERSHIP: t('Your access changed. Open the service again.', '你的访问已变更，请重新打开该服务。'),
    SERVICE_WITHDRAWN: t('The publisher withdrew this service.', '发布者已下线该服务。'),
    SERVICE_DISABLED: t('The publisher turned this service off. Nothing was sent.', '发布者已关闭该服务，消息未发送。'),
    SERVICE_USER_BUSY: t('Wait for your earlier request to finish. Nothing was sent.', '请等待你之前的请求完成，消息未发送。'),
    SERVICE_BUSY: t('The service is busy with other people’s requests. Nothing was sent; try again shortly.', '该服务正在处理其他人的请求，消息未发送，请稍后再试。'),
    REQUEST_QUOTA_EXCEEDED: t('Today’s requests are used up. Nothing was sent.', '今日的请求次数已用完，消息未发送。'),
    HOST_UNAVAILABLE: t('The publisher’s computer is offline. Nothing was sent.', '发布者的电脑不在线，消息未发送。'),
    SERVICE_SESSION_LIMIT: t('You have the most sessions this service allows.', '你在该服务中的会话数已达上限。'),
    REPORT_LIMIT: t('You have sent today’s reports. Try again tomorrow.', '你今日的举报次数已用完，请明天再试。'),
    SERVICE_LIMIT: t('You offer the most services one account may.', '你提供的服务数已达上限。'),
    PUBLISHER_REQUIRED: t('Only the publisher can do this.', '只有发布者可以执行此操作。'),
    SIGN_IN_REQUIRED: t('Sign in first.', '请先登录。'),
    TEAM_MODEL_UNAVAILABLE: t('This Bot’s model is not set up on this computer.', '此电脑上尚未配置该 Bot 的模型。'),
    RESOURCE_BINDING_REQUIRED: t('This Bot needs files chosen for it, which a service does not carry yet.', '该 Bot 需要绑定文件，服务暂不支持。'),
    SERVICE_UNAVAILABLE: t('The service could not be reached. Try again.', '无法连接服务，请重试。'),
  };
  return known[code] ?? code;
}
