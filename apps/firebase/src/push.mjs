import { createHash } from 'node:crypto';
import { z } from 'zod';
import { ApiError } from './catalog.mjs';

export const REGISTRATION_TTL_MS = 30 * 24 * 60 * 60 * 1000;
export const DEFAULT_MESSAGE_TTL_SECONDS = 60 * 60;
export const SELF_PUSH_LIMIT = 30;
export const SELF_PUSH_WINDOW_MS = 10 * 60 * 1000;
const installationId = z.uuid();
const secret = z.string().regex(/^[a-f0-9]{64}$/);
const revision = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const appIds = {
  android: new Set(['com.lingxi.code', 'com.lingxi.code.debug', 'com.lingxi.code.direct', 'com.lingxi.code.direct.debug']),
  ios: new Set(['com.lingxi.code', 'com.lingxi.code.full']),
};
const shortText = (max) => z.string().trim().min(1).max(max);
const registration = z.object({
  installationSecret: secret, revision,
  fcmInstallationId: z.string().regex(/^[A-Za-z0-9_-]{22}$/),
  platform: z.enum(['android', 'ios']), appId: shortText(160),
  appVersion: shortText(80), osVersion: shortText(80), deviceModel: shortText(160),
  locale: shortText(80), notificationsEnabled: z.boolean(),
}).strict().refine((v) => appIds[v.platform].has(v.appId), 'Unknown application');
const revocation = z.object({ installationSecret: secret, revision }).strict();
const notification = z.object({
  uid: shortText(128), title: shortText(120), body: shortText(500),
  ttlSeconds: z.number().int().min(1).max(86400).default(DEFAULT_MESSAGE_TTL_SECONDS),
}).strict();
// Remote Control "push to my own devices". host_id is a 22-character base64url id; session_id uses
// the desktop's session-id pattern (apps/electron/src/main/sessionIdentity.ts), which keeps the
// collapse key under APNs' 64-byte limit.
const selfMessage = z.object({
  kind: z.enum(['permission', 'question', 'idle', 'task']),
  host_id: z.string().regex(/^[A-Za-z0-9_-]{22}$/),
  session_id: z.string().regex(/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i),
  exclude_installation_ids: z.array(installationId.transform((id) => id.toLowerCase())).max(16).default([]),
}).strict();
// Generic text only: a self push never carries prompt, tool or project text. The wording is the
// chat_background_* family from resources/translations (the desktop's own notifications use the same
// strings, and a question and an idle wait read the same there). Installations store their locale:
// zh* gets Chinese, everything else English.
const waiting = { zh: ['灵犀正在等待你', '后台对话需要你的输入才能继续'], en: ['Lingxi is waiting for you', 'A background conversation needs your input before it can continue.'] };
const SELF_COPY = {
  permission: { zh: ['灵犀需要你的许可', '灵犀需要你的批准才能继续。'], en: ['Lingxi needs your permission', 'Lingxi needs your approval before it can continue.'] },
  question: waiting, idle: waiting,
  task: { zh: ['灵犀已完成', '后台对话已完成，点按查看结果'], en: ['Lingxi finished', 'The background conversation is complete. Tap to view it.'] },
};
const selfCopy = (kind, locale) => SELF_COPY[kind][/^zh(?:[-_]|$)/i.test(locale ?? '') ? 'zh' : 'en'];
const digest = (value) => createHash('sha256').update(value).digest('hex');
const millis = (value) => value?.toMillis?.() ?? Number.NaN;

