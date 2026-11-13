// M3-4: files in a Team message on Web and Desktop — choosing, uploading, sending, showing and taking back (L2–L4, client side).
import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { createElement } from 'react';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { MessageCommand, TeamMessage } from '../../../packages/teams/messages';
import { ATTACHMENT_FILE_BYTES, ATTACHMENT_PART_BYTES, admitFiles, attachmentName, attachmentType, downloadAttachment, fileSize, uploadAttachment, type MessageAttachment } from '../../../packages/teams/attachments';
import { isMemberExecutionCommand } from '../../../packages/teams/execution-validation';
import { firestoreMessage } from '../../../packages/teams/firestore';
import { FakeTeams, conversation, memoryStorage, message, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { MessageFiles } = loadTeamsModule<typeof import('../../../packages/teams/MessageFiles')>('MessageFiles');

const scope = { teamId: 'team-test', generation: 1 };
const sha = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
const bytesOf = (size: number, seed = 1) => Uint8Array.from({ length: size }, (_, index) => (index * 31 + seed) % 251);
const picked = (name: string, type: string, bytes: Uint8Array) => ({ name, type, size: bytes.length, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer });
const attachment = (name: string, mime: MessageAttachment['mime'], bytes: Uint8Array, more: Partial<MessageAttachment> = {}): MessageAttachment =>
  ({ attachmentId: `file-${name}`.replace(/[^A-Za-z0-9_-]/gu, '-').padEnd(8, '0'), name, mime, byteLength: bytes.length, sha256: sha(bytes), ...more });
/** The upload half of the service, kept in memory: what was created, which parts arrived, and every command it saw. */
function uploads(more: { before?(command: Extract<MessageCommand, { schemaVersion: 1 }>): void | Promise<void> } = {}) {
  const files = new Map<string, { facts: Record<string, unknown>; parts: Map<number, Uint8Array>; state: string }>(), seen: Extract<MessageCommand, { schemaVersion: 1 }>[] = [];
  const handle = async (command: MessageCommand): Promise<unknown> => {
    if (!('schemaVersion' in command)) return {};
    assert.ok(isMemberExecutionCommand(command), `the client validates ${command.action}`);
    seen.push(structuredClone(command)); await more.before?.(command);
    const p = command.payload as Record<string, never>;
    if (command.action === 'createUpload') {
      if (!files.has(p.uploadId)) files.set(p.uploadId, { facts: p, parts: new Map(), state: 'uploading' });
      return { uploadId: p.uploadId, state: files.get(p.uploadId)!.state, parts: Math.ceil(p.byteLength / ATTACHMENT_PART_BYTES), partBytes: ATTACHMENT_PART_BYTES };
    }
    if (command.action === 'putUploadPart') { files.get(p.uploadId)!.parts.set(p.index, Uint8Array.from(Buffer.from(p.data, 'base64'))); return { index: p.index }; }
    if (command.action === 'finalizeUpload') { files.get(p.uploadId)!.state = 'verified'; return { uploadId: p.uploadId, state: 'verified', parts: files.get(p.uploadId)!.parts.size, partBytes: ATTACHMENT_PART_BYTES }; }
    if (command.action === 'cancelUpload') { files.delete(p.uploadId); return { uploadId: p.uploadId, state: 'cancelled' }; }
    return {};
  };
  const whole = (uploadId: string) => Buffer.concat([...files.get(uploadId)!.parts.entries()].sort((a, b) => a[0] - b[0]).map(([, part]) => part));
  return { files, seen, handle, whole };
}
/** The read half: files by id, cut into parts as the service cuts them. */
const reader = (stored: Map<string, Uint8Array>, calls: string[] = []) => async (command: MessageCommand): Promise<unknown> => {
  if (!('schemaVersion' in command) || command.action !== 'readAttachmentPart') return undefined;
  const bytes = stored.get(command.payload.attachmentId); calls.push(`${command.payload.attachmentId}:${command.payload.index}`);
  if (!bytes) throw new Error('ATTACHMENT_REVOKED');
  return { attachmentId: command.payload.attachmentId, index: command.payload.index, parts: Math.max(1, Math.ceil(bytes.length / ATTACHMENT_PART_BYTES)),
    data: Buffer.from(bytes.subarray(command.payload.index * ATTACHMENT_PART_BYTES, (command.payload.index + 1) * ATTACHMENT_PART_BYTES)).toString('base64') };
};

test('attachments: what kind a file is sent as comes from its type, else its name; other kinds are not taken', () => {
  assert.deepEqual([attachmentType('a.png', 'image/png'), attachmentType('a', 'image/jpeg'), attachmentType('a.JPG', ''), attachmentType('a', 'image/jpg'), attachmentType('doc.pdf', 'application/pdf'),
    attachmentType('notes.md', 'text/markdown'), attachmentType('notes.md', ''), attachmentType('data.csv', 'text/csv'), attachmentType('a.txt', 'text/plain; charset=utf-8')],
  ['image/png', 'image/jpeg', 'image/jpeg', 'image/jpeg', 'application/pdf', 'text/plain', 'text/plain', 'text/plain', 'text/plain']);
  for (const [name, type] of [['a.gif', 'image/gif'], ['a.zip', 'application/zip'], ['a.png', 'image/webp'], ['a.docx', ''], ['a', ''], ['a.html', 'text/html'], ['a.heic', 'image/heic']]) assert.equal(attachmentType(name!, type!), undefined, name);
  assert.deepEqual([attachmentName('/Users/a/Desktop/photo.png'), attachmentName('C:\\tmp\\a b.txt'), attachmentName('  行程\u0000.pdf '), attachmentName('..'), attachmentName('')], ['photo.png', 'a b.txt', '行程.pdf', 'file', 'file']);
  const long = attachmentName(`${'长'.repeat(200)}.pdf`), astral = attachmentName(`${'😀'.repeat(100)}.png`);
  assert.deepEqual([long.length, long.endsWith('.pdf'), astral.length <= 120, astral.endsWith('.png'), /[\ud800-\udbff]$/u.test(astral.slice(0, -4))], [120, true, true, true, false]);
  assert.deepEqual([fileSize(12), fileSize(320 * 1024), fileSize(1.44 * 1024 * 1024)], ['12 B', '320 KB', '1.4 MB']);
});

test('attachments: a draft takes five files of up to 10 MiB and 20 MiB together, and says why each of the others stays out (L3)', () => {
  const file = (name: string, size: number, type = 'image/png') => ({ name, type, size });
  const first = admitFiles([], [file('a.png', 100), file('b.gif', 100, 'image/gif'), file('empty.txt', 0, 'text/plain'), file('huge.pdf', ATTACHMENT_FILE_BYTES + 1, 'application/pdf'), file('max.pdf', ATTACHMENT_FILE_BYTES, 'application/pdf')]);
  assert.deepEqual(first.accepted.map(item => [item.name, item.mime]), [['a.png', 'image/png'], ['max.pdf', 'application/pdf']]);
  assert.deepEqual(first.refused, [{ name: 'b.gif', code: 'ATTACHMENT_TYPE' }, { name: 'empty.txt', code: 'ATTACHMENT_EMPTY' }, { name: 'huge.pdf', code: 'ATTACHMENT_TOO_LARGE' }]);
  // Counted with what the draft already holds, in the order chosen.
  const six = admitFiles([{ byteLength: 1 }, { byteLength: 1 }, { byteLength: 1 }], [file('4.png', 1), file('5.png', 1), file('6.png', 1)]);
  assert.deepEqual([six.accepted.map(item => item.name), six.refused], [['4.png', '5.png'], [{ name: '6.png', code: 'ATTACHMENT_COUNT' }]]);
  const heavy = admitFiles([{ byteLength: ATTACHMENT_FILE_BYTES }], [file('second.pdf', ATTACHMENT_FILE_BYTES, 'application/pdf'), file('third.png', 1), file('fourth.txt', 5, 'text/plain')]);
  assert.deepEqual([heavy.accepted.map(item => item.name), heavy.refused], [['second.pdf'], [{ name: 'third.png', code: 'ATTACHMENT_TOTAL' }, { name: 'fourth.txt', code: 'ATTACHMENT_TOTAL' }]]);
});

test('attachments: an upload goes in parts under stable command ids, so a step repeated after a lost answer is the same step (L3)', async () => {
  const bytes = bytesOf(ATTACHMENT_PART_BYTES * 2 + 77), service = uploads(), progress: number[] = [];
  let lose = 2;
  const api = { messages: async <T>(command: MessageCommand) => { const answer = await service.handle(command);
    if ('schemaVersion' in command && command.action === 'putUploadPart' && command.payload.index === 1 && lose-- > 0) throw new Error('SERVICE_UNAVAILABLE');
    return answer as T; } };
  await uploadAttachment(api, scope, 'test-chat', 'upload-0001', { name: 'photo.png', mime: 'image/png', bytes }, { onProgress: sent => progress.push(sent), wait: async () => {} });
  assert.deepEqual(service.seen.map(command => command.commandId), ['upload-0001_c', 'upload-0001_p0', 'upload-0001_p1', 'upload-0001_p1', 'upload-0001_p1', 'upload-0001_p2', 'upload-0001_f']);
  assert.deepEqual(service.seen[0]!.payload, { conversationId: 'test-chat', uploadId: 'upload-0001', name: 'photo.png', mime: 'image/png', byteLength: bytes.length, sha256: sha(bytes) });
  assert.equal(sha(service.whole('upload-0001')), sha(bytes));
  assert.deepEqual(progress, [ATTACHMENT_PART_BYTES, ATTACHMENT_PART_BYTES * 2, bytes.length]);
  // Started again with the same id after it was checked: nothing is sent twice.
  const before = service.seen.length;
  await uploadAttachment(api, scope, 'test-chat', 'upload-0001', { name: 'photo.png', mime: 'image/png', bytes }, { wait: async () => {} });
  assert.deepEqual(service.seen.slice(before).map(command => command.action), ['createUpload']);
  // A refusal is final and is not asked again; a stopped upload sends nothing more.
  let asked = 0;
  await assert.rejects(uploadAttachment({ messages: async () => { asked += 1; throw new Error('UPLOAD_QUOTA_EXCEEDED'); } }, scope, 'test-chat', 'upload-0002', { name: 'a.png', mime: 'image/png', bytes }, { wait: async () => {} }), /UPLOAD_QUOTA_EXCEEDED/);
  assert.equal(asked, 1);
  const refused = { messages: async <T>(command: MessageCommand) => ('schemaVersion' in command && command.action === 'finalizeUpload' ? { state: 'failed', code: 'UPLOAD_TYPE_MISMATCH' } : await service.handle(command)) as T };
  await assert.rejects(uploadAttachment(refused, scope, 'test-chat', 'upload-0003', { name: 'a.png', mime: 'image/png', bytes: bytesOf(10) }), /UPLOAD_TYPE_MISMATCH/);
  const stop = new AbortController(), stopped = uploads({ before: command => { if (command.action === 'putUploadPart') stop.abort(); } });
  await assert.rejects(uploadAttachment({ messages: async <T>(command: MessageCommand) => await stopped.handle(command) as T }, scope, 'test-chat', 'upload-0004', { name: 'a.png', mime: 'image/png', bytes }, { signal: stop.signal }));
  assert.deepEqual(stopped.seen.map(command => command.action), ['createUpload', 'putUploadPart']);
});

test('attachments: a file is read back whole and must be the bytes its message names (L2)', async () => {
  const bytes = bytesOf(ATTACHMENT_PART_BYTES + 5), file = attachment('photo.png', 'image/png', bytes), calls: string[] = [];
  const read = reader(new Map([[file.attachmentId, bytes]]), calls), api = { messages: async <T>(command: MessageCommand) => await read(command) as T };
  assert.equal(sha(await downloadAttachment(api, scope, file)), sha(bytes));
  assert.deepEqual(calls, [`${file.attachmentId}:0`, `${file.attachmentId}:1`]);
  await assert.rejects(downloadAttachment(api, scope, { ...file, sha256: sha(bytesOf(3)) }), /ATTACHMENT_DAMAGED/);
  await assert.rejects(downloadAttachment(api, scope, { ...file, attachmentId: 'file-gone-0' }), /ATTACHMENT_REVOKED/);
});

test('attachments: a message document gives its files; an entry that is not a file is left out', () => {
  const value = { seq: 1, author: { kind: 'user', uid: 'bob' }, kind: 'text', text: '', changeSeq: 1, changeId: 'change-1', createdAt: null,
    attachments: [{ attachmentId: 'file-00001', name: 'a.png', mime: 'image/png', byteLength: 3, sha256: 'a'.repeat(64) }, { attachmentId: 'file-00002', name: 'b.exe', mime: 'application/x-msdownload', byteLength: 3, sha256: 'b'.repeat(64) },
      { attachmentId: 'file-00003', name: 'c.txt', mime: 'text/plain', byteLength: 9, sha256: 'c'.repeat(64), revoked: true }, 'nonsense'] };
  assert.deepEqual(firestoreMessage('message-1', value).attachments, [{ attachmentId: 'file-00001', name: 'a.png', mime: 'image/png', byteLength: 3, sha256: 'a'.repeat(64) },
    { attachmentId: 'file-00003', name: 'c.txt', mime: 'text/plain', byteLength: 9, sha256: 'c'.repeat(64), revoked: true }]);
  assert.equal('attachments' in firestoreMessage('message-1', { ...value, attachments: undefined }), false);
});

async function openGroup(context: Parameters<Parameters<typeof scenario>[1]>[0], thread: TeamMessage[] = [], handle: (command: MessageCommand) => unknown | Promise<unknown> = () => undefined) {
  withDom(context);
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; const sent: MessageCommand[] = [], service = uploads();
  fake.messages = async <T>(command: MessageCommand) => {
    sent.push(structuredClone(command));
    if (command.action === 'listTasks') return { tasks: [] } as T;
    if (command.action === 'listSharedBots') return { bots: [] } as T;
    return (await handle(command) ?? await service.handle(command)) as T;
  };
  fake.conversations.set('team-test', [conversation('test-chat', { kind: 'group', lastSeq: thread.length }), conversation('other-chat', { kind: 'group' })]);
  fake.threads.set('test-chat', thread);
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await workspace.refresh(); workspace.selectConversation('test-chat');
  await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat' && workspace.getSnapshot().messages.length === thread.length && workspace.getSnapshot().botsLoaded, 'the chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, zh: false })); context.after(() => page.unmount());
  const choose = (...files: ReturnType<typeof picked>[]) => page.during(() => page.find(n => n.type === 'input' && n.props.type === 'file')[0].props.onChange({ target: { files, value: 'x' } }));
  const chips = () => page.find(n => n.type === 'li' && classes(n).includes('tw-file')).map(n => textOf(n));
  const sendButton = () => page.oneByLabel('Send');
  const submits = () => sent.filter((command): command is Extract<MessageCommand, { action: 'submit' }> => 'schemaVersion' in command && command.action === 'submit');
  return { fake, workspace, page, sent, service, choose, chips, sendButton, submits, box: () => page.find(n => n.type === 'textarea')[0] };
}

