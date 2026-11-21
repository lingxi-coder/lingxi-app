import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';

const entry = new URL('../src/index.mjs', import.meta.url);
const project = 'lingxi-dataset-v2-stg-fixture';

test('Dataset staging uses only its project’s identities and browser origins', () => {
  execFileSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import { EventEmitter } from 'node:events';
    const project = process.env.GCLOUD_PROJECT;
    const { teamsApi, llmApi, onDatasetTaskDelivery } = await import(${JSON.stringify(entry.href)});
    assert.equal(teamsApi.__endpoint.serviceAccountEmail, 'push-api@' + project + '.iam.gserviceaccount.com');
    assert.equal(onDatasetTaskDelivery.__endpoint.serviceAccountEmail, teamsApi.__endpoint.serviceAccountEmail);
    assert.equal(llmApi.__endpoint.serviceAccountEmail, 'llm-proxy@' + project + '.iam.gserviceaccount.com');
    assert.deepEqual(teamsApi.__endpoint.secretEnvironmentVariables, [{ key: 'LINGXI_DATASET_RENDER_KEY' }]);
    for (const [route, path] of [[teamsApi, '/v1/teams'], [llmApi, '/v1/llm']]) {
      async function preflight(origin) {
        const res = Object.assign(new EventEmitter(), {
          headers: new Map(),
          set(name, value) { this.headers.set(name, value); return this; },
          status(value) { this.code = value; return this; },
          send() {}, end() {}, write() {},
        });
        await route({ method: 'OPTIONS', url: path, headers: { origin } }, res);
        return res;
      }
      const staging = 'https://' + project + '.web.app';
      const accepted = await preflight(staging);
      assert.equal(accepted.code, 204);
      assert.equal(accepted.headers.get('access-control-allow-origin'), staging);
      const rejected = await preflight('https://lingxios.net');
      assert.equal(rejected.code, 403);
      assert.equal(rejected.headers.has('access-control-allow-origin'), false);
    }
  `], { env: { ...process.env, FIREBASE_CONFIG: JSON.stringify({ projectId: project }), GCLOUD_PROJECT: project } });
});
