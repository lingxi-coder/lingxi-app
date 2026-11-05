import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import {
  emptyRuntimeCenterState,
  openRuntimeCenterItem,
  type RuntimeCenterState,
} from '../src/renderer/bridge/runtimeCenterState';
import { beginSideQuestion, finishSideQuestion } from '../src/renderer/bridge/sideQuestion';
import type { UseBridge } from '../src/renderer/bridge/bridgeTypes.js';
import { RuntimeCenterInspector, RuntimeCenterOverview } from '../src/renderer/components/RuntimeCenter';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { installTestLocale } from './test-locale';

// Match the existing render tests under the root tsconfig's classic JSX loader.
(globalThis as { React?: typeof React }).React = React;
let restoreLocale: () => void;
before(() => { restoreLocale = installTestLocale('en'); });
after(() => restoreLocale());

const SESSION_ID = 'btw-render-session';
const QUESTION = 'Which tasks remain unfinished?';

function renderInspector(runtimeCenter: RuntimeCenterState): string {
  const bridge = {
    runtimeCenter,
    desktop: emptyDesktopState(),
    conversation: { sessionKey: SESSION_ID, plan: [] },
  } as unknown as UseBridge;
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
    React.createElement(RuntimeCenterInspector, { bridge }),
  ));
}

function renderOverview(runtimeCenter: RuntimeCenterState): string {
  const bridge = {
    runtimeCenter: { ...runtimeCenter, overviewOpen: true },
    desktop: emptyDesktopState(),
    conversation: { sessionKey: SESSION_ID, plan: [] },
  } as unknown as UseBridge;
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
    React.createElement(RuntimeCenterOverview, { bridge }),
  ));
}

function begin(): RuntimeCenterState {
  return beginSideQuestion(emptyRuntimeCenterState(), SESSION_ID, 1, `/btw ${QUESTION}`);
}

test('/btw opens the existing subagent inspector with the question and running status', () => {
  const state = begin();
  const html = renderInspector(state);
  assert.match(html, /aria-label="Runtime inspector"/);
  assert.deepEqual(state.activeItem, { kind: 'agent', id: 'btw:1' });
  assert.deepEqual(state.tabs, [{ kind: 'agent', id: 'btw:1' }]);
  assert.match(html, /role="tabpanel"[^>]*aria-labelledby="runtime-inspector-tab-agent%3Abtw%3A1"/);
  assert.ok(html.includes(QUESTION));
  assert.match(html, />running<\/span>/);
  assert.match(html, /role="status"/);
  assert.ok(!html.includes('Waiting for the agent to emit its first message.'));
});

test('/btw completion renders Markdown through the existing subagent transcript', () => {
  const html = renderInspector(finishSideQuestion(begin(), SESSION_ID, 1, 'Finish **verification** and run `npm test`.'));
  assert.ok(html.includes(QUESTION));
  assert.match(html, />done<\/span>/);
  assert.ok(html.includes('verification</span>'));
  assert.match(html, /<code[^>]*>npm test<\/code>/);
  assert.ok(!html.includes('**verification**'));
  assert.ok(!html.includes('>running</span>'));
});

test('/btw errors retain the question and show the failure response', () => {
  const html = renderInspector(finishSideQuestion(begin(), SESSION_ID, 1, 'The side question request timed out.', true));
  assert.ok(html.includes(QUESTION));
  assert.ok(html.includes('The side question request timed out.'));
  assert.match(html, />failed<\/span>/);
  assert.ok(!html.includes('>running</span>'));
});

test('/btw stays available in its tab while ordinary subagents occupy their own list', () => {
  const initial = emptyRuntimeCenterState();
  const state = beginSideQuestion({ ...initial, agents: {
    reviewer: { agent_id: 'reviewer', name: 'Existing reviewer', agent_type: 'general-purpose', status: 'running' },
  } }, SESSION_ID, 1, `/btw ${QUESTION}`);
  const completed = finishSideQuestion(state, SESSION_ID, 1, 'One verification task remains.');
  const agentsOpen = openRuntimeCenterItem(completed, { kind: 'section', id: 'agents' });
  const list = renderInspector(agentsOpen);
  assert.ok(list.includes('Existing reviewer'));
  assert.ok(!list.includes('/btw'));
  assert.match(list, /No completed subagents/);
  assert.match(list, />running<\/span>/);
  const overview = renderOverview(agentsOpen);
  assert.match(overview, /aria-label="Side chats"[^>]*>[\s\S]*?\/btw/);
  const reopenedState = openRuntimeCenterItem(agentsOpen, { kind: 'agent', id: 'btw:1' });
  assert.deepEqual(reopenedState.activeItem, { kind: 'agent', id: 'btw:1' });
  assert.equal(reopenedState.tabs.filter((tab) => tab.kind === 'agent' && tab.id === 'btw:1').length, 1);
  const reopened = renderInspector(reopenedState);
  assert.ok(reopened.includes(QUESTION));
  assert.ok(reopened.includes('One verification task remains.'));
});
