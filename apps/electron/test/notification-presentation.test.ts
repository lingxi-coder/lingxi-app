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
