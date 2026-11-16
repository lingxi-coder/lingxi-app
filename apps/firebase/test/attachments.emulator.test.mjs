// M3-4: files in a Team message — upload, check, send, read, revoke (acceptance L2–L4, service side).
import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { attachmentLimits, sweepUploads } from '../src/team-attachments.mjs';
import { sharedBotCommand } from '../src/shared-bot-commands.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 120000 };
const projectId = 'demo-lingxi-teams-execution', teamId = 'team_attachments', botId = 'sharedbot_01', chat = 'chat_group_01', botChat = 'chat_botdm_01', otherChat = 'chat_other_01';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
let app, db, teams, store, messages, now, serial = 0;
const sha = value => createHash('sha256').update(value).digest('hex');
const command = (action, payload, commandId = `command_${++serial}`, generation = 1) => ({ schemaVersion: 1, commandId, teamId, generation, action, payload });
const execute = (action, payload, actor = member, commandId, generation) => store.execute(actor, command(action, payload, commandId, generation));
const png = size => Buffer.concat([Buffer.from('89504e470d0a1a0a', 'hex'), randomBytes(size - 8)]);
const pdf = size => Buffer.concat([Buffer.from('%PDF-1.7\n'), randomBytes(size - 9)]);
const slices = bytes => Array.from({ length: Math.ceil(bytes.length / attachmentLimits.partBytes) }, (_, index) =>
  bytes.subarray(index * attachmentLimits.partBytes, (index + 1) * attachmentLimits.partBytes).toString('base64'));
const facts = (bytes, more = {}) => ({ conversationId: chat, uploadId: `upload_${++serial}`, name: 'photo.png', mime: 'image/png', byteLength: bytes.length, sha256: sha(bytes), ...more });
/** A member's whole upload of one file, to the point where a message can carry it. */
async function upload(bytes, more = {}, actor = member) {
  const file = facts(bytes, more), created = await execute('createUpload', file, actor);
  assert.deepEqual([created.state, created.parts, created.partBytes], ['uploading', slices(bytes).length, attachmentLimits.partBytes]);
  for (const [index, data] of slices(bytes).entries()) await execute('putUploadPart', { uploadId: file.uploadId, index, data }, actor);
  return { ...file, result: await execute('finalizeUpload', { uploadId: file.uploadId }, actor) };
}
async function download(attachmentId, actor) {
  const first = await execute('readAttachmentPart', { attachmentId, index: 0 }, actor), parts = [first.data];
  for (let index = 1; index < first.parts; index += 1) parts.push((await execute('readAttachmentPart', { attachmentId, index }, actor)).data);
  return Buffer.concat(parts.map(part => Buffer.from(part, 'base64')));
}
const send = (attachments, more = {}, actor = member, id = ++serial) => execute('submit',
  { conversationId: chat, messageId: `message_${id}`, changeId: `change_${id}`, text: '', targets: [], attachments, ...more }, actor);
const messageData = async (id, conversationId = chat) => (await db.doc(`teams/${teamId}/conversations/${conversationId}/messages/${id}`).get()).data();
const chatData = async (conversationId = chat) => (await db.doc(`teams/${teamId}/conversations/${conversationId}`).get()).data();
const uploadData = async id => (await db.doc(`teams/${teamId}/uploads/${id}`).get()).data();
const partCount = async id => (await db.collection(`teams/${teamId}/uploads/${id}/parts`).get()).size;
const publish = () => ({ sharedBotId: botId, expectedRevision: 0, name: 'Travel assistant', description: 'Shared travel notes', usePolicy: { mode: 'all' },
  deployment: { ...host, sharedBotId: botId, revision: 1, owner: { uid: owner.uid, generation: 1 }, instanceId: 'instance_owner', definitionDigest: 'a'.repeat(64),
    capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2, capabilities: { durableWait: false, writeOperations: true, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: sha('persona_01'), sourceAudience: { mode: 'team' } }], quotas: { team: 100, member: 20, bot: 100 } });

