import { createHash } from 'node:crypto';
import { FieldValue } from 'firebase-admin/firestore';
import { SharedBotError } from './shared-bots.mjs';

/**
 * Files in a Team message (detailed design 19). A member uploads a file for one Chat in parts, the service checks the whole
 * of it, and a message then carries it. The bytes are never public: every preview, download and model pull goes through
 * `readAttachmentPart`, which is answered only to someone who reads that Chat now.
 *
 * `teams/{team}/uploads/{uploadId}` is the record (`uploading → verified → attached`, or `failed`, or `revoked`), and
 * `…/parts/{index}` its bytes in base64. Clients read neither directly.
 */
export const attachmentLimits = { partBytes: 700 * 1024, fileBytes: 10 * 1024 * 1024, messageFiles: 5, messageBytes: 20 * 1024 * 1024 };
export const attachmentTypes = ['image/png', 'image/jpeg', 'application/pdf', 'text/plain'];
/** What one member may hold that no message carries yet, and for how long. */
const openUploads = 10, openBytes = 40 * 1024 * 1024, uploadMs = 24 * 60 * 60 * 1000;
const { partBytes } = attachmentLimits;

const deny = (status, code) => { throw new SharedBotError(status, code); };
const sameMember = (left, right) => left.uid === right.uid && left.generation === right.generation;
const partRefs = (ref, upload) => Array.from({ length: upload.parts }, (_, index) => ref.collection('parts').doc(String(index)));
const facts = u => ({ conversationId: u.conversationId, name: u.name, mime: u.mime, byteLength: u.byteLength, sha256: u.sha256 });
const descriptor = u => ({ attachmentId: u.uploadId, name: u.name, mime: u.mime, byteLength: u.byteLength, sha256: u.sha256 });
const pending = u => u.state === 'uploading' || u.state === 'verified';
/** An upload nobody attached in time counts as failed, whatever was stored. */
const view = (u, now) => {
  const state = pending(u) && u.expiresAt <= now ? 'failed' : u.state, code = state !== u.state ? 'UPLOAD_EXPIRED' : u.code;
  return { uploadId: u.uploadId, state, parts: u.parts, partBytes, ...(pending(u) ? { expiresAt: u.expiresAt } : {}), ...(state === 'failed' && code ? { code } : {}) };
};
function remove(tx, ref, upload) { for (const part of partRefs(ref, upload)) tx.delete(part); tx.delete(ref); }
/** The first bytes say what a file is; its name and the type the sender gave do not. */
function typed(mime, head) {
  if (mime === 'image/png') return head.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'));
  if (mime === 'image/jpeg') return head.subarray(0, 3).equals(Buffer.from('ffd8ff', 'hex'));
  return mime === 'application/pdf' && head.subarray(0, 5).toString('latin1') === '%PDF-';
}

export async function createUpload(store, { tx, team, identity, command: { payload: p }, now }) {
  await store.conversation(tx, team, p.conversationId, identity, true);
  const uploads = team.collection('uploads'), ref = uploads.doc(p.uploadId);
  const existing = (await tx.get(ref)).data(), mine = await tx.get(uploads.where('openBy', '==', identity.uid).limit(openUploads * 4));
  if (existing) {
    if (!sameMember(existing.uploader, identity) || JSON.stringify(facts(existing)) !== JSON.stringify(facts(p))) deny(409, 'UPLOAD_ID_CONFLICT');
    return view(existing, now);
  }
  let count = 0, bytes = 0;
  for (const doc of mine.docs) {
    const upload = doc.data();
    // What ran out of time is released here, by the member's next upload, and by the daily sweep.
    if (upload.expiresAt <= now) remove(tx, doc.ref, upload); else { count += 1; bytes += upload.byteLength; }
  }
  if (count >= openUploads || bytes + p.byteLength > openBytes) deny(429, 'UPLOAD_QUOTA_EXCEEDED');
  const upload = { schemaVersion: 1, uploadId: p.uploadId, ...facts(p), uploader: identity, parts: Math.ceil(p.byteLength / partBytes),
    state: 'uploading', openBy: identity.uid, createdAt: now, expiresAt: now + uploadMs };
  tx.create(ref, upload);
  return view(upload, now);
}

async function own(tx, team, identity, uploadId) {
  const ref = team.collection('uploads').doc(uploadId), upload = (await tx.get(ref)).data();
  if (!upload || !sameMember(upload.uploader, identity)) deny(404, 'UPLOAD_REQUIRED');
  return { ref, upload };
}

