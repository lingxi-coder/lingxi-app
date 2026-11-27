import { spawn } from 'node:child_process';
import { rename, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const metadataToken = 'http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token';

export function createCoreTokenRefresh(path, request = fetch, now = Date.now) {
  let refreshAt = 0;
  let pending;
  return async function refresh() {
    if (now() < refreshAt) return;
    pending ??= (async () => {
      const response = await request(metadataToken, {
        headers: { 'Metadata-Flavor': 'Google' }, redirect: 'error', signal: AbortSignal.timeout(4000),
      });
      if (!response.ok || response.headers.get('metadata-flavor') !== 'Google') {
        throw new Error('Could not get a Firestore workload token');
      }
      const { access_token: token, token_type: type, expires_in: expires } = await response.json();
      if (typeof token !== 'string' || !token || type?.toLowerCase() !== 'bearer' ||
          !Number.isInteger(expires) || expires < 1 || expires > 86400) {
        throw new Error('Invalid Firestore workload token');
      }
      await writeFile(path + '.next', token, { mode: 0o600 });
      await rename(path + '.next', path);
      refreshAt = now() + Math.max(0, expires - 60) * 1000;
    })().finally(() => { pending = undefined; });
    await pending;
  };
}

export async function startCloudDataCore({ projectId, apiKey, actorFile, origin, cert, key }) {
  if (process.env.FIRESTORE_EMULATOR_HOST || process.env.FIREBASE_AUTH_EMULATOR_HOST) {
    throw new Error('The Cloud Run Data adapter requires a live Firebase project');
  }
  const directory = dirname(actorFile);
  const token = join(directory, 'firestore-token');
  const config = join(directory, 'core.json');
  const refreshToken = createCoreTokenRefresh(token);
  await refreshToken();
  await writeFile(config, JSON.stringify({
    project: projectId, firebase_api_key: apiKey, actor_file: actorFile, firestore_token_file: token,
  }), { mode: 0o600 });

  const core = new URL(origin);
  const child = spawn('/usr/local/bin/lingxi-data', [
    'api', '--config', config, '--listen', core.host, '--cert', cert, '--tls-key', key,
  ], { stdio: 'inherit' });
  let stopped = false;
  child.once('error', () => { stopped = true; });
  child.once('exit', () => { stopped = true; });
  const readiness = new URL('/v1/data', core);
  const deadline = Date.now() + 25_000;
  while (!stopped && Date.now() < deadline) {
    // A request without an ID token only checks the TLS listener and API startup.
    const response = await fetch(readiness, {
      method: 'POST', signal: AbortSignal.timeout(1000), redirect: 'error',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ command: { action: 'describe', dataset: 'readiness' } }),
    }).catch(() => undefined);
    if (response) await response.body?.cancel();
    if (!stopped && response?.status === 401) return { child, refreshToken };
    await delay(150);
  }
  child.kill();
  throw new Error('The LingXi Data Core did not start');
}
