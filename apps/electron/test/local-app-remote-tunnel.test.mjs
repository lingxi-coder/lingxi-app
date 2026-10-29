import assert from 'node:assert/strict';
import test from 'node:test';
import { renderLocalAppTunnel } from '../scripts/local-app-remote-tunnel.mjs';

const input = { serverAddr: 'relay.example.com', gatewayPort: 17980,
  originTemplate: 'https://{appId}-{sessionId}.desktop-one.apps.example.com',
  tokenFile: '/private/token', trustedCaFile: '/private/relay-ca.pem', certificateFile: '/private/app-cert.pem', privateKeyFile: '/private/app-key.pem' };

test('renders Desktop-terminated TLS with wildcard routes, preserved Host, file token and verified relay', () => {
  const result = renderLocalAppTunnel(input);
  assert.match(result.toml, /customDomains = \["\*\.desktop-one\.apps\.example\.com"\]/);
  assert.match(result.toml, /transport\.tls\.trustedCaFile = "\/private\/relay-ca.pem"/);
  assert.match(result.toml, /auth\.tokenSource\.file\.path = "\/private\/token"/);
  assert.match(result.toml, /localAddr = "127.0.0.1:17980"/);
  assert.match(result.toml, /type = "https2http"/);
  assert.match(result.toml, /^loginFailExit = false$/m);
  assert.doesNotMatch(result.toml, /connectServerLocalIP/);
  assert.doesNotMatch(result.toml, /hostHeaderRewrite|insecureSkipVerify|auth\.token\s*=/);
  assert.deepEqual(result.desktopEnvironment, { LINGXI_LOCAL_APP_REMOTE_ORIGIN: input.originTemplate, LINGXI_LOCAL_APP_GATEWAY_PORT: '17980' });
});

test('different Desktop routes receive stable distinct proxy names on a shared relay', () => {
  const proxyName = config => /^name = "([^"]+)"$/m.exec(renderLocalAppTunnel(config).toml)?.[1];
  const first = proxyName(input);
  const second = proxyName({ ...input, originTemplate: 'https://{appId}-{sessionId}.desktop-two.apps.example.com' });
  assert.match(first, /^local-apps-[a-f0-9]{32}$/);
  assert.equal(first, proxyName(input));
  assert.notEqual(first, second);
});

test('refuses shared origins, unsafe hosts, missing verification and malformed port/path inputs', () => {
  for (const override of [
    { originTemplate: 'https://{appId}.apps.example.com' },
    { originTemplate: 'https://{appId}-{sessionId}.apps.example.com/path' },
    { originTemplate: 'http://{appId}-{sessionId}.apps.example.com' },
    { originTemplate: 'https://user:secret@{appId}-{sessionId}.apps.example.com' },
    { originTemplate: 'https://{appId}-{sessionId}.apps.example.com?x=1' },
    { serverAddr: 'relay.example.com/path' }, { serverAddr: 'relay.example.com\nkey=bad' },
    { trustedCaFile: '' }, { privateKeyFile: 'relative/key' }, { gatewayPort: 0 }, { serverPort: 65536 },
  ]) assert.throws(() => renderLocalAppTunnel({ ...input, ...override }));
});

test('escapes file names as TOML strings without reading credential files', () => {
  const result = renderLocalAppTunnel({ ...input, tokenFile: '/private/a "quoted" file' });
  assert.ok(result.toml.includes('"/private/a \\"quoted\\" file"'));
});