test('attachments: the daily sweep has its index, and the bytes are not indexed', () => {
  const indexes = JSON.parse(readFileSync(new URL('../../../infra/firebase/firestore.teams.indexes.json', import.meta.url), 'utf8')).fieldOverrides;
  const of = (group, field) => indexes.find(item => item.collectionGroup === group && item.fieldPath === field)?.indexes.map(index => index.queryScope).sort();
  assert.deepEqual([of('uploads', 'expiresAt'), of('parts', 'data'), of('transferParts', 'data')], [['COLLECTION', 'COLLECTION_GROUP'], [], []]);
});

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'team-attachment-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  await db.recursiveDelete(db.collection('teams')); await db.recursiveDelete(db.collection('users'));
  now = Date.UTC(2026, 10, 13); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Attachment test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  // `other` is in the team and not in this Chat.
  await messages.execute(owner, { action: 'create', teamId, generation: 1, conversationId: chat, kind: 'group', title: 'Trip', participantUids: [owner.uid, member.uid] });
  await messages.execute(member, { action: 'create', teamId, generation: 1, conversationId: otherChat, kind: 'group', title: 'Other', participantUids: [member.uid, other.uid] });
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('attachments: a picture and a document are uploaded in parts, sent, and read back whole by each reader of the Chat and by nobody else (L2)', options, async () => {
  const picture = png(attachmentLimits.partBytes * 2 + 4321), notes = Buffer.from('Flight CA 981, 9 March, 13:05, gate 12\n集合地点：三号航站楼', 'utf8');
  const first = await upload(picture), second = await upload(notes, { name: '行程 notes.txt', mime: 'text/plain' });
  assert.deepEqual([first.result.state, second.result.state, first.result.parts, second.result.parts], ['verified', 'verified', 3, 1]);
  // Before a message carries it, a file is nobody's to read, its sender included: the sender has it on their own device.
  await assert.rejects(execute('readAttachmentPart', { attachmentId: first.uploadId, index: 0 }, member), { code: 'ATTACHMENT_REQUIRED' });
  const sent = await send([first.uploadId, second.uploadId], { text: 'Boarding passes' });
  assert.deepEqual(sent.tasks, []);
  const message = await messageData(sent.messageId);
  assert.deepEqual(message.attachments, [
    { attachmentId: first.uploadId, name: 'photo.png', mime: 'image/png', byteLength: picture.length, sha256: sha(picture) },
    { attachmentId: second.uploadId, name: '行程 notes.txt', mime: 'text/plain', byteLength: notes.length, sha256: sha(notes) }]);
  assert.deepEqual([message.kind, message.text, 'taskIds' in message, 'mentions' in message], ['text', 'Boarding passes', false, false], 'a message that asks no Bot looks like any other');
  assert.deepEqual([(await uploadData(first.uploadId)).state, (await uploadData(first.uploadId)).messageId, 'expiresAt' in await uploadData(first.uploadId)], ['attached', sent.messageId, false]);
  // The other reader, and the sender on a second device: the same bytes, every time through the service.
  for (const reader of [owner, member]) {
    assert.equal(sha(await download(first.uploadId, reader)), sha(picture));
    assert.equal((await download(second.uploadId, reader)).toString('utf8'), notes.toString('utf8'));
  }
  // A member of the team who does not read this Chat, and someone outside the team.
  await assert.rejects(execute('readAttachmentPart', { attachmentId: first.uploadId, index: 0 }, other), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(execute('readAttachmentPart', { attachmentId: first.uploadId, index: 0 }, { uid: 'stranger' }), { code: 'MEMBERSHIP_REQUIRED' });
  await assert.rejects(execute('readAttachmentPart', { attachmentId: first.uploadId, index: 3 }, owner), { code: 'UPLOAD_PART_INVALID' });
  await assert.rejects(execute('readAttachmentPart', { attachmentId: 'upload_unknown', index: 0 }, owner), { code: 'ATTACHMENT_REQUIRED' });
  // Files alone are a message too, listed by their names.
  const alone = await send([(await upload(pdf(2048), { name: 'tickets.pdf', mime: 'application/pdf' })).uploadId]);
  assert.deepEqual([(await messageData(alone.messageId)).text, (await chatData()).lastMessage.preview, (await chatData()).lastSeq], ['', 'tickets.pdf', 2]);
});

test('attachments: a file that is not what it says, is cut short or was changed on the way is refused, and its bytes are not kept (L3)', options, async () => {
  const picture = png(5000);
  // Called a JPEG, a text file that is not text, a text file with a NUL.
  for (const [bytes, more] of [[picture, { mime: 'image/jpeg', name: 'photo.jpg' }], [Buffer.from([0xff, 0xfe, 0xfd, 0x80]), { mime: 'text/plain', name: 'a.txt' }],
    [Buffer.from('a\u0000b'), { mime: 'text/plain', name: 'b.txt' }], [Buffer.from('not a pdf'), { mime: 'application/pdf', name: 'c.pdf' }]]) {
    const refused = await upload(bytes, more);
    assert.deepEqual([refused.result.state, refused.result.code, await partCount(refused.uploadId)], ['failed', 'UPLOAD_TYPE_MISMATCH', 0]);
    await assert.rejects(send([refused.uploadId]), { code: 'ATTACHMENT_REQUIRED' });
  }
  // Not the bytes that were announced.
  const changed = facts(picture, { sha256: sha('something else') });
  await execute('createUpload', changed); await execute('putUploadPart', { uploadId: changed.uploadId, index: 0, data: picture.toString('base64') });
  assert.deepEqual((({ state, code }) => [state, code])(await execute('finalizeUpload', { uploadId: changed.uploadId })), ['failed', 'UPLOAD_DIGEST_MISMATCH']);
  // A part of the wrong size never gets in, and a missing part is not a failure: the upload goes on.
  const large = png(attachmentLimits.partBytes + 10), file = facts(large);
  await execute('createUpload', file);
  await assert.rejects(execute('putUploadPart', { uploadId: file.uploadId, index: 0, data: large.subarray(0, 100).toString('base64') }), { code: 'UPLOAD_PART_INVALID' });
  await assert.rejects(execute('putUploadPart', { uploadId: file.uploadId, index: 2, data: large.subarray(0, 12).toString('base64') }), { code: 'UPLOAD_PART_INVALID' });
  await execute('putUploadPart', { uploadId: file.uploadId, index: 1, data: slices(large)[1] });
  await assert.rejects(execute('finalizeUpload', { uploadId: file.uploadId }), { code: 'UPLOAD_INCOMPLETE' });
  await assert.rejects(send([file.uploadId]), { code: 'ATTACHMENT_REQUIRED' }, 'an unchecked file cannot be sent');
  await assert.rejects(execute('putUploadPart', { uploadId: file.uploadId, index: 0, data: slices(large)[0] }, owner), { code: 'UPLOAD_REQUIRED' }, 'only its sender adds to it');
  await execute('putUploadPart', { uploadId: file.uploadId, index: 0, data: slices(large)[0] });
  assert.equal((await execute('finalizeUpload', { uploadId: file.uploadId })).state, 'verified');
  await assert.rejects(execute('putUploadPart', { uploadId: file.uploadId, index: 0, data: slices(large)[0] }), { code: 'UPLOAD_SEALED' });
  // What the service will not take at all: other kinds of file, more than 10 MiB, a path for a name, six files in a message.
  const parses = (action, payload) => sharedBotCommand.safeParse(command(action, payload)).success;
  assert.equal(parses('createUpload', facts(picture)), true);
  for (const more of [{ mime: 'image/gif' }, { mime: 'application/zip' }, { byteLength: attachmentLimits.fileBytes + 1 }, { byteLength: 0 }, { name: '../photo.png' }, { name: 'a\\b.png' }, { name: '' }])
    assert.equal(parses('createUpload', facts(picture, more)), false, JSON.stringify(more));
  const message = { conversationId: chat, messageId: 'message_x1', changeId: 'change_x01', text: '', targets: [] };
  assert.equal(parses('submit', { ...message, attachments: ['upload_a1', 'upload_a2', 'upload_a3', 'upload_a4', 'upload_a5'] }), true);
  assert.equal(parses('submit', { ...message, attachments: ['upload_a1', 'upload_a2', 'upload_a3', 'upload_a4', 'upload_a5', 'upload_a6'] }), false);
  assert.equal(parses('submit', { ...message, attachments: ['upload_a1', 'upload_a1'] }), false);
  assert.equal(parses('submit', message), false, 'neither text for a Bot nor a file');
  assert.equal(parses('submit', { ...message, text: 'hello' }), false, 'an ordinary message is not sent this way');
});

test('attachments: files of more than 20 MiB together do not go into one message (L3)', options, async () => {
  // Checked uploads written directly: the sum is what is tested here, not 24 MiB of emulator traffic.
  const ids = ['upload_big_1', 'upload_big_2', 'upload_big_3'];
  for (const uploadId of ids) await db.doc(`teams/${teamId}/uploads/${uploadId}`).set({ schemaVersion: 1, uploadId, conversationId: chat, name: `${uploadId}.pdf`, mime: 'application/pdf',
    byteLength: 8 * 1024 * 1024, sha256: sha(uploadId), uploader: { uid: member.uid, generation: 1 }, parts: 12, state: 'verified', openBy: member.uid, createdAt: now, expiresAt: now + 3600000 });
  await assert.rejects(send(ids), { code: 'ATTACHMENT_LIMIT' });
  assert.deepEqual((await send(ids.slice(0, 2))).tasks, []);
  assert.equal((await chatData()).lastSeq, 1);
});

test('attachments: every step asked again gives the same answer, and one file goes into one message once (L3)', options, async () => {
  const picture = png(3000), file = facts(picture);
  const created = await execute('createUpload', file, member, 'command_create1');
  assert.deepEqual(await execute('createUpload', file, member, 'command_create1'), created, 'the same command');
  assert.deepEqual(await execute('createUpload', file, member, 'command_create2'), created, 'the same upload under a new command');
  await assert.rejects(execute('createUpload', { ...file, name: 'other.png' }), { code: 'UPLOAD_ID_CONFLICT' });
  await assert.rejects(execute('createUpload', file, owner), { code: 'UPLOAD_ID_CONFLICT' }, 'an id belongs to the member who took it');
  for (let i = 0; i < 2; i += 1) assert.deepEqual(await execute('putUploadPart', { uploadId: file.uploadId, index: 0, data: picture.toString('base64') }), { index: 0 });
  const checked = await execute('finalizeUpload', { uploadId: file.uploadId }, member, 'command_final01');
  assert.deepEqual(await execute('finalizeUpload', { uploadId: file.uploadId }, member, 'command_final01'), checked);
  assert.deepEqual(await execute('finalizeUpload', { uploadId: file.uploadId }, member, 'command_final02'), checked);
  assert.equal(await partCount(file.uploadId), 1);
  // Someone else's file, a file uploaded for another Chat, and a file that is already in a message.
  await assert.rejects(send([file.uploadId], {}, owner), { code: 'ATTACHMENT_REQUIRED' });
  await assert.rejects(send([file.uploadId], { conversationId: otherChat }), { code: 'ATTACHMENT_REQUIRED' });
  const payload = { conversationId: chat, messageId: 'message_once', changeId: 'change_once', text: 'One', targets: [], attachments: [file.uploadId] };
  const sent = await execute('submit', payload, member, 'command_send01');
  assert.deepEqual(await execute('submit', payload, member, 'command_send01'), sent, 'a repeated tap');
  assert.deepEqual(await execute('submit', payload, member, 'command_send02'), sent, 'the same message sent again after a lost answer');
  await assert.rejects(execute('submit', { ...payload, text: 'Two' }, member), { code: 'MESSAGE_ID_CONFLICT' });
  await assert.rejects(send([file.uploadId]), { code: 'ATTACHMENT_REQUIRED' }, 'not into a second message');
  assert.deepEqual([(await chatData()).lastSeq, (await db.collection(`teams/${teamId}/conversations/${chat}/messages`).get()).size], [1, 1]);
  await assert.rejects(execute('cancelUpload', { uploadId: file.uploadId }), { code: 'UPLOAD_SEALED' }, 'a sent file is revoked, not cancelled');
});

test('attachments: what no message took is limited, ends after a day and is cleaned up; a file taken out of a draft is gone at once', options, async () => {
  const small = png(64), kept = [];
  for (let i = 0; i < 10; i += 1) kept.push(await upload(small));
  await assert.rejects(execute('createUpload', facts(small)), { code: 'UPLOAD_QUOTA_EXCEEDED' });
  assert.equal((await execute('createUpload', facts(small), owner)).state, 'uploading', 'one member’s drafts do not count against another');
  // Out of a draft: nothing is left, the place is free again, and asking again changes nothing.
  for (let i = 0; i < 2; i += 1) assert.deepEqual(await execute('cancelUpload', { uploadId: kept[0].uploadId }), { uploadId: kept[0].uploadId, state: 'cancelled' });
  await assert.rejects(execute('cancelUpload', { uploadId: kept[1].uploadId }, owner), { code: 'UPLOAD_REQUIRED' });
  assert.deepEqual([await uploadData(kept[0].uploadId), await partCount(kept[0].uploadId)], [undefined, 0]);
  const waiting = facts(small); await execute('createUpload', waiting);
  // A day later: none of it can be sent or added to.
  now += 24 * 60 * 60 * 1000;
  await assert.rejects(send([kept[1].uploadId]), { code: 'UPLOAD_EXPIRED' });
  await assert.rejects(execute('putUploadPart', { uploadId: waiting.uploadId, index: 0, data: small.toString('base64') }), { code: 'UPLOAD_EXPIRED' });
  await assert.rejects(execute('finalizeUpload', { uploadId: waiting.uploadId }), { code: 'UPLOAD_EXPIRED' });
  assert.deepEqual((({ state, code }) => [state, code])(await execute('finalizeUpload', { uploadId: kept[1].uploadId })), ['failed', 'UPLOAD_EXPIRED']);
  // The member's next upload releases their own; the daily sweep releases everyone's.
  assert.equal((await execute('createUpload', facts(small))).state, 'uploading');
  assert.deepEqual([await uploadData(kept[1].uploadId), await partCount(kept[1].uploadId), await uploadData(waiting.uploadId)], [undefined, 0, undefined]);
  assert.equal((await db.collection(`teams/${teamId}/uploads`).where('openBy', '==', owner.uid).get()).size, 1);
  assert.equal(await sweepUploads(db, now), 1);
  assert.equal((await db.collection(`teams/${teamId}/uploads`).where('openBy', '==', owner.uid).get()).size, 0);
  assert.equal((await db.collection(`teams/${teamId}/uploads`).get()).size, 1, 'the upload that is still in time stays');
});

test('attachments: a file taken back, a recalled message and a member who left each end reading for good (L4)', options, async () => {
  const picture = png(2000), notes = Buffer.from('notes');
  const first = await upload(picture), second = await upload(notes, { name: 'notes.txt', mime: 'text/plain' });
  const sent = await send([first.uploadId, second.uploadId], { text: 'Two files' });
  assert.equal(sha(await download(first.uploadId, owner)), sha(picture));
  // Only the sender takes a file back. The message says so as a change, the bytes are gone, the other file stays.
  await assert.rejects(execute('revokeAttachment', { attachmentId: first.uploadId, changeId: 'change_revoke1' }, owner), { code: 'MESSAGE_AUTHOR_REQUIRED' });
  await assert.rejects(execute('revokeAttachment', { attachmentId: first.uploadId, changeId: 'change_revoke1' }, other), { code: 'CONVERSATION_REQUIRED' });
  const before = await chatData();
  for (const commandId of ['command_revoke1', 'command_revoke1', 'command_revoke2'])
    assert.deepEqual(await execute('revokeAttachment', { attachmentId: first.uploadId, changeId: 'change_revoke1' }, member, commandId), { attachmentId: first.uploadId, state: 'revoked' });
  const message = await messageData(sent.messageId), after = await chatData();
  assert.deepEqual(message.attachments.map(a => [a.attachmentId, a.revoked ?? false]), [[first.uploadId, true], [second.uploadId, false]]);
  assert.deepEqual([after.lastChangeSeq, message.changeSeq, message.changeId, after.lastSeq, message.text], [before.lastChangeSeq + 1, before.lastChangeSeq + 1, 'change_revoke1', before.lastSeq, 'Two files']);
  const change = (await db.doc(`teams/${teamId}/conversations/${chat}/changes/change_revoke1`).get()).data();
  assert.deepEqual([change.operation, change.messageId, change.actorUid, change.seq], ['edit', sent.messageId, member.uid, after.lastChangeSeq]);
  assert.deepEqual([(await uploadData(first.uploadId)).state, await partCount(first.uploadId)], ['revoked', 0]);
  for (const reader of [owner, member]) await assert.rejects(execute('readAttachmentPart', { attachmentId: first.uploadId, index: 0 }, reader), { code: 'ATTACHMENT_REVOKED' });
  assert.equal((await download(second.uploadId, owner)).toString(), 'notes');
  // Recalled through the service: its files go with it.
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId: chat, messageId: sent.messageId, changeId: 'change_recall1', operation: 'recall', text: '' });
  assert.deepEqual([(await uploadData(second.uploadId)).state, await partCount(second.uploadId)], ['revoked', 0]);
  await assert.rejects(execute('readAttachmentPart', { attachmentId: second.uploadId, index: 0 }, owner), { code: 'ATTACHMENT_REVOKED' });
  // Recalled by a client's own write, which the rules allow: the service still gives nothing.
  const third = await upload(picture), again = await send([third.uploadId]);
  await db.doc(`teams/${teamId}/conversations/${chat}/messages/${again.messageId}`).update({ recalled: true, text: '' });
  await assert.rejects(execute('readAttachmentPart', { attachmentId: third.uploadId, index: 0 }, owner), { code: 'ATTACHMENT_REVOKED' });
  // A member who was removed reads nothing more, with the id they already know; back in the team as a new member they are a reader again like for the messages.
  const fourth = await upload(picture, {}, owner), kept = await send([fourth.uploadId], {}, owner);
  assert.equal(sha(await download(fourth.uploadId, member)), sha(picture));
  await teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 });
  await assert.rejects(execute('readAttachmentPart', { attachmentId: fourth.uploadId, index: 0 }, member), { code: 'MEMBERSHIP_REQUIRED' });
  assert.equal((await messageData(kept.messageId)).attachments.length, 1);
});