scenario('Files are chosen, upload at once, and go out with the message; what cannot be sent says why (L2, L3)', async context => {
  const { workspace, page, service, choose, chips, sendButton, submits, box } = await openGroup(context);
  assert.equal(page.byLabel('Add files').length, 1); assert.equal(sendButton().props.disabled, true);
  const photo = bytesOf(5000, 3), notes = new TextEncoder().encode('Gate 12, 13:05');
  await choose(picked('photo.png', 'image/png', photo), picked('clip.mov', 'video/quicktime', bytesOf(10)), picked('notes.txt', 'text/plain', notes));
  await until(() => workspace.getSnapshot().draftFiles.every(file => file.state === 'ready') && workspace.getSnapshot().draftFiles.length === 2, 'both uploads');
  await page.during(() => {});
  assert.deepEqual(chips(), ['photo.png5 KB', 'notes.txt14 B']);
  assert.match(textOf(page.oneByClass('tw-file-refusals')), /clip\.mov: Only PNG and JPEG pictures, PDF documents and plain text files can be sent/);
  // Files alone are a message: nothing is typed.
  assert.equal(sendButton().props.disabled, false);
  await page.type(box(), 'Boarding passes'); await page.submit();
  await until(() => submits().length === 1, 'the message');
  const ids = workspace.getSnapshot().draftFiles.map(file => file.uploadId); assert.deepEqual(ids, [], 'the files left the draft');
  const payload = submits()[0]!.payload;
  assert.deepEqual([payload.text, payload.targets, payload.attachments?.length], ['Boarding passes', [], 2]);
  assert.deepEqual(payload.attachments!.map(id => service.files.get(id)!.state), ['verified', 'verified']);
  assert.deepEqual([sha(service.whole(payload.attachments![0]!)), new TextDecoder().decode(service.whole(payload.attachments![1]!))], [sha(photo), 'Gate 12, 13:05']);
  assert.equal(page.byClass('tw-file-refusals').length, 0);
  assert.equal(box().props.value, '');
});

