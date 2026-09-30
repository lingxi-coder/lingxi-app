import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { AgentAvatar, agentAvatarIndex } from '../src/renderer/components/AgentAvatar';
import { TranscriptAgents } from '../src/renderer/components/TranscriptAgents';
import { RuntimeCenterInspector } from '../src/renderer/components/RuntimeCenter';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const render = (node: React.ReactElement, dark = false) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, node));
const identities = (html: string) => [...html.matchAll(/data-agent-avatar="(\d+)"/g)].map((match) => Number(match[1]));

test('all 28 stable identities have self-contained light and dark SVG assets', () => {
  const seen = new Set<number>();
  for (let i = 0; i < 300; i++) seen.add(agentAvatarIndex(`agent-${i}`));
  assert.equal(seen.size, 28);
  for (const index of seen) {
    const id = Array.from({ length: 300 }, (_, i) => `agent-${i}`).find((id) => agentAvatarIndex(id) === index)!;
    for (const dark of [false, true]) {
      const html = render(React.createElement(AgentAvatar, { agentId: id }), dark);
      const src = html.match(/src="([^"]+)"/)![1];
      assert.ok(src.endsWith(`-${dark ? 'dark' : 'light'}.svg`));
      const svg = readFileSync(new URL(src), 'utf8');
      assert.match(svg, /<svg\b/);
      assert.doesNotMatch(svg, /<script|<foreignObject|(?:href|src)=["']https?:/i);
      assert.match(html, /alt="" aria-hidden="true"/);
    }
  }
});

test('agent identity survives renames and terminal status and matches inspector rows and tabs', () => {
  const agent = { agent_id: 'agent-review-42', name: 'Review', agent_type: 'reviewer', status: 'running' };
  const expected = [agentAvatarIndex(agent.agent_id)];
  for (const status of ['running', 'completed', 'failed', 'cancelled']) {
    const row = { ...agent, name: `Renamed ${status}`, status };
    assert.deepEqual(identities(render(React.createElement(TranscriptAgents, { agents: [row] }))), expected);
  }
  for (const kind of ['section', 'agent'] as const) {
    const active = { kind, id: kind === 'section' ? 'agents' : agent.agent_id };
    const bridge = {
      runtimeCenter: { ...emptyRuntimeCenterState(), inspectorOpen: true, activeItem: active, tabs: [active], agents: { [agent.agent_id]: agent } },
      desktop: emptyDesktopState(), conversation: { plan: [] },
    };
    assert.deepEqual(identities(render(React.createElement(RuntimeCenterInspector, { bridge: bridge as never }))), expected);
  }
});