export async function putUploadPart(_store, { tx, team, identity, command: { payload: p }, now }) {
  const { ref, upload } = await own(tx, team, identity, p.uploadId);
  if (upload.state !== 'uploading') deny(409, 'UPLOAD_SEALED');
  if (upload.expiresAt <= now) deny(410, 'UPLOAD_EXPIRED');
  const expected = p.index < upload.parts - 1 ? partBytes : upload.byteLength - partBytes * (upload.parts - 1);
  if (p.index >= upload.parts || Buffer.from(p.data, 'base64').length !== expected) deny(400, 'UPLOAD_PART_INVALID');
  tx.set(ref.collection('parts').doc(String(p.index)), { index: p.index, data: p.data, at: now });
  return { index: p.index };
}

/** The whole file is read back and checked: its length, its digest, and that it is the kind of file it says. Asked again, the answer is the same. */
export async function finalizeUpload(_store, { tx, team, identity, command: { payload: p }, now }) {
  const { ref, upload } = await own(tx, team, identity, p.uploadId);
  if (upload.state !== 'uploading') return view(upload, now);
  if (upload.expiresAt <= now) deny(410, 'UPLOAD_EXPIRED');
  const digest = createHash('sha256'), text = upload.mime === 'text/plain' ? new TextDecoder('utf-8', { fatal: true }) : null;
  let length = 0, head = Buffer.alloc(0), readable = true;
  for (const part of partRefs(ref, upload)) {
    const value = (await tx.get(part)).data();
    if (!value) deny(409, 'UPLOAD_INCOMPLETE');
    const bytes = Buffer.from(value.data, 'base64');
    digest.update(bytes); length += bytes.length;
    if (!head.length) head = bytes.subarray(0, 8);
    if (text && readable) try { text.decode(bytes, { stream: true }); readable = !bytes.includes(0); } catch { readable = false; }
  }
  if (text && readable) try { text.decode(); } catch { readable = false; }
  const code = length !== upload.byteLength || digest.digest('hex') !== upload.sha256 ? 'UPLOAD_DIGEST_MISMATCH'
    : (text ? readable : typed(upload.mime, head)) ? null : 'UPLOAD_TYPE_MISMATCH';
  if (code) {
    for (const part of partRefs(ref, upload)) tx.delete(part);
    tx.update(ref, { state: 'failed', code, openBy: FieldValue.delete(), expiresAt: FieldValue.delete() });
    return view({ ...upload, state: 'failed', code }, now);
  }
  tx.update(ref, { state: 'verified', verifiedAt: now });
  return view({ ...upload, state: 'verified' }, now);
}

/** Taking a file out of a draft. Nothing is left of it; a file a message already carries is revoked instead. */
export async function cancelUpload(_store, { tx, team, identity, command: { payload: p } }) {
  const ref = team.collection('uploads').doc(p.uploadId), upload = (await tx.get(ref)).data();
  if (!upload) return { uploadId: p.uploadId, state: 'cancelled' };
  if (!sameMember(upload.uploader, identity)) deny(404, 'UPLOAD_REQUIRED');
  if (upload.state === 'attached' || upload.state === 'revoked') deny(409, 'UPLOAD_SEALED');
  remove(tx, ref, upload);
  return { uploadId: p.uploadId, state: 'cancelled' };
}

/**
 * The files a new message may carry: checked files of this sender, uploaded for this Chat, that no other message carries.
 * Reads only; `attach` writes once the message is written in the same transaction.
 */
export async function attachable(tx, team, identity, conversationId, uploadIds, now) {
  if (!uploadIds?.length) return { attachments: [], attach() {} };
  const refs = uploadIds.map(id => team.collection('uploads').doc(id)), uploads = (await tx.getAll(...refs)).map(doc => doc.data());
  for (const upload of uploads) {
    if (!upload || !sameMember(upload.uploader, identity) || upload.conversationId !== conversationId || upload.state !== 'verified') deny(409, 'ATTACHMENT_REQUIRED');
    if (upload.expiresAt <= now) deny(410, 'UPLOAD_EXPIRED');
  }
  if (uploads.reduce((sum, upload) => sum + upload.byteLength, 0) > attachmentLimits.messageBytes) deny(413, 'ATTACHMENT_LIMIT');
  return { attachments: uploads.map(descriptor), attach(messageId) {
    for (const ref of refs) tx.update(ref, { state: 'attached', messageId, attachedAt: now, openBy: FieldValue.delete(), expiresAt: FieldValue.delete() });
  } };
}

