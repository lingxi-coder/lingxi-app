import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const staging = 'https://dataset-product-tx4epgql6a-uc.a.run.app';
const replicas = ['https://dataset-api-a-tx4epgql6a-uc.a.run.app', 'https://dataset-api-b-tx4epgql6a-uc.a.run.app'];

interface FixtureTransport {
  fetch: typeof fetch;
  replayedWrites(): number;
}

/** The product gateway receives the normal write first. Submit the same run request to each private
 * replica and compare its persisted receipt, without giving the host a second human credential. */
export async function datasetFixtureTransport(origin: string): Promise<FixtureTransport> {
  const path = process.env.DATASET_V2_TEST_IAM_FILE;
  if (!path) {
    assert.match(origin, /^https:\/\/127\.0\.0\.1:\d+$/u);
    return { fetch, replayedWrites: () => 0 };
  }
  assert.equal(origin, staging);
  const tokens: Record<string, string> = JSON.parse(await readFile(path, 'utf8'));
  for (const url of [staging, ...replicas]) {
    const token = tokens[url];
    assert.equal(typeof token, 'string');
    const payload = JSON.parse(Buffer.from(token!.split('.')[1]!, 'base64url').toString('utf8'));
    assert.equal(payload.aud, url);
    assert.ok(payload.exp > Date.now() / 1000);
  }
  let writes = 0;
  function send(host: string, path: string, init: RequestInit): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set('X-Serverless-Authorization', 'Bearer ' + tokens[host]);
    return fetch(host + path, { ...init, headers, redirect: 'error' });
  }
  async function fetchStaging(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const url = new URL(input instanceof Request ? input.url : String(input));
    assert.equal(url.origin, staging);
    assert.ok(url.pathname === '/v1/teams/datasets' || url.pathname === '/v2/dataset-runs');
    assert.equal(url.search, '');
    assert.ok(init?.method === 'POST' && typeof init.body === 'string');
    const result = await send(staging, url.pathname, init);
    if (url.pathname === '/v2/dataset-runs' && result.ok && JSON.parse(init.body).command.action === 'put') {
      const receipt = await result.clone().json();
      for (const replica of replicas) {
        const replay = await send(replica, url.pathname, init);
        assert.equal(replay.status, 200);
        assert.deepEqual(await replay.json(), receipt);
      }
      writes++;
    }
    return result;
  }
  return { fetch: fetchStaging, replayedWrites: () => writes };
}
