import { createHash } from 'node:crypto';
import { z } from 'zod';
import { ApiError } from './catalog.mjs';

export const REGISTRATION_TTL_MS = 30 * 24 * 60 * 60 * 1000;
export const DEFAULT_MESSAGE_TTL_SECONDS = 60 * 60;
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
const digest = (value) => createHash('sha256').update(value).digest('hex');
const millis = (value) => value?.toMillis?.() ?? Number.NaN;

// Both collections are private to the Admin SDK. The FID claim makes an FCM registration
// unique across installations; the installation secret permits offline logout
// retries without retaining a Firebase Auth credential after sign-out.
export class PushStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  device(id) { return this.db.doc(`pushInstallations/${id}`); }
  claim(hash) { return this.db.doc(`pushRegistrationClaims/${hash}`); }

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
    return { deleted };
  }
}

export function buildPushMessage(device, input, now = Date.now()) {
  const expires = Math.min(now + input.ttlSeconds * 1000, millis(device.expiresAt));
  return {
    fid: device.fcmInstallationId,
    data: { recipientUid: device.uid, expiresAt: String(expires), title: input.title, body: input.body },
    // Data-only on Android so the app can reject late messages for an old user.
    android: { priority: 'high', ttl: Math.max(0, expires - now), restrictedPackageName: device.appId },
    apns: {
      headers: { 'apns-expiration': String(Math.floor(expires / 1000)), 'apns-push-type': 'alert', 'apns-priority': '10', 'apns-topic': device.appId },
      payload: { aps: { alert: { title: input.title, body: input.body }, sound: 'default' } },
    },
  };
}

export async function sendUserPush(store, messaging, input, clock = Date.now) {
  const value = notification.parse(input), devices = await store.active(value.uid);
  let sent = 0, failed = 0, invalidated = 0;
  for (const device of devices) {
    // Re-read immediately before sending; registration/logout may have changed
    // while a previous device was being sent to.
    const current = (await store.device(device.id).get()).data();
    if (current?.uid !== device.uid || current?.revision !== device.revision || millis(current.expiresAt) <= clock()) continue;
    try {
      await messaging.send(buildPushMessage(device, value, clock())); sent++;
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
      if (!(match && !match[2] && ['PUT', 'DELETE'].includes(request.method)) && !initializing && !admin) throw new ApiError(404, 'NOT_FOUND');
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
