import assert from 'node:assert/strict';
import { createElement } from 'react';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { TeamsApi } from '../../../packages/teams';
import type { LocalPublishableBot, PublishLocalBotInput, SharedBotCard } from '../../../packages/teams/execution';
import { FakeTeams, botCard, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom } from './support/workspacePage';

const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');

// Bot v3 B8: what a Bot takes from other Bots, and whom it may ask, is chosen where the Bot is published.
const local: LocalPublishableBot = { instanceId: 'local-new', name: 'Travel', description: '', model: 'Configured model' };
const beta = botCard({ sharedBotId: 'sharedbot_beta', name: 'Beta', maintainer: { uid: 'bob', generation: 1 }, instanceId: 'local-beta' });
const gamma = botCard({ sharedBotId: 'sharedbot_gamma', name: 'Gamma', maintainer: { uid: 'carol', generation: 1 }, instanceId: 'local-gamma' });
const FIELDS = ['acceptDelegation', 'acceptFrom', 'delegateTo', 'delegation'] as const;

async function wizard(context: Parameters<Parameters<typeof scenario>[1]>[0], cards: SharedBotCard[], friend?: string) {
  withDom(context);
  const sentInputs: PublishLocalBotInput[] = [];
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = [];
  Object.assign(fake.api, { localBots: async () => [local], selectSharedFiles: async () => [], publishLocalBot: async (input: PublishLocalBotInput) => { sentInputs.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } } satisfies Partial<TeamsApi>);
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail);
  fake.bots = cards; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === cards.length, 'the Bots');
  const page = await mount(workspace, s => createElement(BotDirectory, { api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots, ...(friend ? { friend } : {}) }));
  context.after(() => page.unmount());
  await page.click(page.button('Load local Bots'));
  const choose = async () => { await page.type(page.find(node => node.type === 'select')[0], 'local-new'); };
  const review = async () => { await page.type(page.find(node => node.type === 'textarea')[0], 'Help prepare a family trip.'); await page.submit(); };
  const consent = () => page.find(node => node.type === 'input' && node.props.type === 'checkbox').at(-1);
  const fieldsetOf = (node: any): string => { for (let at = node.parent; at; at = at.parent) if (at.type === 'fieldset') return textOf(at); return ''; };
  /** The checkbox of a Bot in one list: the same Bot can be in the list it is taken from and in the one it is asked. */
  const box = (legend: string, name: string) => {
    const found = page.find(node => node.type === 'input' && node.props.type === 'checkbox' && textOf(node.parent) === name && fieldsetOf(node).startsWith(legend));
    assert.equal(found.length, 1, `one checkbox "${name}" under "${legend}"`);
    return found[0];
  };
  const accept = () => page.find(node => node.type === 'select' && textOf(node.parent).startsWith('Requests this Bot takes from other Bots'))[0];
  const daily = () => page.find(node => node.type === 'input' && node.props.type === 'number' && textOf(node.parent).startsWith('Requests to other Bots per UTC day'));
  const sent = async () => { await page.check(consent()); await page.click(page.button('Publish Bot')); assert.equal(sentInputs.length, 1); return sentInputs[0]!; };
  return { page, choose, review, consent, box, accept, daily, sent };
}

scenario('Publishing: a Bot that takes nothing from other Bots and asks none is published as before, and the controls say what they are for', async context => {
  const { page, choose, review, sent } = await wizard(context, [beta, gamma]);
  await choose();
  assert.match(page.text(), /Bots asking Bots/); assert.match(page.text(), /Both Bots must agree\./);
  assert.match(page.text(), /never what this Bot may use\./, 'the other Bot never gets what this Bot may use');
  await review();
  assert.doesNotMatch(page.text(), /Takes requests from|May ask:/, 'nothing to review of it');
  const input = await sent();
  assert.deepEqual(FIELDS.filter(field => field in input), [], 'a publication without them takes none and asks none');
});

scenario('Publishing: taking requests from any Bot of the Team and asking chosen Bots with a daily limit sends exactly those, and the review names them', async context => {
  const { page, choose, review, box, accept, daily, sent } = await wizard(context, [beta, gamma]);
  await choose();
  assert.equal(accept().props.value, 'none'); assert.equal(daily().length, 0, 'no limit to set until a Bot may be asked');
  await page.type(accept(), 'team_bots');
  assert.equal(page.find(node => node.type === 'fieldset' && textOf(node).startsWith('Takes requests from')).length, 0, 'every Bot of the Team: none to choose');
  await page.check(box('May ask these Bots', 'Beta'));
  assert.equal(daily().length, 1); assert.equal(daily()[0].props.value, 10, 'ten a day unless it is changed');
  await page.type(daily()[0], '5');
  await review();
  assert.match(page.text(), /Takes requests from any Bot of the Team\./); assert.match(page.text(), /May ask: Beta, at most 5 requests a day\./);
  assert.deepEqual(await sent().then(({ acceptDelegation, acceptFrom, delegateTo, delegation }) => ({ acceptDelegation, acceptFrom, delegateTo, delegation })),
    { acceptDelegation: 'team_bots', acceptFrom: undefined, delegateTo: ['sharedbot_beta'], delegation: { daily: 5 } });
});

