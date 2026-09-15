import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectedGitScope } from '../src/renderer/bridge/gitScope';
test('selected Git project overrides the still-open conversation project', () => {
 assert.deepEqual(selectedGitScope('/LingXi', {projectPath:'/AGAI',sessionId:'old'},'/AGAI'), {projectPath:'/LingXi',sessionId:'__draft__'});
 assert.deepEqual(selectedGitScope('/LingXi', {projectPath:'/LingXi',sessionId:'current'}), {projectPath:'/LingXi',sessionId:'current'});
 assert.equal(selectedGitScope(undefined,undefined),null);
});
