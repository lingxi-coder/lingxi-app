import assert from 'node:assert/strict';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { HostedServicesApi, PublishedService, ServiceCommand, ServiceMessage, ServiceView } from '../../../packages/teams/hosted-service';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom } from './support/workspacePage';

// Bot services (Bots offered outside Teams) as a person meets them: looking one up, agreeing, asking, reporting, leaving;
// and, where Bots are hosted, offering one. The API is a fake that keeps what a real service keeps.
const { HostedServices } = loadTeamsModule<typeof import('../../../packages/teams/HostedServices')>('HostedServices');
const { serviceIdFrom, serviceLink, isServiceMessageCommand } = loadTeamsModule<typeof import('../../../packages/teams/hosted-service')>('hosted-service');

const serviceId = 's_travel_0001', sharedBotId = 'sharedbot001', uid = 'caller';
class ApiError extends Error {}
function fake(host = false) {
  const view: ServiceView = { serviceId, name: 'Travel helper', description: 'Answers travel questions', publisherName: 'Pat', sharedBotId, availability: 'available', entered: false,
    cost: 'publisher', visibility: 'requester_and_publisher', retention: 'Kept for 30 days.', limits: { userConcurrent: 1, userDaily: 20, userDailyLeft: 20 } };
  const state = { view, published: [] as PublishedService[], sessions: [] as Array<{ id: string; title: string }>, messages: new Map<string, ServiceMessage[]>(), changes: new Map<string, string[]>(),
    tasks: [] as any[], sent: [] as any[], commands: [] as ServiceCommand[], reports: [] as any[], refuse: '' as string, publishFails: '' as string, published_inputs: [] as any[] };
  const say = (conversationId: string, text: string, author: ServiceMessage['author'], more: Partial<ServiceMessage> = {}) => {
    const list = state.messages.get(conversationId)!, changes = state.changes.get(conversationId)!, id = `message-${conversationId}-${list.length + 1}`;
    list.push({ id, seq: list.length + 1, changeSeq: changes.length + 1, text, createdAt: new Date(2026, 10, 13, 9, list.length).toISOString(), author, ...more }); changes.push(id); return id;
  };
  const api: HostedServicesApi = {
    async services<T>(command: ServiceCommand): Promise<T> {
      state.commands.push(command);
      if (command.action === 'listServices') return { published: state.published, entered: state.view.entered ? [state.view] : [] } as T;
      if (command.action === 'createService') { const made = { ...command, action: undefined, state: 'active', online: false, publisher: uid, publisherName: 'Me', cost: 'publisher', visibility: 'requester_and_publisher', revision: 1, createdAt: 1, updatedAt: 1 } as unknown as PublishedService; state.published.push(made); return made as T; }
      const mine = state.published.find(service => 'serviceId' in command && service.serviceId === command.serviceId);
      if (command.action === 'setServiceOnline') { mine!.online = command.online; return mine as T; }
      if (command.action === 'withdrawService') { mine!.state = 'withdrawn'; mine!.online = false; return mine as T; }
      // The publisher is shown what they set, as the service does.
      if (command.action === 'getService' && mine) return mine as T;
      if (command.serviceId !== serviceId) throw new ApiError('NOT_FOUND');
      if (command.action === 'getService') return { ...state.view } as T;
      if (command.action === 'enterService') { assert.equal(command.acknowledged, true); state.view = { ...state.view, entered: true, generation: 1 }; return { generation: 1, service: state.view } as T; }
      if (command.action === 'leaveService') { state.view = { ...state.view, entered: false, generation: undefined }; return { left: true } as T; }
      if (command.action === 'reportService') { state.reports.push(command); return { reported: true } as T; }
      throw new ApiError('INVALID_INPUT');
    },
    async serviceMessages<T>(raw: any): Promise<T> {
      assert.equal(isServiceMessageCommand(raw), true, `a caller's command in a service scope: ${raw.action}`);
      const command: any = raw;
      state.sent.push(command); const p = command.payload;
      if (command.action === 'list') return { conversations: state.sessions.map(session => ({ ...session, kind: 'bot_dm', lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0 })) } as T;
      if (command.action === 'sync') { const list = state.messages.get(command.conversationId) ?? [], changes = (state.changes.get(command.conversationId) ?? []).slice(command.after);
        return { conversation: { id: command.conversationId }, changes: changes.map((messageId, index) => ({ seq: command.after + index + 1, messageId })), messages: list.filter(message => changes.includes(message.id)), hasMore: false } as T; }
      if (command.action === 'createConversation') { state.sessions.push({ id: p.conversationId, title: p.title }); state.messages.set(p.conversationId, []); state.changes.set(p.conversationId, []); return { conversationId: p.conversationId } as T; }
      if (command.action === 'listTasks') return { tasks: state.tasks.filter(task => task.conversationId === p.conversationId).map(task => ({ ...task })) } as T;
      if (state.refuse) throw new ApiError(state.refuse);
      if (command.action === 'submit') { say(p.conversationId, p.text, { kind: 'user', uid });
        state.tasks.push({ taskId: `task-0000${state.tasks.length + 1}`, conversationId: p.conversationId, sharedBotId, requester: { uid, generation: 1 }, state: 'queued', createdAt: state.tasks.length + 1, deadline: 9e15, epoch: 1, activeSpentMs: 0 }); return { tasks: [state.tasks.at(-1)] } as T; }
      if (command.action === 'answerWait') { say(p.conversationId, p.text, { kind: 'user', uid }); const task = state.tasks.find(value => value.wait?.waitId === p.waitId); task.state = 'queued'; delete task.wait; return {} as T; }
      if (command.action === 'cancelTask') { state.tasks.find(task => task.taskId === p.taskId).state = 'cancelled'; return {} as T; }
      throw new ApiError('INVALID_INPUT');
    },
    ...(host ? {
      localBots: async () => [{ instanceId: 'instance001', name: 'Trip planner', description: 'Plans trips', model: 'anthropic/synthetic' }, { instanceId: 'instance002', name: 'Needs files', description: '', requiredResources: ['handbook'] }],
      publishLocalBot: async (input: any) => { state.published_inputs.push(input); if (state.publishFails) throw new ApiError(state.publishFails); return { sharedBotId: input.sharedBotId, revision: 1 }; },
      hostStatus: async () => ({ bots: state.published.map(service => ({ teamId: service.serviceId, sharedBotId: service.sharedBotId, state: 'ready' as const, running: 0 })), openAtLogin: { available: false, enabled: false, needsApproval: false }, keepAwake: false, onBattery: false }),
    } : {}),
  };
  /** The publisher's Bot answers the oldest request that is under way. */
  const answer = (text: string, question = false) => { const task = state.tasks.find(value => value.state === 'queued')!;
    const id = say(task.conversationId, text, { kind: 'shared_bot', sharedBotId }, question ? { waitId: 'wait00001' } : {});
    if (question) { task.state = 'waiting'; task.wait = { waitId: 'wait00001', kind: 'human_input', recipient: { uid, generation: 1 }, expiresAt: 9e15, messageId: id }; } else task.state = 'completed'; };
  return { api, state, answer };
}
const store = { subscribe: () => () => {}, getSnapshot: () => 0 }, fast = { answering: 10, idle: 10 };
async function open(context: TestContext, f: ReturnType<typeof fake>, more: Record<string, unknown> = {}) {
  withDom(context);
  const page = await mount(store, () => createElement(HostedServices, { api: f.api, uid, zh: false, renderMarkdown: (text: string) => createElement('article', { 'data-md': true }, text),
    link: (id: string) => serviceLink('https://lingxios.net', id), refreshMs: fast, ...more }));
  context.after(() => page.unmount());
  const field = (label: string) => page.find(node => ['input', 'textarea', 'select'].includes(node.type) && textOf(node.parent) .startsWith(label))[0];
  /** One more reading of the open session, as the timer would do. */
  const tick = async () => { await page.during(async () => { await new Promise(resolve => setTimeout(resolve, 60)); }); };
  return { page, field, tick };
}
const sentActions = (f: ReturnType<typeof fake>) => f.state.sent.filter(c => !['list', 'sync', 'listTasks'].includes(c.action)).map(c => c.action);