scenario('Publishing: only from the Bots chosen needs a Bot chosen, and the same Bot can be taken from and asked', async context => {
  const { page, choose, review, box, accept, sent } = await wizard(context, [beta, gamma]);
  await choose(); await page.type(page.find(node => node.type === 'textarea')[0], 'Help prepare a family trip.');
  assert.equal(page.button('Review publication').props.disabled, false, 'nothing else is missing');
  await page.type(accept(), 'listed');
  assert.equal(page.button('Review publication').props.disabled, true, 'a list of nobody is not a list');
  await page.check(box('Takes requests from', 'Gamma')); await page.check(box('Takes requests from', 'Beta'));
  assert.equal(page.button('Review publication').props.disabled, false);
  await page.check(box('May ask these Bots', 'Gamma'));
  await review();
  assert.match(page.text(), /Takes requests from: Gamma, Beta/); assert.match(page.text(), /May ask: Gamma, at most 10 requests a day\./);
  const input = await sent();
  assert.deepEqual([input.acceptDelegation, input.acceptFrom, input.delegateTo, input.delegation], ['listed', ['sharedbot_gamma', 'sharedbot_beta'], ['sharedbot_gamma'], { daily: 10 }]);
});

scenario('Publishing: a change after the review asks for the review again', async context => {
  const { page, choose, review, consent, box } = await wizard(context, [beta]);
  await choose(); await review(); await page.check(consent());
  assert.equal(page.button('Publish Bot').props.disabled, false);
  await page.check(box('May ask these Bots', 'Beta'));
  assert.match(page.text(), /Publication inputs or memberships changed\. Review again before publishing\./); assert.equal(page.button('Publish Bot').props.disabled, true);
});

scenario('Publishing again keeps what the Bot takes and whom it may ask, and what is ticked of a Bot that is gone is not sent', async context => {
  const own = botCard({ sharedBotId: 'sharedbot_own', name: 'Travel', maintainer: { uid: 'alice', generation: 1 }, instanceId: 'local-new', acceptDelegation: 'listed', acceptFrom: ['sharedbot_beta'],
    delegateTo: ['sharedbot_gamma', 'sharedbot_gone'], delegation: { daily: 7 } });
  const { page, choose, review, box, accept, daily, sent } = await wizard(context, [own, beta, gamma]);
  await choose();
  assert.equal(accept().props.value, 'listed');
  assert.deepEqual([['Takes requests from', 'Beta'], ['Takes requests from', 'Gamma'], ['May ask these Bots', 'Beta'], ['May ask these Bots', 'Gamma']].map(([legend, name]) => box(legend!, name!).props.checked), [true, false, false, true]);
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && textOf(node.parent) === 'Travel').length, 0, 'a Bot is not asked to ask itself');
  assert.equal(daily()[0].props.value, 7);
  await review();
  const input = await sent();
  assert.deepEqual([input.acceptDelegation, input.acceptFrom, input.delegateTo, input.delegation], ['listed', ['sharedbot_beta'], ['sharedbot_gamma'], { daily: 7 }]);
  assert.equal(input.expectedRevision, 1);
});

scenario('Publishing: a friendship\'s Bots are not asked about asking other Bots', async context => {
  const friends = await wizard(context, [beta], 'Bob');
  await friends.choose();
  assert.doesNotMatch(friends.page.text(), /Bots asking Bots/);
});

scenario('Publishing: a Team with no other Bot offers nobody to ask and nothing to choose from', async context => {
  const { page, choose, review, accept, sent } = await wizard(context, []);
  await choose();
  assert.match(page.text(), /No other Bot of this Team is published yet\./);
  assert.equal(page.find(node => node.type === 'fieldset' && textOf(node).startsWith('May ask these Bots')).length, 0);
  await page.type(accept(), 'team_bots'); await review();
  const input = await sent();
  assert.deepEqual([input.acceptDelegation, 'delegateTo' in input], ['team_bots', false], 'it can still take requests; there is nobody to ask');
});
