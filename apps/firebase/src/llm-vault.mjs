import { createCipheriv, createDecipheriv, createHmac, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { FieldValue, Timestamp } from 'firebase-admin/firestore';
import { z } from 'zod';

/**
 * The model-provider key vault. A user's provider keys are kept so every LingXi client signed in to the account can use
 * them: the Web through the proxy (the browser never holds a key), native apps by an explicit, audited export.
 *
 * Each key is sealed with its own random data key (AES-256-GCM); the data key is sealed with the master key named by
 * `kid`. Both seals bind the owner and provider id as associated data, so a sealed record copied to another user or
 * provider does not open. The master keyring is a Secret Manager secret, `{"current":"k1","keys":{"k1":"<base64 32B>"}}`;
 * a new key is added as a new current and old ones stay to open older records. A lost key cannot be recovered.
 */

export const PRESETS = JSON.parse(readFileSync(new URL('./llm-presets.json', import.meta.url), 'utf8')).presets;
const presetById = new Map(PRESETS.map(preset => [preset.id, preset]));
export const presetOf = id => presetById.get(id);

/** Upstream protocols the proxy speaks. Desktop-only kinds (Azure, Bedrock, Vertex, Foundry) are stored as `other`. */
export const WEB_PROTOCOLS = ['open_ai_chat', 'open_ai_responses', 'anthropic_messages', 'gemini_generate_content'];
export const DESKTOP_TYPES = ['openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai',
  'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude'];
export const PROVIDER_LIMIT = 20;
export const CLIENTS = ['desktop', 'android', 'ios'];

export class VaultError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const fail = (status, code) => { throw new VaultError(status, code); };

const AAD = 'lingxi-llm-vault/v1';
export function parseKeyring(text) {
  let parsed;
  try { parsed = JSON.parse(text); } catch { throw new Error('The vault keyring is not JSON'); }
  const keys = new Map();
  for (const [kid, value] of Object.entries(parsed?.keys ?? {})) {
    const key = Buffer.from(String(value), 'base64');
    if (!/^[A-Za-z0-9_-]{1,32}$/u.test(kid) || key.length !== 32) throw new Error('A vault key is not 32 bytes');
    keys.set(kid, key);
  }
  if (!keys.has(parsed?.current)) throw new Error('The vault keyring has no current key');
  return { current: parsed.current, keys };
}

const gcm = (key, iv, aad, data) => {
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  cipher.setAAD(Buffer.from(aad));
  const body = Buffer.concat([cipher.update(data), cipher.final()]);
  return Buffer.concat([iv, cipher.getAuthTag(), body]).toString('base64');
};
const ungcm = (key, packed, aad) => {
  const raw = Buffer.from(packed, 'base64');
  if (raw.length < 29) throw new Error('short');
  const decipher = createDecipheriv('aes-256-gcm', key, raw.subarray(0, 12));
  decipher.setAAD(Buffer.from(aad));
  decipher.setAuthTag(raw.subarray(12, 28));
  return Buffer.concat([decipher.update(raw.subarray(28)), decipher.final()]);
};

/** Seals `secret` for `uid`'s provider `providerId`. */
export function seal(keyring, uid, providerId, secret) {
  const aad = `${AAD}|${uid}|${providerId}`, dek = randomBytes(32);
  try {
    return { v: 1, kid: keyring.current, dek: gcm(keyring.keys.get(keyring.current), randomBytes(12), `${aad}|dek`, dek),
      data: gcm(dek, randomBytes(12), aad, Buffer.from(secret, 'utf8')) };
  } finally { dek.fill(0); }
}

/** Opens a sealed record, or throws VAULT_UNREADABLE: a wrong owner, provider, key or a changed byte all look the same. */
export function open(keyring, uid, providerId, sealed) {
  const aad = `${AAD}|${uid}|${providerId}`, master = keyring.keys.get(sealed?.kid);
  let dek;
  try {
    if (sealed?.v !== 1 || !master) throw new Error('unknown');
    dek = ungcm(master, sealed.dek, `${aad}|dek`);
    return ungcm(dek, sealed.data, aad).toString('utf8');
  } catch { fail(500, 'VAULT_UNREADABLE'); }
  finally { dek?.fill(0); }
}

/** A stable, keyed fingerprint of a key, so clients can tell whether a key they hold is the one stored without seeing it. */
export function keyFingerprint(keyring, uid, secret) {
  const derived = createHmac('sha256', keyring.keys.get(keyring.current)).update(`${AAD}|fingerprint`).digest();
  return `${keyring.current}:${createHmac('sha256', derived).update(`${uid}\n${secret}`).digest('base64url').slice(0, 22)}`;
}

/** What a list may show of a key: a short known prefix and the last four characters. */
export function keyHint(secret) {
  if (secret.length < 12) return `…${secret.slice(-2)}`;
  const prefix = /^(sk-ant-|sk-or-|sk-|xai-|AIza|gho_)/u.exec(secret)?.[0] ?? '';
  return `${prefix}…${secret.slice(-4)}`;
}

const reserved = new Set(['builtin', 'claude', '__proto__', 'prototype', 'constructor']);
export const providerId = z.string().regex(/^[a-z0-9][a-z0-9._-]{0,63}$/u).refine(value => !reserved.has(value));
const text = max => z.string().trim().min(1).max(max).refine(value => !/[\u0000-\u001f\u007f]/u.test(value));
const apiKey = z.string().trim().min(1).max(4096).refine(value => !/[\s\u0000-\u001f\u007f]/u.test(value));
const modelId = text(200);
/** A base URL the proxy may call: https, a host name (no IP literal, no credentials, no query). DNS is checked per call. */
export const baseUrl = z.string().trim().max(512).refine(value => {
  let url;
  try { url = new URL(value); } catch { return false; }
  const host = url.hostname.toLowerCase();
  return url.protocol === 'https:' && !url.username && !url.password && !url.search && !url.hash && (!url.port || url.port === '443')
    && host.includes('.') && !/^[\d.]+$/u.test(host) && !host.startsWith('[') && !/(^|\.)(localhost|internal|local)$/u.test(host);
});
export const upsertInput = z.object({
  action: z.literal('upsert'), id: providerId, presetId: z.string().max(64).optional(), label: text(80).optional(),
  protocol: z.enum([...WEB_PROTOCOLS, 'other']).optional(), desktopType: z.enum(DESKTOP_TYPES).optional(),
  baseUrl: baseUrl.optional(), models: z.array(z.object({ id: modelId, name: text(120).optional() }).strict()).max(100).optional(),
  defaultModel: modelId.optional(), apiKey: apiKey.optional(), expectedRevision: z.number().int().min(0).optional(),
}).strict();
export const removeInput = z.object({ action: z.literal('delete'), id: providerId, expectedRevision: z.number().int().min(0).optional() }).strict();
export const exportInput = z.object({ ids: z.array(providerId).min(1).max(PROVIDER_LIMIT).optional() }).strict();

const millis = value => value instanceof Timestamp ? value.toMillis() : value instanceof Date ? value.getTime() : null;
/** The part of a record any signed-in client of its owner may see. */
export function metadataOf(id, data) {
  return { id, presetId: data.presetId ?? null, label: data.label, protocol: data.protocol, desktopType: data.desktopType ?? null,
    baseUrl: data.baseUrl, models: data.models ?? [], defaultModel: data.defaultModel ?? null, keyHint: data.keyHint,
    keyFingerprint: data.keyFingerprint, source: data.source, revision: data.revision,
    webUsable: WEB_PROTOCOLS.includes(data.protocol), createdAt: millis(data.createdAt), updatedAt: millis(data.updatedAt) };
}

/** The day and minute windows a limit counts in (UTC). */
const windows = now => ({ day: new Date(now).toISOString().slice(0, 10), minute: Math.floor(now / 60000) });

export class VaultStore {
  constructor(db, keyring, clock = Date.now) { this.db = db; this.keyring = keyring; this.clock = clock; }
  root(uid) { return this.db.doc(`llmVault/${uid}`); }
  ref(uid, id) { return this.db.doc(`llmVault/${uid}/providers/${id}`); }
  ring() { return typeof this.keyring === 'function' ? this.keyring() : this.keyring; }

  async list(uid) {
    const snapshot = await this.root(uid).collection('providers').orderBy('createdAt').get();
    return snapshot.docs.map(doc => metadataOf(doc.id, doc.data()));
  }

  /** Adds or changes a provider. A new one needs a key, and so does a new endpoint; any other change keeps the stored key. */
  async upsert(uid, input, source) {
    const ring = this.ring(), now = new Date(this.clock());
    const preset = input.presetId === undefined ? undefined : presetOf(input.presetId) ?? fail(400, 'UNKNOWN_PRESET');
    const sealedKey = input.apiKey === undefined ? undefined
      : { sealed: seal(ring, uid, input.id, input.apiKey), keyHint: keyHint(input.apiKey), keyFingerprint: keyFingerprint(ring, uid, input.apiKey) };
    return this.db.runTransaction(async tx => {
      const ref = this.ref(uid, input.id), current = await tx.get(ref), old = current.data();
      if (input.expectedRevision !== undefined && (old?.revision ?? 0) !== input.expectedRevision) fail(409, 'REVISION_CONFLICT');
      if (!old) {
        if (!sealedKey) fail(400, 'KEY_REQUIRED');
        const count = (await tx.get(this.root(uid).collection('providers').count())).data().count;
        if (count >= PROVIDER_LIMIT) fail(409, 'PROVIDER_LIMIT');
      }
      const protocol = input.protocol ?? preset?.protocol ?? old?.protocol;
      const base = input.baseUrl ?? preset?.baseUrl ?? old?.baseUrl;
      if (!protocol || !base) fail(400, 'PROVIDER_INCOMPLETE');
      // A stored key is only ever sent where it was given for, so a change of protocol or base URL must bring the key again.
      if (old && !sealedKey && (protocol !== old.protocol || base !== old.baseUrl)) fail(400, 'KEY_REQUIRED');
      if (protocol === 'other' && !(input.desktopType ?? old?.desktopType)) fail(400, 'PROVIDER_INCOMPLETE');
      const models = input.models ?? old?.models ?? preset?.models ?? [];
      const data = {
        presetId: input.presetId ?? old?.presetId ?? null, label: input.label ?? old?.label ?? preset?.label ?? input.id,
        protocol, desktopType: input.desktopType ?? old?.desktopType ?? null, baseUrl: base, models,
        defaultModel: input.defaultModel ?? old?.defaultModel ?? preset?.defaultModel ?? models[0]?.id ?? null,
        ...(sealedKey ?? {}), source: old && !sealedKey ? old.source : source, revision: (old?.revision ?? 0) + 1,
        createdAt: old?.createdAt ?? now, updatedAt: now,
      };
      tx.set(ref, { ...old, ...data });
      tx.set(this.root(uid).collection('audit').doc(), { action: old ? 'update' : 'create', providerId: input.id, client: source,
        keyChanged: !!sealedKey, at: now, expireAt: new Date(now.getTime() + 400 * 86400000) });
      return metadataOf(input.id, { ...old, ...data });
    });
  }

  async remove(uid, input, source) {
    const now = new Date(this.clock());
    await this.db.runTransaction(async tx => {
      const ref = this.ref(uid, input.id), current = await tx.get(ref);
      if (!current.exists) return;
      if (input.expectedRevision !== undefined && current.get('revision') !== input.expectedRevision) fail(409, 'REVISION_CONFLICT');
      tx.delete(ref);
      tx.set(this.root(uid).collection('audit').doc(), { action: 'delete', providerId: input.id, client: source, at: now,
        expireAt: new Date(now.getTime() + 400 * 86400000) });
    });
    return { ok: true };
  }

  /** The record and its key, for one call of the proxy. The caller keeps the key in a local and drops it. */
  async openKey(uid, id) {
    const snapshot = await this.ref(uid, id).get();
    if (!snapshot.exists) fail(404, 'PROVIDER_NOT_FOUND');
    const data = snapshot.data();
    return { provider: metadataOf(id, data), apiKey: open(this.ring(), uid, id, data.sealed) };
  }

  /** Plaintext keys for a native client, each export audited. */
  async exportKeys(uid, ids, client) {
    const providers = ids ? ids.map(id => this.ref(uid, id)) : (await this.root(uid).collection('providers').listDocuments());
    const snapshots = providers.length ? await this.db.getAll(...providers) : [];
    const now = new Date(this.clock()), batch = this.db.batch(), ring = this.ring(), out = [];
    for (const snapshot of snapshots) {
      if (!snapshot.exists) continue;
      const data = snapshot.data();
      out.push({ ...metadataOf(snapshot.id, data), apiKey: open(ring, uid, snapshot.id, data.sealed) });
      batch.set(this.root(uid).collection('audit').doc(), { action: 'export', providerId: snapshot.id, client, at: now,
        expireAt: new Date(now.getTime() + 400 * 86400000) });
    }
    if (out.length) await batch.commit();
    return out;
  }

  /** Counts one use of `kind` against per-minute and per-day limits, or throws RATE_LIMITED without counting it. */
  async take(uid, kind, { perMinute = Infinity, perDay = Infinity }) {
    const { day, minute } = windows(this.clock());
    await this.db.runTransaction(async tx => {
      const ref = this.root(uid), counters = (await tx.get(ref)).get(`limits.${kind}`) ?? {};
      const dayCount = counters.day === day ? counters.dayCount : 0, minuteCount = counters.minute === minute ? counters.minuteCount : 0;
      if (dayCount >= perDay || minuteCount >= perMinute) fail(429, 'RATE_LIMITED');
      tx.set(ref, { limits: { [kind]: { day, dayCount: dayCount + 1, minute, minuteCount: minuteCount + 1 } },
        updatedAt: FieldValue.serverTimestamp() }, { merge: true });
    });
  }
}