// All collections are private to the Admin SDK. The FID claim makes an FCM registration
// unique across installations; the installation secret permits offline logout
// retries without retaining a Firebase Auth credential after sign-out.
// pushSelfLimits holds one small rate-limit document per user for the self-push route.
export class PushStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  device(id) { return this.db.doc(`pushInstallations/${id}`); }
  claim(hash) { return this.db.doc(`pushRegistrationClaims/${hash}`); }
  limit(uid) { return this.db.doc(`pushSelfLimits/${digest(uid)}`); }

  async initialize(id, input) {
    installationId.parse(id);
    const { installationSecret } = z.object({ installationSecret: secret }).strict().parse(input);
    const secretHash = digest(installationSecret);
    await this.db.runTransaction(async (tx) => {
      const ref = this.device(id), previous = (await tx.get(ref)).data();
      if (previous && previous.secretHash !== secretHash) throw new ApiError(403, 'INSTALLATION_SECRET_MISMATCH');
      if (!previous) tx.create(ref, { secretHash, revision: 0, uid: null, registrationHash: null,
        expiresAt: new Date(this.clock() + REGISTRATION_TTL_MS) });
    });
    return { initialized: true };
  }

  async mutate(id, input, uid = null) {
    installationId.parse(id);
    const value = (uid ? registration : revocation).parse(input);
    const secretHash = digest(value.installationSecret), registrationHash = uid ? digest(value.fcmInstallationId) : null;
    return this.db.runTransaction(async (tx) => {
      const ref = this.device(id), snapshot = await tx.get(ref), previous = snapshot.data();
      // Initialization requires Firebase Auth and contains no deliverable FID.
      // An anonymous revoke never creates documents; a cancelled initialization
      // cannot later bind a user because binding is a separate client request.
      if (!previous) {
        if (uid) throw new ApiError(409, 'INSTALLATION_NOT_INITIALIZED');
        return { revision: value.revision, expiresAt: new Date(this.clock() + REGISTRATION_TTL_MS).toISOString() };
      }
      if (previous && previous.secretHash !== secretHash) throw new ApiError(403, 'INSTALLATION_SECRET_MISMATCH');
      if (previous && value.revision < previous.revision) throw new ApiError(409, 'STALE_REGISTRATION');
      if (previous && value.revision === previous.revision) {
        const { installationSecret: _, ...fields } = value;
        if (previous.uid !== uid || (uid && Object.entries(fields).some(([key, entry]) => previous[key] !== entry))) {
          throw new ApiError(409, 'STALE_REGISTRATION');
        }
        return { revision: previous.revision, expiresAt: previous.expiresAt.toDate().toISOString() };
      }
      const now = this.clock(), expiresAt = new Date(now + REGISTRATION_TTL_MS);
      const claimRef = registrationHash ? this.claim(registrationHash) : null;
      const claimed = claimRef ? (await tx.get(claimRef)).data() : null;
      if (claimed && claimed.installationId !== id && millis(claimed.expiresAt) > now) {
        throw new ApiError(409, 'REGISTRATION_ALREADY_BOUND');
      }
      // Read the old claim before writing anything (Firestore transaction rule).
      const oldClaimRef = previous?.registrationHash && previous.registrationHash !== registrationHash ? this.claim(previous.registrationHash) : null;
      const oldClaim = oldClaimRef ? (await tx.get(oldClaimRef)).data() : null;
      if (oldClaim?.installationId === id) tx.delete(oldClaimRef);
      if (claimRef) tx.set(claimRef, { installationId: id, expiresAt });
      const common = { secretHash, revision: value.revision, updatedAt: new Date(now), expiresAt };
      if (uid) {
        const { installationSecret: _, ...device } = value;
        tx.set(ref, { ...device, ...common, uid, registrationHash,
          createdAt: previous?.createdAt ?? new Date(now), lastSeenAt: new Date(now) });
      } else {
        // Retain a tombstone so a delayed authenticated PUT cannot resurrect a
        // logged-out binding. TTL/scheduled cleanup removes it after 30 days.
        tx.set(ref, { ...common, uid: null, registrationHash: null, revokedAt: new Date(now) });
      }
      return { revision: value.revision, expiresAt: expiresAt.toISOString() };
    });
  }

  async active(uid) {
    const snapshot = await this.db.collection('pushInstallations').where('uid', '==', uid).get();
    return snapshot.docs.map((d) => ({ id: d.id, ...d.data() }))
      .filter((d) => d.notificationsEnabled && d.fcmInstallationId && millis(d.expiresAt) > this.clock());
  }

  async invalidate(device) {
    // A failed send for an old FID must not revoke a replacement FID or a
    // new user's binding. Check the exact snapshot inside the transaction.
    return this.db.runTransaction(async (tx) => {
      const ref = this.device(device.id), current = (await tx.get(ref)).data();
      if (current?.revision !== device.revision || current?.registrationHash !== device.registrationHash) return;
      const claimRef = this.claim(device.registrationHash), claim = (await tx.get(claimRef)).data();
      if (claim?.installationId === device.id) tx.delete(claimRef);
      tx.set(ref, { secretHash: current.secretHash, revision: current.revision, uid: null, registrationHash: null,
        revokedAt: new Date(this.clock()), expiresAt: current.expiresAt });
    });
  }

  // At most SELF_PUSH_LIMIT self pushes per user in any SELF_PUSH_WINDOW_MS: a sliding window of
  // send timestamps (at most SELF_PUSH_LIMIT numbers) in one document, updated transactionally so
  // concurrent requests cannot oversubscribe it. A refused request is not recorded.
  async takeSelfQuota(uid) {
    return this.db.runTransaction(async (tx) => {
      const ref = this.limit(uid), now = this.clock();
      const recent = ((await tx.get(ref)).data()?.sentAt ?? []).filter((at) => at > now - SELF_PUSH_WINDOW_MS);
      if (recent.length >= SELF_PUSH_LIMIT) throw new ApiError(429, 'RATE_LIMITED');
      tx.set(ref, { sentAt: [...recent, now], expiresAt: new Date(now + SELF_PUSH_WINDOW_MS) });
    });
  }

  async prune() {
    let deleted = 0;
    // Bound each invocation. The query is repeated because deletes shift pages.
    for (let page = 0; page < 20; page++) {
      const snapshot = await this.db.collection('pushInstallations').where('expiresAt', '<=', new Date(this.clock())).limit(100).get();
      if (snapshot.empty) break;
      for (const doc of snapshot.docs) {
        await this.db.runTransaction(async (tx) => {
          const current = (await tx.get(doc.ref)).data();
          if (!current || millis(current.expiresAt) > this.clock()) return;
          const claimRef = current.registrationHash ? this.claim(current.registrationHash) : null;
          const claim = claimRef ? (await tx.get(claimRef)).data() : null;
          if (claim?.installationId === doc.id) tx.delete(claimRef);
          tx.delete(doc.ref); deleted++;
        });
      }
    }
    // A closed rate-limit window is worthless. Losing one that was refreshed a moment ago only
    // resets that user's quota, so these are plain batch deletes (and not counted in `deleted`).
    for (let page = 0; page < 20; page++) {
      const snapshot = await this.db.collection('pushSelfLimits').where('expiresAt', '<=', new Date(this.clock())).limit(100).get();
      if (snapshot.empty) break;
      const batch = this.db.batch();
      for (const doc of snapshot.docs) batch.delete(doc.ref);
      await batch.commit();
    }
    return { deleted };
  }
}

