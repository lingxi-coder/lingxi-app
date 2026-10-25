import assert from 'node:assert/strict';
import { test } from 'node:test';
import { canEnumerateRtcInterfaces } from '../src/main/remoteControl/rtcPermissionCheck.js';

const trusted = {
  permission: 'media', sameContents: true, isMainFrame: true,
  pageUrl: 'file:///app/remote-rtc.html', requestingUrl: 'file:///app/remote-rtc.html',
};

test('only the loaded RTC main page may pass the interface-enumeration status check', () => {
  assert.equal(canEnumerateRtcInterfaces(trusted), true);
  for (const change of [
    { sameContents: false }, { isMainFrame: false }, { isMainFrame: undefined },
    { pageUrl: undefined }, { pageUrl: '' }, { requestingUrl: '' },
    { requestingUrl: undefined }, { requestingUrl: 'file:///app/other.html' },
    { requestingUrl: 'https://example.com/remote-rtc.html' },
  ]) assert.equal(canEnumerateRtcInterfaces({ ...trusted, ...change }), false, JSON.stringify(change));
});

test('interface enumeration does not grant other permission status checks', () => {
  for (const permission of ['geolocation', 'notifications', 'display-capture', 'clipboard-read', 'local-network', 'unknown']) {
    assert.equal(canEnumerateRtcInterfaces({ ...trusted, permission }), false, permission);
  }
});
