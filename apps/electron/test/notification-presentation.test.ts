import assert from 'node:assert/strict';
import { test } from 'node:test';
import { notificationPresentation } from '../src/main/notificationPresentation';

test('native macOS notifications show a project subtitle without leaking its full path', () => {
  const result = notificationPresentation('Task finished', 'Review is ready', { projectPath: '/Users/private/Projects/LingXi', sessionId: 'session' }, 'darwin');
  assert.deepEqual(result, { title: 'Task finished', subtitle: 'LingXi', body: 'Review is ready' });
  assert.equal('subtitle' in notificationPresentation('Task', 'Ready', undefined, 'linux'), false);
});

test('native notification copy collapses whitespace and bounds long Unicode content', () => {
  const result = notificationPresentation('  Task\n finished ', '🙂'.repeat(300), undefined, 'darwin');
  assert.equal(result.title, 'Task finished');
  assert.equal(Array.from(result.body).length, 240);
  assert.ok(result.body.endsWith('🙂…'));
  assert.equal(notificationPresentation(' ', '\n ').title, 'LingXi Code');
});

test('a notification names a Bot by its name, because the folder of its chats says nothing', () => {
  const ref = { projectPath: '/Users/private/app-data/bots/Bot0000000000000000001/workspace', sessionId: 'session' };
  assert.equal(notificationPresentation('Done', 'Ready', ref, 'darwin').subtitle, 'workspace');
  assert.deepEqual(notificationPresentation('Done', 'Ready', ref, 'darwin', 'Research'), { title: 'Done', subtitle: 'Research', body: 'Ready' });
  assert.equal(notificationPresentation('Done', 'Ready', ref, 'darwin', 'R'.repeat(100)).subtitle?.length, 60, 'and a long name is bounded like a folder name');
  assert.equal('subtitle' in notificationPresentation('Done', 'Ready', undefined, 'darwin', 'Research'), false, 'a notification of no session has no place to name');
  assert.equal('subtitle' in notificationPresentation('Done', 'Ready', ref, 'linux', 'Research'), false, 'and only macOS shows a subtitle');
});