scenario('A message waits for its files, a file can be taken out again, and a failed one must be removed before sending (L3)', async context => {
  let release!: () => void; const held = new Promise<void>(resolve => { release = resolve; });
  let fail = '';
  const { workspace, page, sent, choose, chips, sendButton, submits, box } = await openGroup(context, [], async command => {
    if (!('schemaVersion' in command)) return undefined;
    if (command.action === 'putUploadPart') await held;
    if (command.action === 'finalizeUpload' && fail) return { state: 'failed', code: fail };
    return undefined;
  });
  await choose(picked('a.png', 'image/png', bytesOf(100)), picked('b.png', 'image/png', bytesOf(200)));
  await page.type(box(), 'With two pictures');
  assert.deepEqual(chips(), ['a.pngUploading… 0%', 'b.pngUploading… 0%']);
  assert.equal(sendButton().props.disabled, true, 'not before the files have arrived');
  assert.match(page.text(), /You can send once the files have uploaded/);
  await page.submit(); assert.equal(submits().length, 0, 'Enter does not send it either');
  // One is taken out while it uploads: it is given up at the service too, and never sent.
  const removed = workspace.getSnapshot().draftFiles[0]!.uploadId;
  await page.click(page.oneByLabel('Remove a.png'));
  assert.deepEqual(chips(), ['b.pngUploading… 0%']);
  await until(() => sent.some(command => 'schemaVersion' in command && command.action === 'cancelUpload' && command.payload.uploadId === removed), 'the cancel');
  release();
  await until(() => workspace.getSnapshot().draftFiles[0]?.state === 'ready', 'the other upload'); await page.during(() => {});
  assert.equal(sendButton().props.disabled, false);
  // A file the service refuses stays in the draft with the reason, and holds the message back until it is removed.
  fail = 'UPLOAD_TYPE_MISMATCH';
  await choose(picked('fake.png', 'image/png', bytesOf(50)));
  await until(() => workspace.getSnapshot().draftFiles.some(file => file.state === 'failed'), 'the refusal'); await page.during(() => {});
  assert.match(chips()[1]!, /fake\.pngOnly PNG and JPEG pictures.*a file must be what its name says/);
  assert.equal(sendButton().props.disabled, true); assert.match(page.text(), /Remove the file that could not be uploaded to send/);
  await page.click(page.oneByLabel('Remove fake.png'));
  await page.submit(); await until(() => submits().length === 1, 'the message');
  assert.deepEqual([submits()[0]!.payload.attachments?.length, submits()[0]!.payload.attachments!.includes(removed)], [1, false]);
});

