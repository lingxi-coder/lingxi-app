import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_RTDB_URL, DEFAULT_STUN_URLS } from '@lingxi/bridge-client/remote';

import { DEFAULT_PUSH_API_URL, hostDisplayName, remoteControlConfig } from '../src/main/remoteControl/config.js';

test('the defaults are the project database, the default STUN servers and no switches', () => {
  const config = remoteControlConfig({}, true);
  assert.deepEqual(config, {
    rtdbUrl: DEFAULT_RTDB_URL,
    stunUrls: [...DEFAULT_STUN_URLS],
    pushApiUrl: DEFAULT_PUSH_API_URL,
    forceTunnel: false,
    forceEnabled: false,
    warnings: [],
  });
});

test('the push API address can be configured with the same distrust as the database address', () => {
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_PUSH_URL: 'https://push.example.com/api/' }, true).pushApiUrl, 'https://push.example.com/api');
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_PUSH_URL: 'http://127.0.0.1:5001/x/us-central1/pushApi' }, true).pushApiUrl, 'http://127.0.0.1:5001/x/us-central1/pushApi');
  for (const bad of ['http://example.com', 'ftp://example.com', 'https://user:pass@example.com', 'not a url']) {
    const config = remoteControlConfig({ LINGXI_REMOTE_PUSH_URL: bad }, true);
    assert.equal(config.pushApiUrl, DEFAULT_PUSH_API_URL, bad);
    assert.equal(config.warnings.length, 1, bad);
  }
});

test('a database URL is reduced to its origin, and only https (or http on loopback) is believed', () => {
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_RTDB_URL: 'https://example-default-rtdb.firebaseio.com/' }, true).rtdbUrl, 'https://example-default-rtdb.firebaseio.com');
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_RTDB_URL: 'http://127.0.0.1:9000' }, true).rtdbUrl, 'http://127.0.0.1:9000');
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_RTDB_URL: 'http://localhost:9000/' }, true).rtdbUrl, 'http://localhost:9000');

  // The ID token goes to this URL: anything doubtful is ignored and reported, never trusted.
  for (const bad of [
    'http://example.com',
    'ftp://example.com',
    'https://user:pass@example.com',
    'https://example.com/path',
    'https://example.com/?x=1',
    'https://example.com/#frag',
    'not a url',
  ]) {
    const config = remoteControlConfig({ LINGXI_REMOTE_RTDB_URL: bad }, true);
    assert.equal(config.rtdbUrl, DEFAULT_RTDB_URL, bad);
    assert.equal(config.warnings.length, 1, bad);
    assert.match(config.warnings[0]!, /LINGXI_REMOTE_RTDB_URL/);
  }
});

test('the emulator namespace is kept only when it is a plain name', () => {
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_RTDB_NAMESPACE: 'demo-project' }, true).rtdbNamespace, 'demo-project');
  const bad = remoteControlConfig({ LINGXI_REMOTE_RTDB_NAMESPACE: 'a b&c=d' }, true);
  assert.equal(bad.rtdbNamespace, undefined);
  assert.equal(bad.warnings.length, 1);
});

test('STUN servers: a list of stun: and stuns: URLs, others dropped, at most eight', () => {
  const config = remoteControlConfig({ LINGXI_REMOTE_STUN: ' stun:a.example:3478 , stuns:b.example , turn:c.example , https://d.example ' }, true);
  assert.deepEqual(config.stunUrls, ['stun:a.example:3478', 'stuns:b.example']);
  assert.equal(config.warnings.length, 1);

  const none = remoteControlConfig({ LINGXI_REMOTE_STUN: 'turn:c.example' }, true);
  assert.deepEqual(none.stunUrls, [...DEFAULT_STUN_URLS]);

  const many = remoteControlConfig({ LINGXI_REMOTE_STUN: Array.from({ length: 12 }, (_, i) => `stun:s${i}.example`).join(',') }, true);
  assert.equal(many.stunUrls.length, 8);
});

test('forcing the tunnel works everywhere, forcing the feature on only in an unpackaged build', () => {
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_FORCE_TUNNEL: '1' }, true).forceTunnel, true);
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_FORCE_TUNNEL: 'yes' }, true).forceTunnel, false);
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_CONTROL: '1' }, false).forceEnabled, true);
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_CONTROL: '1' }, true).forceEnabled, false);
  assert.equal(remoteControlConfig({ LINGXI_REMOTE_CONTROL: '0' }, false).forceEnabled, false);
});

test('the host name shown to viewers is clean and fits a host record', () => {
  assert.equal(hostDisplayName('Lingfeng-MacBook-Pro.local'), 'Lingfeng-MacBook-Pro');
  assert.equal(hostDisplayName('  desk\u0000top\n'), 'desk top');
  assert.equal(hostDisplayName(''), 'LingXi Desktop');
  assert.equal(hostDisplayName('.local'), 'LingXi Desktop');
  assert.equal(hostDisplayName('a'.repeat(300)).length, 128);
  // A cap in the middle of a surrogate pair drops the half pair.
  const capped = hostDisplayName(`${'a'.repeat(127)}😀`);
  assert.equal(capped, 'a'.repeat(127));
  assert.equal(hostDisplayName('机房 Mac'), '机房 Mac');
});