scenario('a link names a service; anything else is not looked up', async () => {
  assert.equal(serviceIdFrom(` https://lingxios.net/services/${serviceId} `), serviceId);
  assert.equal(serviceIdFrom(serviceId), serviceId);
  assert.equal(serviceIdFrom(serviceLink('https://lingxios.net', serviceId)), serviceId);
  for (const value of ['', 'team00001', 'https://lingxios.net/teams/join?code=abc', `https://lingxios.net/services/${serviceId}/extra`, 'https://lingxios.net/services/team00001']) assert.equal(serviceIdFrom(value), null, value);
});

scenario('looking a service up shows its terms and enters nothing; agreeing opens it', async context => {
  const f = fake(), { page } = await open(context, f);
  assert.match(page.text(), /You talk to the Bot only/u);
  assert.equal(page.find(node => node.type === 'h3' && textOf(node) === 'Services you offer').length, 0, 'a browser hosts no Bots and offers none');
  await page.type(page.find(node => node.type === 'input')[0], 'not a link'); await page.submit();
  assert.match(page.text(), /That is not a service link/u);
  assert.deepEqual(f.state.commands.map(c => c.action), ['listServices']);
  await page.type(page.find(node => node.type === 'input')[0], `https://lingxios.net/services/${serviceId}`); await page.submit();
  // The terms: who offers it, who reads, who pays, what is kept, how much.
  const terms = page.text();
  for (const line of [/Offered by Pat\. This is not a Team/u, /Who can read your sessions: you and Pat/u, /Who pays: Pat pays for the model/u, /Kept for 30 days\./u, /20 requests a day, 1 at a time/u, /report this service to LingXi/u]) assert.match(terms, line);
  assert.ok(!f.state.commands.some(c => c.action === 'enterService'), 'reading the terms enters nothing');
  assert.equal(f.state.sent.length, 0, 'and nothing is asked of the service scope');
  await page.click(page.button('Agree and open'));
  assert.deepEqual(f.state.commands.filter(c => c.action === 'enterService'), [{ action: 'enterService', serviceId, acknowledged: true }]);
  assert.equal(page.oneByClass('tw-conv-head') && page.find(node => node.type === 'h2')[0] && textOf(page.find(node => node.type === 'h2')[0]), 'Travel helper');
  assert.match(page.text(), /Pat can read this session and pays for the answers\. Requests left today: 20\./u);
});