scenario('A file belongs to the conversation it was added in: switching while it uploads sends nothing elsewhere (L3)', async context => {
  let release!: () => void; const held = new Promise<void>(resolve => { release = resolve; });
  const { workspace, page, service, sent, choose, submits, box } = await openGroup(context, [], async command => { if ('schemaVersion' in command && command.action === 'putUploadPart') await held; return undefined; });
  await choose(picked('a.png', 'image/png', bytesOf(100)));
  await page.during(() => workspace.selectConversation('other-chat'));
  await until(() => workspace.getSnapshot().selectedConversationId === 'other-chat', 'the other chat'); await page.during(() => {});
  assert.deepEqual(workspace.getSnapshot().draftFiles, [], 'the other conversation has no file');
  release(); await until(() => [...service.files.values()].every(file => file.state === 'verified'), 'the upload finishing in the background');
  await page.type(box(), 'Hello there'); await page.submit();
  await until(() => sent.some(command => command.action === 'mutate'), 'the plain message');
  const plain = sent.find((command): command is Extract<MessageCommand, { action: 'mutate' }> => command.action === 'mutate')!;
  assert.deepEqual([plain.conversationId, plain.text, 'attachments' in plain], ['other-chat', 'Hello there', false]);
  assert.equal(submits().length, 0, 'a message without a file or a Bot is an ordinary message');
  assert.deepEqual([...service.files.values()].map(file => file.facts.conversationId), ['test-chat']);
  // Back where it was added, the file is still in the draft, uploaded.
  await page.during(() => workspace.selectConversation('test-chat'));
  await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'back'); await page.during(() => {});
  assert.deepEqual(workspace.getSnapshot().draftFiles.map(file => [file.name, file.state]), [['a.png', 'ready']]);
  // Closing the workspace (another account, sign-out) ends every upload and forgets every draft.
  workspace.close();
});