/** One part of a file in a message, for someone who reads its Chat now. Never answered from an earlier answer. */
export async function readAttachmentPart(store, { tx, team, identity, command: { payload: p } }) {
  const ref = team.collection('uploads').doc(p.attachmentId), upload = (await tx.get(ref)).data();
  if (!upload || (upload.state !== 'attached' && upload.state !== 'revoked')) deny(404, 'ATTACHMENT_REQUIRED');
  const chat = await store.conversation(tx, team, upload.conversationId, identity);
  const message = (await tx.get(chat.ref.collection('messages').doc(upload.messageId))).data();
  // A recalled message takes its files with it, however it was recalled.
  if (upload.state === 'revoked' || !message || message.recalled) deny(410, 'ATTACHMENT_REVOKED');
  if (p.index >= upload.parts) deny(400, 'UPLOAD_PART_INVALID');
  const part = (await tx.get(ref.collection('parts').doc(String(p.index)))).data();
  if (!part) deny(410, 'ATTACHMENT_REVOKED');
  return { attachmentId: p.attachmentId, index: p.index, parts: upload.parts, data: part.data };
}

/**
 * The sender takes a file back out of a message. The bytes are deleted and the message says so, as a change like an edit,
 * so every reader's copy of the message follows. Asked again, nothing more happens.
 */
export async function revokeAttachment(store, { tx, team, identity, command: { payload: p }, now }) {
  const ref = team.collection('uploads').doc(p.attachmentId), upload = (await tx.get(ref)).data();
  if (!upload || (upload.state !== 'attached' && upload.state !== 'revoked')) deny(404, 'ATTACHMENT_REQUIRED');
  const chat = await store.conversation(tx, team, upload.conversationId, identity, true);
  if (upload.uploader.uid !== identity.uid) deny(403, 'MESSAGE_AUTHOR_REQUIRED');
  const messageRef = chat.ref.collection('messages').doc(upload.messageId), changeRef = chat.ref.collection('changes').doc(p.changeId);
  const [message, change] = (await tx.getAll(messageRef, changeRef)).map(doc => doc.data());
  if (change && (change.messageId !== upload.messageId || change.actorUid !== identity.uid || upload.state !== 'revoked')) deny(409, 'MESSAGE_ID_CONFLICT');
  if (upload.state === 'revoked') return { attachmentId: p.attachmentId, state: 'revoked' };
  for (const part of partRefs(ref, upload)) tx.delete(part);
  tx.update(ref, { state: 'revoked', revokedAt: now });
  if (message && !message.recalled) {
    const changeSeq = chat.value.lastChangeSeq + 1, at = new Date(now);
    if (!Number.isSafeInteger(changeSeq)) deny(409, 'CURSOR_LIMIT');
    tx.update(messageRef, { attachments: message.attachments.map(a => a.attachmentId === p.attachmentId ? { ...a, revoked: true } : a), changeSeq, changeId: p.changeId });
    tx.create(changeRef, { seq: changeSeq, messageId: upload.messageId, operation: 'edit', actorUid: identity.uid, generation: identity.generation,
      textDigest: createHash('sha256').update(message.text, 'utf8').digest('hex'), at });
    tx.update(chat.ref, { lastChangeSeq: changeSeq, lastChange: { id: p.changeId, seq: changeSeq } });
  }
  return { attachmentId: p.attachmentId, state: 'revoked' };
}

/** A message recalled through the service gives up its files at once. Reads first; the returned function writes. */
export async function recallAttachments(tx, team, message, now) {
  const live = (message.attachments ?? []).filter(a => !a.revoked);
  if (!live.length) return () => {};
  const refs = live.map(a => team.collection('uploads').doc(a.attachmentId)), uploads = (await tx.getAll(...refs)).map(doc => doc.data());
  return () => { for (const [index, upload] of uploads.entries()) if (upload?.state === 'attached') {
    for (const part of partRefs(refs[index], upload)) tx.delete(part);
    tx.update(refs[index], { state: 'revoked', revokedAt: now });
  } };
}

/** Uploads no message took within a day, in every team; run daily. Returns how many were released. */
export async function sweepUploads(db, now = Date.now(), limit = 200) {
  const expired = await db.collectionGroup('uploads').where('expiresAt', '<=', now).limit(limit).get();
  for (const doc of expired.docs) await db.runTransaction(async tx => {
    const upload = (await tx.get(doc.ref)).data();
    if (upload && pending(upload) && upload.expiresAt <= now) remove(tx, doc.ref, upload);
  });
  return expired.size;
}
