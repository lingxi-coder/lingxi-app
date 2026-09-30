import { test } from 'node:test';
import assert from 'node:assert/strict';

import { classifyDesktopError } from '../src/renderer/bridge/errors';

test('desktop errors distinguish credential, protocol, engine, workspace, and transport recovery', () => {
  assert.deepEqual(
    classifyDesktopError('macOS login keychain is locked or access is denied (deepseek)'),
    {
      title: 'Provider credential unavailable',
      detail: 'Allow LingXi Code in Keychain, or replace the stored API key in Settings, then retry.',
    },
  );
  assert.equal(classifyDesktopError('macOS Data Protection Keychain is unavailable for this app signature').title, 'Secure persistence unavailable');
  assert.equal(classifyDesktopError('Provider credential required; configure a trusted credential source').title, 'Provider credential missing');
  assert.equal(classifyDesktopError('stored provider credential could not be decrypted (deepseek); replace it in Settings').title, 'Provider credential unavailable');
  assert.equal(classifyDesktopError('HTTP 401 unauthorized').title, 'Provider credential rejected');
  assert.equal(classifyDesktopError('incompatible protocol version').title, 'Engine version mismatch');
  assert.equal(classifyDesktopError('packaged bridge-server is missing').title, 'Bundled engine unavailable');
  assert.equal(classifyDesktopError('workspace path is not a directory').title, 'Workspace unavailable');
  assert.equal(classifyDesktopError('socket disconnected').title, 'Engine connection interrupted');
});
