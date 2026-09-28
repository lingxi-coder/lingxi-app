import { test } from 'node:test';
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
import { RuntimeCenterInspector } from '../src/renderer/components/RuntimeCenter';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

// Match the existing render tests under the root tsconfig's classic JSX loader.
(globalThis as { React?: typeof React }).React = React;

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

function begin(): RuntimeCenterState {
  return beginSideQuestion(emptyRuntimeCenterState(), SESSION_ID, 1, `/btw ${QUESTION}`);
}

test('/btw opens the existing subagent inspector with the question and running status', () => {
  const html = renderInspector(begin());
  assert.match(html, /aria-label="Runtime inspector"/);
  assert.match(html, /role="tab"[^>]*aria-selected="true"/);
  assert.match(html, />\/btw<\/span>/);
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

test('/btw remains available alongside existing subagents in the ordinary subagent list', () => {
  const initial = emptyRuntimeCenterState();
  const state = beginSideQuestion({ ...initial, agents: {
    reviewer: { agent_id: 'reviewer', name: 'Existing reviewer', agent_type: 'general-purpose', status: 'running' },
  } }, SESSION_ID, 1, `/btw ${QUESTION}`);
  const completed = finishSideQuestion(state, SESSION_ID, 1, 'One verification task remains.');
  const list = renderInspector(openRuntimeCenterItem(completed, { kind: 'section', id: 'agents' }));
  assert.ok(list.includes('Existing reviewer'));
  assert.ok(list.includes('/btw'));
  assert.match(list, />done<\/span>/);
  assert.match(list, />running<\/span>/);
  const reopened = renderInspector(openRuntimeCenterItem(completed, { kind: 'agent', id: 'btw:1' }));
  assert.ok(reopened.includes(QUESTION));
  assert.ok(reopened.includes('One verification task remains.'));
});
