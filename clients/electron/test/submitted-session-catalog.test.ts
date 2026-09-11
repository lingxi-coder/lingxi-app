import { test } from 'node:test';
import assert from 'node:assert/strict';
import { submittedSessionCatalogs } from '../src/renderer/bridge/submittedSessionCatalog';

const row = { uuid: 'new', title: 'First message', modified_rfc3339: '2026-09-11T12:00:00Z', message_count: 1, mode: 'code' as const, path: '' };
const submitted = [{ projectPath: '/project', row }];

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
