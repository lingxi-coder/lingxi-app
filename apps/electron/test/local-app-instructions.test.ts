import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { LocalAppInstructions } from '../src/renderer/components/LocalAppInstructions';
import { localAppInstructionIds } from '../src/renderer/components/localAppInstructionPresentation';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { RunItem, ToolRunItem } from '../src/renderer/model/runItem';

(globalThis as { React?: typeof React }).React = React;
const skill: ToolRunItem = {
  type: 'tool', id: 'skill', tool: 'Skill', status: 'done',
  view: { verb: 'skill', label: 'Skill', title: 'Skill(local-app)', primary: 'local-app' },
  result: { headline: 'Launching skill: local-app' },
};
const body: RunItem = { type: 'narration', id: 'instructions', role: 'user', strong: true,
  text: '# Desktop Local Apps\n\nBuild a real application project.\n\n## Workflow\n\nKeep saved data.\n\nEND_OF_INSTRUCTIONS',
  transcriptUuid: 'instruction-row' };
function render(element: React.ReactElement, dark = false) {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, element));
}

test('only the adjacent desktop skill companion receives instruction presentation', () => {
  assert.deepEqual([...localAppInstructionIds([skill, body])], ['instructions']);
  assert.deepEqual([...localAppInstructionIds([skill, { type: 'thinking', id: 't', text: '' }, body])], ['instructions']);
  for (const items of [[body], [skill, { ...body, text: 'Create a clock' }],
    [skill, { ...body, delivery: 'pending' as const }],
    [{ ...skill, status: 'error' as const }, body],
    [{ ...skill, view: { ...skill.view, primary: 'another-skill' } }, body],
    [skill, { ...body, role: 'assistant' as const }, body]]) {
    assert.equal(localAppInstructionIds(items).size, 0);
  }
});

test('skill instructions start collapsed without a user bubble and retain the transcript anchor', () => {
  const html = render(React.createElement(Stage, { liveItems: [skill, body], sessionKey: 'test' }));
  assert.match(html, /data-message-uuid="instruction-row"/);
  assert.match(html, /data-run-type="skill-instructions"/);
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /user-message-bubble|Show more|END_OF_INSTRUCTIONS/);
  const ordinary = render(React.createElement(Stage, { liveItems: [body], sessionKey: 'test' }));
  assert.match(ordinary, /user-message-bubble/);
});

test('expanded instructions keep the entire document readable in both themes', () => {
  for (const dark of [false, true]) {
    const html = render(React.createElement(LocalAppInstructions, {
      id: body.id, text: body.text, open: true, onSetOpen() {},
    }), dark);
    assert.match(html, /aria-expanded="true"/);
    assert.match(html, /END_OF_INSTRUCTIONS/);
    assert.match(html, /font-size:13px/);
    assert.doesNotMatch(html, /2.15em|hidden=""/);
  }
});