test('attachments: a request to a Bot carries its files, and the Bot’s maintainer reads them for the model like any reader', options, async () => {
  await execute('publishSharedBot', publish(), owner); await execute('heartbeatDeployment', { ...host, ready: true }, owner);
  await execute('createConversation', { conversationId: botChat, kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member);
  const picture = png(4096), file = await upload(picture, { conversationId: botChat });
  const asked = await send([file.uploadId], { conversationId: botChat, text: 'What does this ticket say?', targets: [botId] });
  assert.equal(asked.tasks.length, 1);
  const message = await messageData(asked.messageId, botChat);
  assert.deepEqual([message.attachments[0].sha256, message.taskIds, message.mentions], [sha(picture), [asked.tasks[0].taskId], [`shared_bot:${botId}`]]);
  assert.equal(sha(await download(file.uploadId, owner)), sha(picture), 'the maintainer’s host');
  await assert.rejects(execute('readAttachmentPart', { attachmentId: file.uploadId, index: 0 }, other), { code: 'CONVERSATION_REQUIRED' });
  // A Bot DM takes no message without its Bot, with or without a file.
  const second = await upload(picture, { conversationId: botChat });
  await assert.rejects(send([second.uploadId], { conversationId: botChat }), { code: 'INVALID_BOT_TARGET' });
  // The request counts against the quotas as before; a message of files alone counts against none.
  const used = async () => (await db.collection(`teams/${teamId}/requestQuotas`).get()).docs.map(doc => doc.data().used).sort();
  assert.deepEqual(await used(), [1, 1, 1]);
  await send([(await upload(picture)).uploadId]);
  assert.deepEqual(await used(), [1, 1, 1]);
});

test('attachments: a finished task records the files its model was given, as far as the service stands behind them, and the reply shows them (L2)', options, async () => {
  await execute('publishSharedBot', publish(), owner); await execute('heartbeatDeployment', { ...host, ready: true }, owner);
  await execute('createConversation', { conversationId: botChat, kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member);
  const picture = png(4096), paper = pdf(2048);
  const first = await upload(picture, { conversationId: botChat, name: 'ticket.png' }), second = await upload(paper, { conversationId: botChat, name: 'itinerary.pdf', mime: 'application/pdf' });
  const elsewhere = await upload(picture, { name: 'elsewhere.png' }); await send([elsewhere.uploadId]);
  const loose = await upload(picture, { conversationId: botChat, name: 'never-sent.png' });
  const asked = await send([first.uploadId, second.uploadId], { conversationId: botChat, text: 'When do we leave?', targets: [botId] }), taskId = asked.tasks[0].taskId;
  const lastChangeSeq = async () => (await chatData(botChat)).lastChangeSeq;
  const started = await execute('startTask', { ...host, taskId, expectedLastChangeSeq: await lastChangeSeq() }, owner);
  assert.equal(started.execute, true);
  const result = more => ({ ...host, taskId, permitId: started.permitId, outcome: 'completed', text: 'At 18:30.', finishedAt: now,
    inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
  const claimed = [{ attachmentId: first.uploadId, sha256: sha(picture) }, { attachmentId: second.uploadId, sha256: sha(paper) },
    { attachmentId: first.uploadId, sha256: sha(picture) },                 // named twice
    { attachmentId: elsewhere.uploadId, sha256: sha(picture) },             // a file of another conversation
    { attachmentId: loose.uploadId, sha256: sha(picture) }];                // checked, but no message carries it
  await assert.rejects(execute('completeTask', result({ readFiles: [...claimed, { attachmentId: 'unknown_file_01', sha256: sha('x') }] }), owner), { name: 'ZodError' }, 'more than five');
  await assert.rejects(execute('completeTask', result({ readFiles: [] }), owner), { name: 'ZodError' });
  const done = await execute('completeTask', result({ readFiles: claimed }), owner, 'command_complete_files');
  assert.equal(done.state, 'completed');
  const reply = await messageData(done.replyId, botChat), task = (await db.doc(`teams/${teamId}/tasks/${taskId}`).get()).data();
  assert.deepEqual(reply.readFiles, [{ attachmentId: first.uploadId, name: 'ticket.png' }, { attachmentId: second.uploadId, name: 'itinerary.pdf' }]);
  assert.deepEqual(task.readFiles, [{ attachmentId: first.uploadId, sha256: sha(picture) }, { attachmentId: second.uploadId, sha256: sha(paper) }]);
  assert.equal('attachments' in reply, false, 'the reply shows its sources; it carries no file of its own');
  // The same delivery again is the same result. A file taken back afterwards stays in the record: the Bot did read it.
  assert.deepEqual(await execute('completeTask', result({ readFiles: claimed }), owner, 'command_complete_files'), done);
  await execute('revokeAttachment', { attachmentId: second.uploadId, changeId: `change_${++serial}` });
  assert.equal((await messageData(done.replyId, botChat)).readFiles.length, 2);
  // A file with another digest is not one the service checked.
  const next = await send([(await upload(picture, { conversationId: botChat, name: 'again.png' })).uploadId], { conversationId: botChat, text: 'And this?', targets: [botId] });
  const again = await execute('startTask', { ...host, taskId: next.tasks[0].taskId, expectedLastChangeSeq: await lastChangeSeq() }, owner);
  const plain = await execute('completeTask', { ...result({ readFiles: [{ attachmentId: first.uploadId, sha256: sha('other bytes') }] }), taskId: next.tasks[0].taskId, permitId: again.permitId }, owner);
  const second_reply = await messageData(plain.replyId, botChat);
  assert.equal('readFiles' in second_reply, false);
  // A failed run claims nothing, whatever the host sends.
  const third = await send([(await upload(picture, { conversationId: botChat, name: 'third.png' })).uploadId], { conversationId: botChat, text: 'Once more', targets: [botId] });
  const failing = await execute('startTask', { ...host, taskId: third.tasks[0].taskId, expectedLastChangeSeq: await lastChangeSeq() }, owner);
  const failed = await execute('completeTask', { ...result({ outcome: 'failed', readFiles: [{ attachmentId: first.uploadId, sha256: sha(picture) }] }), taskId: third.tasks[0].taskId, permitId: failing.permitId }, owner);
  assert.equal('readFiles' in await messageData(failed.replyId, botChat), false);
  // A file taken back while the Bot works changes the request under it: that run publishes nothing, as after any change of its request.
  const changing = await send([(await upload(picture, { conversationId: botChat, name: 'changing.png' })).uploadId], { conversationId: botChat, text: 'And this one?', targets: [botId] });
  const begun = await execute('startTask', { ...host, taskId: changing.tasks[0].taskId, expectedLastChangeSeq: await lastChangeSeq() }, owner);
  await execute('revokeAttachment', { attachmentId: (await messageData(changing.messageId, botChat)).attachments[0].attachmentId, changeId: `change_${++serial}` });
  await assert.rejects(execute('completeTask', { ...result(), taskId: changing.tasks[0].taskId, permitId: begun.permitId }, owner), { code: 'TASK_REVOKED' });
});
