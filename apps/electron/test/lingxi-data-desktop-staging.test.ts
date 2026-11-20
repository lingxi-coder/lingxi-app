import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { AccountService } from '../src/main/account.js';
import { TeamsService } from '../src/main/teams/service.js';
import type { DataRecordResult, DataRef, DataTemplateView, TeamDataScope } from '../../../packages/teams/lingxi-data.js';

const endpoints = process.env.LINGXI_DATA_STAGING_ENDPOINTS;

test('Desktop account and Team services authorize a real staging record and isolate personal data', {
  skip: !endpoints, timeout: 30000,
}, async t => {
  const text = await readFile(endpoints!, 'utf8');
  const line = text.split('\n').find(row => row.startsWith('{"web"'));
  assert.ok(line);
  const fixture = JSON.parse(line) as { api: string; auth: string; project: string; teamId: string; conversationId: string; reference: DataRef };
  const api = new URL(fixture.api);
  assert.equal(api.protocol, 'http:');
  assert.equal(api.hostname, '127.0.0.1');
  assert.match(fixture.auth, /^127\.0\.0\.1:[0-9]+$/u);
  assert.equal(fixture.project, 'demo-lingxi-data');

  async function login(person: string): Promise<{ uid: string; teams: TeamsService }> {
    const config = { projectId: fixture.project, apiKey: 'demo', appId: 'lingxi-data-staging', authDomain: 'localhost' };
    const account = new AccountService(config, {
      persistent: false, read: async () => null, write: async () => {}, remove: async () => {},
    }, async () => { throw new Error('Use only the email fixture'); }, (input, options) => {
      const upstream = new URL(String(input));
      assert.ok(['identitytoolkit.googleapis.com', 'securetoken.googleapis.com'].includes(upstream.hostname));
      return fetch(`http://${fixture.auth}/${upstream.hostname}${upstream.pathname}${upstream.search}`, options);
    });
    const { user } = await account.execute({ type: 'signIn', email: person + '@fixture.invalid', password: 'AccountFixture123' });
    assert.ok(user);
    const teams = new TeamsService(account, config, api.origin);
    t.after(() => teams.close());
    return { uid: user.uid, teams };
  }

  const reader = await login('reader');
  const owner = await login('owner');
  const scope: TeamDataScope = { kind: 'team', teamId: fixture.teamId, conversationId: fixture.conversationId, generation: 1 };
  const record = await reader.teams.data<DataRecordResult>({ scope, command: { action: 'get', reference: fixture.reference } }, reader.uid);
  assert.equal(record.record.value.title, 'Prepare shipment');
  const views = await reader.teams.data<{ views: DataTemplateView[] }>({ scope,
    command: { action: 'templateViews', dataset: fixture.reference.dataset } }, reader.uid);
  assert.deepEqual(views.views.map(view => view.view), ['order-summary']);
  await assert.rejects(reader.teams.data({ scope, command: { action: 'get', reference: fixture.reference } }, owner.uid), /INVALID_SESSION/u);

  const personal = { kind: 'personal' } as const;
  const { dataset } = await owner.teams.data<{ dataset: string }>({ scope: personal, operation: crypto.randomUUID(), command: {
    action: 'create', schema: { type: 'object', additionalProperties: false, properties: { title: { type: 'string' } } },
  } }, owner.uid);
  await assert.rejects(reader.teams.data({ scope: personal, command: { action: 'describe', dataset } }, reader.uid), /DATA_ACCESS_REQUIRED/u);
});
