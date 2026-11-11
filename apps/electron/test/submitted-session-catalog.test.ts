import { test } from 'node:test';
import assert from 'node:assert/strict';
import { firstSubmittedSession, reconcileSubmittedSessions, submittedSessionCatalogs } from '../src/renderer/bridge/submittedSessionCatalog';

const row = { uuid: 'new', title: 'First message', modified_rfc3339: '2026-09-11T12:00:00Z', message_count: 1, mode: 'code' as const, path: '' };
const submitted = [{ projectPath: '/project', row }];

test('first message creates a sidebar row before the transcript catalog is saved', () => {
  const ref = { projectPath: '/project', sessionId: 'new', mode: 'chat' as const };
  const first = firstSubmittedSession(ref, '  /plan   describe the change  ', undefined);
  assert.equal(first?.row.title, '/plan describe the change');
  assert.equal(first?.row.message_count, 1);
  assert.equal(first?.row.mode, 'chat');
  assert.deepEqual(submittedSessionCatalogs({}, first ? [first] : [], [])['/project'].sessions, [first?.row]);
  assert.equal(firstSubmittedSession(ref, 'another message', { ...row, message_count: 2 }), undefined);
});

test('a provisional row is not assigned a guessed mode', () => {
  assert.equal(firstSubmittedSession({ projectPath: '/project', sessionId: 'new' }, 'hello', undefined), undefined);
});

test('first submission is visible before the saved catalog exists and survives stale refreshes', () => {
  assert.deepEqual(submittedSessionCatalogs({}, submitted, [])['/project'].sessions, [row]);
  const stale = { '/project': { sessions: [{ ...row, title: '', message_count: 0 }] } };
  assert.deepEqual(submittedSessionCatalogs(stale, submitted, [])['/project'].sessions, [row]);
  assert.equal(stale['/project'].sessions[0].message_count, 0);
});

test('persisted catalog takes over without duplicate rows or overwriting server titles', () => {
  const saved = { ...row, title: 'Generated title', message_count: 3, path: '/session.jsonl' };
  const catalogs = { '/project': { sessions: [saved] } };
  assert.deepEqual(submittedSessionCatalogs(catalogs, submitted, [])['/project'].sessions, [saved]);
});

test('submitted rows survive project switching but cannot resurrect archived sessions', () => {
  const catalogs = submittedSessionCatalogs({}, [...submitted, { projectPath: '/other', row: { ...row, uuid: 'other' } }], [{ projectPath: '/project', sessionId: 'new' }]);
  assert.equal(catalogs['/project'], undefined);
  assert.equal(catalogs['/other'].sessions[0].uuid, 'other');
});

test('empty, archived and authoritative overlays retain canonical catalog references', () => {
  const catalogs = { '/project': { sessions: [row] }, '/other': { sessions: [] } };
  assert.equal(submittedSessionCatalogs(catalogs, [], []), catalogs);
  assert.equal(submittedSessionCatalogs(catalogs, submitted, []), catalogs);
  assert.equal(submittedSessionCatalogs(catalogs, submitted, [{ projectPath: '/project', sessionId: row.uuid }]), catalogs);
  const changed = submittedSessionCatalogs(catalogs, [{ projectPath: '/missing', row }], []);
  assert.notEqual(changed, catalogs);
  assert.equal(changed['/project'], catalogs['/project']);
  assert.equal(changed['/other'], catalogs['/other']);
});

test('batched overlays preserve sequential prepend order, first positive row and exact source metadata', () => {
  const metadata = { ...row, message_count: 0, path: '/Case/Exact source.jsonl', resume_title: ' Raw title ' };
  const saved = { ...row, uuid: 'saved', title: 'Authoritative' };
  const catalogs = { '/project': { sessions: [metadata, saved] } };
  const result = submittedSessionCatalogs(catalogs, [
    { projectPath: '/project', row: { ...row, path: metadata.path } },
    { projectPath: '/project', row: { ...row, uuid: 'later' } },
    { projectPath: '/project', row: { ...row, title: 'Must not replace first submission' } },
    { projectPath: '/project', row: { ...row, uuid: 'saved', title: 'Must not replace saved title' } },
  ], []);
  assert.deepEqual(result['/project'].sessions.map(session => session.uuid), ['later', 'new', 'saved']);
  assert.equal(result['/project'].sessions[1]?.title, row.title);
  assert.equal(result['/project'].sessions[1]?.path, metadata.path);
  assert.equal(result['/project'].sessions[1]?.resume_title, metadata.resume_title);
  assert.equal(result['/project'].sessions[2], saved);
  assert.equal(catalogs['/project'].sessions[0], metadata);
});

test('many pending overlays scan and copy a project catalog once per merge', () => {
  let identityReads = 0;
  const sessions = Array.from({ length: 2000 }, (_, index) => ({
    ...row, get uuid() { identityReads++; return `saved-${index}`; },
  }));
  const overlays = Array.from({ length: 100 }, (_, index) => ({ projectPath: '/project', row: { ...row, uuid: `pending-${index}` } }));
  const result = submittedSessionCatalogs({ '/project': { sessions } }, overlays, []);
  assert.equal(result['/project'].sessions.length, 2100);
  assert.ok(identityReads <= sessions.length * 3, `${identityReads} identity reads for ${sessions.length} saved rows`);
  assert.equal(result['/project'].sessions[0]?.uuid, 'pending-99');
  assert.equal(result['/project'].sessions[99]?.uuid, 'pending-0');
});

test('runtime-only changes keep the submitted map stable and removals cannot retain stale overlays', () => {
  const empty = new Map();
  assert.equal(reconcileSubmittedSessions(empty, new Map([['runtime', {}]])), empty);
  const first = reconcileSubmittedSessions(empty, new Map([['runtime', { submittedSession: submitted[0] }]]));
  assert.equal(reconcileSubmittedSessions(first, new Map([['runtime', { submittedSession: submitted[0] }], ['other', {}]])), first);
  const removed = reconcileSubmittedSessions(first, new Map([['other', {}]]));
  assert.equal(removed.size, 0);
  assert.equal(first.size, 1);
  const replacement = { projectPath: '/moved', row: { ...row, mode: 'chat' as const } };
  const replaced = reconcileSubmittedSessions(first, new Map([['runtime', { submittedSession: replacement }]]));
  assert.equal(replaced.get('runtime'), replacement);
  assert.equal(first.get('runtime'), submitted[0]);
});

test('overlay order follows runtime insertion rather than first-submission timing', () => {
  const later = { projectPath: '/project', row: { ...row, uuid: 'later' } };
  const states = new Map([['older-runtime', {}], ['newer-runtime', { submittedSession: later }]]);
  const first = reconcileSubmittedSessions(new Map(), states);
  states.set('older-runtime', { submittedSession: submitted[0] });
  const second = reconcileSubmittedSessions(first, states);
  assert.deepEqual([...second.keys()], ['older-runtime', 'newer-runtime']);
  assert.deepEqual(submittedSessionCatalogs({}, second.values(), [])['/project'].sessions.map(session => session.uuid), ['later', 'new']);
});
