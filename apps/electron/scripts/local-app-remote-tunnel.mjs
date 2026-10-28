import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const quote = value => JSON.stringify(value);
function filePath(value, label) {
  if (typeof value !== 'string' || !isAbsolute(value) || /[\u0000-\u001f]/u.test(value)) throw new Error(`${label} must be an absolute file path`);
  return value;
}
function port(value, label) {
  if (!Number.isInteger(value) || value < 1 || value > 65535) throw new Error(`Invalid ${label}`);
  return value;
}

/** Render configuration only. Never opens ports, installs software, or reads secret file contents. */
export function renderLocalAppTunnel(input) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('Expected a tunnel configuration object');
  const { serverAddr, originTemplate } = input;
  if (typeof serverAddr !== 'string' || !/^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/u.test(serverAddr)) throw new Error('serverAddr must be a DNS hostname or IPv4 address');
  if (typeof originTemplate !== 'string' || (originTemplate.match(/\{appId\}/g) ?? []).length !== 1 || (originTemplate.match(/\{sessionId\}/g) ?? []).length !== 1) throw new Error('Origin template requires one {appId} and one {sessionId}');
  const label = `a${'0'.repeat(32)}-${'0'.repeat(20)}`;
  const origin = new URL(originTemplate.replace('{appId}', `a${'0'.repeat(32)}`).replace('{sessionId}', '0'.repeat(20)));
  const suffix = origin.hostname.slice(label.length + 1);
  if (origin.protocol !== 'https:' || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash ||
      !origin.hostname.startsWith(`${label}.`) || !suffix.includes('.') || suffix.split('.').some(part => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/u.test(part))) {
    throw new Error('Use https://{appId}-{sessionId}.<desktop-domain> as the origin template');
  }
  const serverPort = port(input.serverPort ?? 7000, 'server port');
  const gatewayPort = port(input.gatewayPort, 'gateway port');
  const tokenFile = filePath(input.tokenFile, 'tokenFile');
  const trustedCaFile = filePath(input.trustedCaFile, 'trustedCaFile');
  const certificateFile = filePath(input.certificateFile, 'certificateFile');
  const privateKeyFile = filePath(input.privateKeyFile, 'privateKeyFile');
  const toml = [
    '# Generated Local App tunnel. TLS terminates on this Desktop.',
    `serverAddr = ${quote(serverAddr)}`, `serverPort = ${serverPort}`, 'loginFailExit = false',
    'auth.method = "token"', 'auth.tokenSource.type = "file"', `auth.tokenSource.file.path = ${quote(tokenFile)}`,
    'transport.tls.enable = true', `transport.tls.trustedCaFile = ${quote(trustedCaFile)}`,
    `transport.tls.serverName = ${quote(serverAddr)}`, 'log.level = "warn"', '',
    '[[proxies]]', 'name = "local-apps"', 'type = "https"', `customDomains = [${quote(`*.${suffix}`)}]`, '',
    '[proxies.plugin]', 'type = "https2http"', `localAddr = "127.0.0.1:${gatewayPort}"`,
    `crtPath = ${quote(certificateFile)}`, `keyPath = ${quote(privateKeyFile)}`,
    '# Preserve Host: the gateway authenticates each session origin.', '',
  ].join('\n');
  return { toml, desktopEnvironment: { LINGXI_LOCAL_APP_REMOTE_ORIGIN: originTemplate, LINGXI_LOCAL_APP_GATEWAY_PORT: String(gatewayPort) } };
}

async function main(args) {
  if (args.length === 1 && args[0] === '--help') {
    process.stdout.write('Usage: node scripts/local-app-remote-tunnel.mjs <input.json> <new-frpc.toml>\nWrites private config and <new-frpc.toml>.desktop-env.json; does not start a tunnel.\n');
    return;
  }
  if (args.length !== 2) throw new Error('Expected input JSON and a new output TOML path; use --help');
  const result = renderLocalAppTunnel(JSON.parse(await readFile(resolve(args[0]), 'utf8')));
  const output = resolve(args[1]), environment = `${output}.desktop-env.json`;
  // Do not overwrite deployment configuration or follow an existing output symlink.
  await writeFile(output, result.toml, { flag: 'wx', mode: 0o600 });
  await writeFile(environment, JSON.stringify(result.desktopEnvironment, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  process.stdout.write(`Created ${output}\nCreated ${environment}\n`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch(error => { process.stderr.write(`${error instanceof Error ? error.message : 'Configuration failed'}\n`); process.exitCode = 1; });
}