scenario('A message shows its files: a picture in place, a document to open and save, a file that was taken back, and nothing of a recalled message (L2, L4)', async context => {
  const photo = bytesOf(4000, 5), notes = new TextEncoder().encode('Flight CA 981\nGate 12'), stored = new Map<string, Uint8Array>(), calls: string[] = [];
  const picture = attachment('photo.png', 'image/png', photo), text = attachment('notes.txt', 'text/plain', notes), gone = attachment('old.pdf', 'application/pdf', bytesOf(30), { revoked: true });
  const mine = attachment('mine.pdf', 'application/pdf', bytesOf(60, 9));
  stored.set(picture.attachmentId, photo); stored.set(text.attachmentId, notes); stored.set(mine.attachmentId, bytesOf(60, 9));
  const read = reader(stored, calls);
  const { fake, page, sent } = await openGroup(context, [
    message(1, { text: 'Tickets', attachments: [picture, text, gone] }),
    message(2, { text: '', author: { kind: 'user', uid: 'alice' }, attachments: [mine] }),
    message(3, { text: '', recalled: true, attachments: [attachment('secret.png', 'image/png', bytesOf(9))] }),
  ], command => read(command));
  await until(() => calls.includes(`${picture.attachmentId}:0`), 'the picture being read'); await page.during(() => {});
  const rows = page.find(n => n.type === 'li' && classes(n).includes('tw-file'));
  assert.deepEqual(rows.map(n => textOf(n).replace(/\s+/gu, ' ')), ['photo.png4 KB', 'notes.txt21 BOpen', 'old.pdfTaken back by its sender', 'mine.pdf60 B']);
  assert.equal(page.find(n => n.type === 'img' && n.props.alt === 'photo.png' && String(n.props.src).startsWith('blob:')).length, 1, 'the picture itself');
  assert.doesNotMatch(page.text(), /secret\.png/); assert.match(page.text(), /Message recalled/);
  assert.equal(calls.filter(call => call.startsWith(text.attachmentId)).length, 0, 'a document is read when it is opened');
  // Reading a text file here.
  await page.click(page.button('Open')); await until(() => /Flight CA 981/.test(page.text()), 'the text');
  assert.equal(textOf(page.find(n => n.type === 'pre')[0]), 'Flight CA 981\nGate 12');
  // Only the sender is offered to take a file back; it is one change to the message.
  assert.deepEqual(page.find(n => n.type === 'button' && /^Take back /u.test(String(n.props['aria-label'] ?? ''))).map(n => n.props['aria-label']), ['Take back mine.pdf']);
  await page.click(page.oneByLabel('Take back mine.pdf'));
  assert.match(page.text(), /Nobody will be able to open or save it from this conversation any more/);
  assert.equal(sent.some(command => 'schemaVersion' in command && command.action === 'revokeAttachment'), false);
  await page.click(page.button('Take back'));
  const revoke = sent.filter((command): command is Extract<MessageCommand, { action: 'revokeAttachment' }> => 'schemaVersion' in command && command.action === 'revokeAttachment');
  assert.deepEqual([revoke.length, revoke[0]!.payload.attachmentId], [1, mine.attachmentId]);
  // The message as the service then has it.
  await page.during(() => { for (const watch of fake.open('team-test', 'test-chat')) fake.emit(watch, { kind: 'messages', conversationId: 'test-chat', replace: false, messages: [message(2, { text: '', author: { kind: 'user', uid: 'alice' }, changeSeq: 4, attachments: [{ ...mine, revoked: true }] })] }); });
  assert.match(page.text(), /mine\.pdfTaken back by its sender/);
  assert.equal(page.byLabel('Take back mine.pdf').length, 0);
});

