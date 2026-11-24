import assert from 'node:assert/strict';
import { createElement } from 'react';
import type { TeamsApi } from '../../../packages/teams/index';
import { loadTeamsModule } from './support/loadTeamsModule';
import { hostNode, mount, scenario, withDom } from './support/workspacePage';

// The personal datasets, shown in place on the Desktop Datasets page and in a dialog from a chat.
const { PersonalChatData, PersonalDataPanel } = loadTeamsModule<typeof import('../../../packages/teams/PersonalChatData')>('PersonalChatData');

const still = { subscribe: () => () => {}, getSnapshot: () => 0 };
function fake() {
  const asked: string[] = [];
  const api = {
    watchMessages: () => () => {},
    data: async (request: { command: { action: string } }) => {
      asked.push(request.command.action);
      if (request.command.action === 'datasets') return { datasets: [{ dataset: 'orders' }, { dataset: 'notes' }] };
      throw new Error('unexpected ' + request.command.action);
    },
  } as unknown as TeamsApi;
  return { api, asked };
}
const options = (page: Awaited<ReturnType<typeof mount>>) => page.find(node => node.type === 'option').map(node => String(node.props.value));

scenario('the panel lists the personal datasets as soon as it appears', async context => {
  withDom(context);
  const { api, asked } = fake();
  const page = await mount(still, () => createElement(PersonalDataPanel, { api, uid: 'u1', teams: [], locale: 'en' }));
  assert.deepEqual(asked, ['datasets']);
  assert.deepEqual(options(page), ['', 'orders', 'notes']);
  assert.equal(page.find(node => node.type === 'dialog').length, 0, 'the panel is the content, not a dialog');
});

scenario('a host without the data service shows no panel', async context => {
  withDom(context);
  const page = await mount(still, () => createElement(PersonalDataPanel, { api: {} as TeamsApi, uid: 'u1', teams: [], locale: 'en' }));
  assert.equal(page.text(), '');
});

scenario('the chat action loads the panel each time its dialog opens and lets it go on close', async context => {
  withDom(context);
  const { api, asked } = fake();
  const nodes: ReturnType<typeof hostNode>[] = [];
  const page = await mount(still, () => createElement(PersonalChatData, { api, uid: 'u1', teams: [], locale: 'en' }),
    () => { const node = hostNode(); nodes.push(node); return node; });
  assert.deepEqual(asked, [], 'nothing loads while the dialog is closed');
  assert.deepEqual(options(page), []);
  await page.click(page.button('Personal data'));
  assert.deepEqual(asked, ['datasets']);
  assert.deepEqual(options(page), ['', 'orders', 'notes']);
  const dialog = page.find(node => node.type === 'dialog')[0];
  const shown = nodes.find(node => node.open === true);
  assert.ok(shown, 'the dialog is shown as a modal');
  // The browser closes the dialog and then sends `close`.
  await page.during(() => { (shown.close as () => void)(); dialog.props.onClose({ target: shown, currentTarget: shown }); });
  assert.deepEqual(options(page), [], 'a closed dialog keeps no dataset state');
  await page.click(page.button('Personal data'));
  assert.deepEqual(asked, ['datasets', 'datasets'], 'reopening loads afresh');
});