// Optional input: `route` (FCM data), `collapseKey` (Android collapseKey, APNs apns-collapse-id)
// and `data` (extra string entries; the base keys always win). Without them the output is unchanged.
export function buildPushMessage(device, input, now = Date.now()) {
  const expires = Math.min(now + input.ttlSeconds * 1000, millis(device.expiresAt));
  return {
    fid: device.fcmInstallationId,
    data: { ...input.data, recipientUid: device.uid, expiresAt: String(expires), title: input.title, body: input.body,
      ...(input.route ? { route: input.route } : {}) },
    // Data-only on Android so the app can reject late messages for an old user.
    android: { priority: 'high', ttl: Math.max(0, expires - now), restrictedPackageName: device.appId,
      ...(input.collapseKey ? { collapseKey: input.collapseKey } : {}) },
    apns: {
      headers: { 'apns-expiration': String(Math.floor(expires / 1000)), 'apns-push-type': 'alert', 'apns-priority': '10', 'apns-topic': device.appId,
        ...(input.collapseKey ? { 'apns-collapse-id': input.collapseKey } : {}) },
      payload: { aps: { alert: { title: input.title, body: input.body }, sound: 'default' } },
    },
  };
}

// One FCM message per device, built by `build(device)`.
async function deliver(store, messaging, devices, build, clock, allowed = async () => true) {
  let sent = 0, failed = 0, invalidated = 0;
  for (const device of devices) {
    // Re-read immediately before sending; registration/logout may have changed
    // while a previous device was being sent to.
    if (!await allowed(device)) continue;
    const current = (await store.device(device.id).get()).data();
    if (current?.uid !== device.uid || current?.revision !== device.revision || millis(current.expiresAt) <= clock()) continue;
    try {
      await messaging.send(build(device)); sent++;
    } catch (error) {
      failed++;
      if (['messaging/registration-token-not-registered', 'messaging/invalid-registration-token'].includes(error.code)) {
        await store.invalidate(device); invalidated++;
      }
      // Generic invalid-argument can describe the payload. Never delete a registration
      // for that error or for transient provider/authentication failures.
    }
  }
  return { sent, failed, invalidated };
}

export async function sendUserPush(store, messaging, input, clock = Date.now) {
  const value = notification.parse(input), devices = await store.active(value.uid);
  return deliver(store, messaging, devices, (device) => buildPushMessage(device, value, clock()), clock);
}