scenario('a link the person followed is looked up at once, and a service that is not theirs says only that', async context => {
  const f = fake(), { page } = await open(context, f, { initialServiceId: serviceId });
  assert.ok(page.has('Agree and open'));
  await page.click(page.button('Not now'));
  assert.ok(!page.has('Agree and open'));
  await page.type(page.find(node => node.type === 'input')[0], 's_missing_0001'); await page.submit();
  assert.match(page.text(), /There is no such service, or it is not offered to this account\./u);
});

scenario('asking, the answer, a refusal, the Bot’s question and cancelling', async context => {
  const f = fake(); f.state.view = { ...f.state.view, entered: true, generation: 1 };
  const { page, tick } = await open(context, f, { initialServiceId: serviceId });
  const box = () => page.oneByLabel('Message');
  // A refusal says what happened and that nothing was sent; the text stays in the box.
  f.state.refuse = 'SERVICE_BUSY';
  await page.type(box(), 'What should we pack?'); await page.submit(page.oneByClass('hosted-service-composer'));
  assert.match(page.text(), /busy with other people’s requests\. Nothing was sent/u);
  assert.equal(box().props.value, 'What should we pack?');
  f.state.refuse = '';
  await page.submit(page.oneByClass('hosted-service-composer'));
  assert.deepEqual(sentActions(f), ['createConversation', 'submit', 'submit']);
  const submit = f.state.sent.filter(c => c.action === 'submit').at(-1);
  assert.deepEqual([submit.serviceId, submit.generation, submit.payload.text, submit.payload.targets, 'teamId' in submit], [serviceId, 1, 'What should we pack?', [sharedBotId], false]);
  assert.equal(box().props.value, '');
  assert.match(page.text(), /Waiting for the publisher’s computer…/u);
  // The answer is the Bot's: it goes through the Markdown renderer. What the person typed is shown as typed.
  f.answer('**Passports.** Which city?', true); await tick();
  assert.deepEqual(page.find(node => node.props['data-md']).map(textOf), ['**Passports.** Which city?']);
  assert.ok(page.find(node => node.type === 'p' && textOf(node) === 'What should we pack?').length === 1);
  assert.match(page.text(), /The Bot asked you a question\. Answer below to continue\./u);
  assert.equal(box().props.placeholder, 'Answer the Bot’s question');
  // The reply to a question continues the waiting request instead of starting another.
  await page.type(box(), 'Lisbon'); await page.submit(page.oneByClass('hosted-service-composer'));
  assert.deepEqual(sentActions(f).slice(-1), ['answerWait']);
  assert.equal(f.state.sent.at(-1 - f.state.sent.slice().reverse().findIndex(c => c.action === 'answerWait')).payload.waitId, 'wait00001');
  await tick();
  await page.click(page.button('Cancel')); await tick();
  assert.deepEqual(sentActions(f).slice(-1), ['cancelTask']);
  assert.match(page.text(), /You cancelled this request\./u);
});

scenario('a service that cannot answer says why, and its sessions stay readable', async context => {
  const f = fake(); f.state.view = { ...f.state.view, entered: true, generation: 1, availability: 'host_offline' };
  const { page } = await open(context, f, { initialServiceId: serviceId });
  assert.match(page.text(), /The publisher’s computer is offline/u);
  assert.match(page.text(), /The service cannot answer right now\. You can still read your sessions\./u);
});

