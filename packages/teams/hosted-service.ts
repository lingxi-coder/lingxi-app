import { TeamsApiError } from './index.js';
import { chatSourceMessage, type ChatReadRequest, type ChatReadResult, type ChatSourceMessage } from './chat-source.js';

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
  return SERVICE_PREFIX + btoa(String.fromCharCode(...bytes)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
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