// POST /v1/push/self/messages: a generic, localized notification to the caller's own devices except
// the excluded installations (the ones already showing that session). Only a request that has at
// least one device to send to uses up quota. `skipped` counts devices not sent to (excluded, or
// changed between lookup and send); `failed` counts provider errors.
export async function sendSelfPush(store, messaging, uid, input, clock = Date.now) {
  const { kind, host_id, session_id, exclude_installation_ids } = selfMessage.parse(input), excluded = new Set(exclude_installation_ids);
  const active = await store.active(uid), devices = active.filter((device) => !excluded.has(device.id));
  if (devices.length) await store.takeSelfQuota(uid);
  const { sent, failed } = await deliver(store, messaging, devices, (device) => {
    const [title, body] = selfCopy(kind, device.locale);
    return buildPushMessage(device, { title, body, ttlSeconds: DEFAULT_MESSAGE_TTL_SECONDS, route: `remote/${host_id}/${session_id}`,
      collapseKey: `rc:${session_id}:${kind}`, data: { kind, host_id, session_id } }, clock());
  }, clock);
  return { sent, skipped: active.length - sent - failed, failed };
}

export function createPushHandler({ store, messaging, verifyToken, origins = [] }) {
  return async (request) => {
    const headers = new Headers({ 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', Vary: 'Origin' });
    const reply = (body, status = 200) => new Response(JSON.stringify(body), { status, headers });
    try {
      const origin = request.headers.get('Origin');
      if (origin && !origins.includes(origin)) throw new ApiError(403, 'ORIGIN_NOT_ALLOWED');
      if (origin) headers.set('Access-Control-Allow-Origin', origin);
      if (request.method === 'OPTIONS') {
        headers.set('Access-Control-Allow-Methods', 'PUT, DELETE, POST, OPTIONS');
        headers.set('Access-Control-Allow-Headers', 'Authorization, Content-Type');
        return new Response(null, { status: 204, headers });
      }
      const path = new URL(request.url).pathname;
      if (path === '/health' && request.method === 'GET') return reply({ status: 'ok', service: 'push' });
      const match = /^\/v1\/push\/installations\/([a-f0-9-]{36})(\/initialize)?$/.exec(path);
      const initializing = match?.[2] === '/initialize' && request.method === 'POST';
      const admin = path === '/v1/admin/push/messages' && request.method === 'POST';
      const self = path === '/v1/push/self/messages' && request.method === 'POST';
      if (!(match && !match[2] && ['PUT', 'DELETE'].includes(request.method)) && !initializing && !admin && !self) throw new ApiError(404, 'NOT_FOUND');
      let user;
      if (request.method !== 'DELETE') {
        const bearer = /^Bearer (\S+)$/.exec(request.headers.get('Authorization') || '');
        if (!bearer) throw new ApiError(401, 'SIGN_IN_REQUIRED');
        try { user = await verifyToken(bearer[1]); } catch { throw new ApiError(401, 'INVALID_SESSION'); }
        if (!user?.uid) throw new ApiError(401, 'INVALID_SESSION');
        if (admin && (user.push_admin !== true || user.email_verified !== true)) throw new ApiError(403, 'PUSH_ADMIN_REQUIRED');
      }
      const raw = await request.text();
      if (Buffer.byteLength(raw) > 16384) throw new ApiError(413, 'REQUEST_TOO_LARGE');
      let body;
      try { body = JSON.parse(raw); } catch { throw new ApiError(400, 'INVALID_JSON'); }
      if (admin) return reply(await sendUserPush(store, messaging, body));
      if (self) return reply(await sendSelfPush(store, messaging, user.uid, body));
      if (initializing) return reply(await store.initialize(match[1], body));
      return reply(await store.mutate(match[1], body, user?.uid));
    } catch (error) {
      if (error instanceof z.ZodError) return reply({ error: { code: 'INVALID_INPUT' } }, 400);
      if (error instanceof ApiError) return reply({ error: { code: error.code } }, error.status);
      // Do not log error objects: provider errors may contain registration data.
      return reply({ error: { code: 'SERVICE_UNAVAILABLE' } }, 503);
    }
  };
}

// Internal Team trigger only. Push contains no message body or invite/session token.
export async function sendTeamPush(store, messaging, input, allowed, clock = Date.now) {
  const devices = await store.active(input.uid);
  return deliver(store, messaging, devices, device => buildPushMessage(device, {
    title: input.title, body: /^zh(?:[-_]|$)/i.test(device.locale ?? '') ? '团队有新消息' : 'New message in your team',
    ttlSeconds: 3600, route: 'team/inbox', collapseKey: input.collapseKey, data: { notificationId: input.notificationId, kind: 'team_message' },
  }, clock()), clock, allowed);
}