scenario('a report goes to LingXi with what the person chose to attach; leaving is confirmed', async context => {
  const f = fake(); f.state.view = { ...f.state.view, entered: true, generation: 1 };
  const { page } = await open(context, f, { initialServiceId: serviceId });
  await page.type(page.oneByLabel('Message'), 'Hello'); await page.submit(page.oneByClass('hosted-service-composer'));
  await page.click(page.button('Report'));
  assert.match(page.text(), /The report goes to LingXi, not to the publisher\./u);
  const reason = page.find(node => node.type === 'select' && node.props.value === 'harmful')[0];
  await page.during(() => reason.props.onChange({ target: { value: 'deceptive' } }));
  assert.ok(page.has('Send report')); await page.submit(page.find(node => node.type === 'form').at(-1));
  assert.equal(f.state.reports.length, 1);
  assert.deepEqual([f.state.reports[0].reason, f.state.reports[0].text, 'sessionId' in f.state.reports[0]], ['deceptive', '', false], 'the session is named only when the person ticks it');
  assert.match(page.text(), /The publisher is not told who reported\./u);
  await page.click(page.button('Close'));
  await page.click(page.button('Leave'));
  assert.match(page.text(), /Requests that have not started are stopped/u);
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Leave' && String(node.props.className).includes('tw-danger-btn'))[0]);
  assert.ok(f.state.commands.some(c => c.action === 'leaveService'));
  assert.match(page.text(), /Services you use/u); assert.match(page.text(), /None yet\./u);
});

scenario('offering a Bot: created off, published into the service’s own scope, switched on, withdrawn', async context => {
  const f = fake(true), { page, field } = await open(context, f);
  await page.click(page.button('Load local Bots'));
  const select = page.find(node => node.type === 'select')[0];
  assert.deepEqual(select.findAllByType('option').map(textOf), ['Choose a Bot', 'Trip planner · anthropic/synthetic'], 'a Bot that needs files bound is not offered');
  await page.during(() => select.props.onChange({ target: { value: 'instance001' } }));
  assert.equal(field('Name people see').props.value, 'Trip planner');
  await page.type(field('Published role'), 'You plan trips.');
  const create = () => page.button('Create the service');
  assert.equal(create().props.disabled, true, 'the publisher has to accept what offering means');
  await page.check(page.find(node => node.type === 'input' && node.props.type === 'checkbox')[0]);
  // A publication that fails leaves no service behind.
  f.state.publishFails = 'TEAM_MODEL_UNAVAILABLE';
  await page.submit(page.find(node => node.type === 'form')[1]);
  assert.match(page.text(), /This Bot’s model is not set up on this computer\./u);
  assert.deepEqual(f.state.published.map(service => service.state), ['withdrawn']);
  f.state.publishFails = '';
  await page.submit(page.find(node => node.type === 'form')[1]);
  const made = f.state.published.at(-1)!, input = f.state.published_inputs.at(-1);
  assert.ok(made.serviceId.startsWith('s_'));
  assert.deepEqual([made.online, made.audience, made.limits, made.retention], [false, { mode: 'public' }, { userConcurrent: 1, serviceConcurrent: 4 }, 'Sessions are stored by LingXi and are not deleted automatically.']);
  // The Bot is published where the service lives: the service's id in place of a Team, for everyone in that scope, with the role alone.
  assert.deepEqual([input.teamId, input.generation, input.sharedBotId, input.instanceId, input.usePolicy, input.fileTokens, input.quotas, input.role],
    [made.serviceId, 1, made.sharedBotId, 'instance001', { mode: 'all' }, [], { team: 200, member: 20, bot: 200 }, 'You plan trips.']);
  assert.match(page.text(), new RegExp(`https://lingxios\\.net/services/${made.serviceId}`, 'u'));
  assert.match(page.text(), /Off/u);
  await page.click(page.button('Turn on'));
  assert.equal(made.online, true); assert.ok(page.has('Turn off'));
  // Their own link is not a service they use: no terms, nothing entered.
  await page.type(page.find(node => node.type === 'input')[0], `https://lingxios.net/services/${made.serviceId}`); await page.submit(page.find(node => node.type === 'form')[0]);
  assert.match(page.text(), /This is a service you offer\./u);
  assert.ok(!page.has('Agree and open'));
  assert.equal(f.state.commands.some(c => c.action === 'enterService'), false);
  await page.click(page.button('Withdraw'));
  assert.match(page.text(), /This cannot be undone/u);
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Withdraw' && node.props.disabled !== undefined && node.parent.props.className === 'tw-dialog-actions')[0]);
  assert.equal(made.state, 'withdrawn');
  assert.ok(!page.has('Turn off'), 'a withdrawn service is no longer listed');
});