scenario('Saving a file reads it through the service each time it is needed, and a refusal is said in words (L4)', async context => {
  withDom(context);
  const bytes = bytesOf(120, 4), file = attachment('tickets.pdf', 'application/pdf', bytes), stored = new Map([[file.attachmentId, bytes]]), calls: string[] = [], saved: [string, string][] = [];
  const read = reader(stored, calls), api = { messages: async <T>(command: MessageCommand) => await read(command) as T };
  const store = { subscribe: () => () => {}, getSnapshot: () => 0 };
  const other = { ...file, attachmentId: 'file-other-1', name: 'other.pdf' };
  const page = await mount(store, () => createElement(MessageFiles, { api, scope: { teamId: 'team-save', generation: 1 }, files: [file, other], zh: false, save: (target, value) => { saved.push([target.name, sha(value)]); } }));
  context.after(() => page.unmount());
  assert.deepEqual(calls, [], 'nothing is read before it is asked for');
  await page.click(page.oneByLabel('Save tickets.pdf')); await until(() => saved.length === 1, 'the save');
  assert.deepEqual(saved, [['tickets.pdf', sha(bytes)]]);
  // The other file was taken back at the service meanwhile.
  await page.click(page.oneByLabel('Save other.pdf')); await until(() => /taken back and can no longer be opened/.test(page.text()), 'the refusal');
  assert.equal(saved.length, 1);
});
