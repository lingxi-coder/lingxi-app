import { randomUUID, createHash } from 'node:crypto';
import { ApiError, validateCatalog } from './catalog.mjs';

export class CatalogStore {
  constructor(db, seed = { schemaVersion: 2, providers: [], models: [] }) { this.db = db; this.seed = validateCatalog(seed); }
  draftRef() { return this.db.doc('catalogV2State/draft'); }
  async payload(id) {
    const root = await this.db.doc(`catalogV2Payloads/${id}`).get();
    if (!root.exists) throw new ApiError(503, 'PAYLOAD_MISSING');
    const { parts, digest } = root.data();
    if (!Number.isInteger(parts) || parts < 1 || parts > 100) throw new ApiError(503, 'PAYLOAD_INVALID');
    const records = await this.db.getAll(...Array.from({ length: parts }, (_, i) => this.db.doc(`catalogV2Parts/${id}-${i}`)));
    const json = records.map((p) => { if (!p.exists) throw new ApiError(503, 'PAYLOAD_INCOMPLETE'); return p.data().json; }).join('');
    if (createHash('sha256').update(json).digest('hex') !== digest) throw new ApiError(503, 'PAYLOAD_INVALID');
    return validateCatalog(JSON.parse(json));
  }
  writePayload(tx, catalog, user) {
    const id = randomUUID(), json = JSON.stringify(catalog), chars = Array.from(json), parts = [];
    for (let i = 0; i < chars.length; i += 100000) parts.push(chars.slice(i, i + 100000).join(''));
    parts.forEach((part, i) => tx.create(this.db.doc(`catalogV2Parts/${id}-${i}`), { json: part }));
    tx.create(this.db.doc(`catalogV2Payloads/${id}`), { parts: parts.length, digest: createHash('sha256').update(json).digest('hex'), createdAt: Date.now(), createdBy: user });
    return id;
  }
  async draft() {
    const doc = await this.draftRef().get();
    if (!doc.exists) return { revision: 0, catalog: this.seed, updatedAt: null };
    const data = doc.data(); return { revision: data.revision, updatedAt: data.updatedAt, catalog: await this.payload(data.payloadId) };
  }
  async save(catalog, expected, user) {
    catalog = validateCatalog(catalog);
    const updatedAt = Date.now();
    await this.db.runTransaction(async (tx) => {
      const doc = await tx.get(this.draftRef());
      if ((doc.data()?.revision || 0) !== expected) throw new ApiError(409, 'REVISION_CONFLICT', 'Draft changed. Reload before saving.');
      const payloadId = this.writePayload(tx, catalog, user);
      tx.set(this.draftRef(), { revision: expected + 1, payloadId, updatedAt, updatedBy: user });
    });
    return { revision: expected + 1, catalog, updatedAt };
  }
  async publish(expected, label, user) {
    const id = randomUUID(), createdAt = Date.now();
    await this.db.runTransaction(async (tx) => {
      const draft = await tx.get(this.draftRef());
      if (!draft.exists) throw new ApiError(400, 'SAVE_DRAFT_FIRST');
      if (draft.data().revision !== expected) throw new ApiError(409, 'REVISION_CONFLICT');
      const data = { payloadId: draft.data().payloadId, revision: expected, label, createdAt, createdBy: user };
      tx.create(this.db.doc(`catalogV2Releases/${id}`), data);
      tx.set(this.db.doc('catalogV2State/current'), { id, ...data });
    });
    return { id, createdAt, label, revision: expected };
  }
  async current() {
    const doc = await this.db.doc('catalogV2State/current').get();
    if (!doc.exists) return null;
    const data = doc.data(); return { id: data.id, createdAt: data.createdAt, catalog: await this.payload(data.payloadId) };
  }
  async history() {
    const q = await this.db.collection('catalogV2Releases').orderBy('createdAt', 'desc').limit(25).select('label', 'createdAt', 'revision').get();
    return q.docs.map((d) => ({ id: d.id, ...d.data() }));
  }
  async restore(id, expected, user) {
    const release = await this.db.doc(`catalogV2Releases/${id}`).get();
    if (!release.exists) throw new ApiError(404, 'RELEASE_NOT_FOUND');
    return this.save(await this.payload(release.data().payloadId), expected, user);
  }
}
