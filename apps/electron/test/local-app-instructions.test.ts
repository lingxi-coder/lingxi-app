import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { LocalAppInstructions } from '../src/renderer/components/LocalAppInstructions';
import { skillInstructionNames } from '../src/renderer/components/localAppInstructionPresentation';
import { conversationFromMessages, prependHistory } from '../src/renderer/bridge/conversation';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
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
  assert.deepEqual([...skillInstructionNames([skill, body])], [['instructions', 'local-app']]);
  assert.deepEqual([...skillInstructionNames([skill, { type: 'thinking', id: 't', text: '' }, body])], [['instructions', 'local-app']]);
  for (const items of [[body], [skill, { ...body, text: 'Create a clock' }],
    [skill, { ...body, delivery: 'pending' as const }],
    [{ ...skill, status: 'error' as const }, body],
    [{ ...skill, view: { ...skill.view, primary: 'another-skill' } }, body],
    [skill, { ...body, role: 'assistant' as const }, body]]) {
    assert.equal(skillInstructionNames(items).size, 0);
  }
});

test('other skill prompts are recognized from their tool output without hiding unrelated user messages', () => {
  const text = '# LingXi API guide\n\nComplete instructions.';
  const api: ToolRunItem = { ...skill, view: { ...skill.view, primary: 'lingxi-api' }, nativeOutput: { body: text } };
  const prompt = { ...body, text };
  assert.deepEqual([...skillInstructionNames([api, prompt])], [['instructions', 'lingxi-api']]);
  assert.equal(skillInstructionNames([api, { ...prompt, text: '# A different request\n\nPlease help.' }]).size, 0);
  const html = render(React.createElement(Stage, { liveItems: [api, prompt], sessionKey: 'test' }));
  assert.match(html, /data-run-type="skill-instructions"/);
  assert.match(html, /lingxi-api/);
  assert.doesNotMatch(html, /Complete instructions\.|user-message-bubble/);
});

test('file-based skill instructions stay collapsed with current and original directory preambles', () => {
  const text = '# LingXi API guide\n\nHelp the user understand or use LingXi.\n\n## Topics\n\nEND_OF_API_GUIDE';
  const api: ToolRunItem = { ...skill, view: { ...skill.view, primary: 'lingxi-api' }, nativeOutput: { body: text } };
  const directory = 'Base directory for this skill: /Applications/LingXi.app/Contents/Resources/skills/lingxi-api\n';
  const resolution = 'Resolve relative references and assets from this directory; use their exact paths instead of searching the application home directory. Use read-only file tools for skill assets; keep project writes in the project directory.\n';
  for (const preamble of ['', `${directory}\n`, `${directory}${resolution}\n`]) {
    for (const newline of ['\n', '\r\n']) {
      const prompt = { ...body, text: `${preamble}${text}`.replace(/\n/g, newline) };
      const rows = transcriptRows([api, prompt], false);
      assert.equal(rows[1]?.type, 'skill-instructions');
      assert.equal('role' in rows[1]!, false, 'instructions do not create a user-turn boundary');
      const html = render(React.createElement(Stage, { liveItems: [api, prompt], sessionKey: 'preamble' }));
      assert.match(html, /data-row-type="skill-instructions"[^>]*data-message-uuid="instruction-row"/);
      assert.match(html, /aria-expanded="false"/);
      assert.doesNotMatch(html, /END_OF_API_GUIDE|user-message-bubble|Show more|data-message-role="user"/);
      const opened = render(React.createElement(LocalAppInstructions, {
        id: prompt.id, text: prompt.text, name: 'lingxi-api', open: true, onSetOpen() {},
      }));
      assert.match(opened, /END_OF_API_GUIDE/);
      assert.doesNotMatch(opened, /Base directory for this skill|Resolve relative references|\/Applications\//);
    }
  }
  const unrelated = { ...body, text: `${directory}${resolution}\n# My own request\n\nKeep this visible.` };
  assert.equal(skillInstructionNames([api, unrelated]).size, 0);
  assert.match(render(React.createElement(Stage, { liveItems: [api, unrelated], sessionKey: 'ordinary' })), /user-message-bubble/);
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

test('current local-app document stays collapsed when the tool body is unavailable', () => {
  const text = '# Local apps\n\nCreate an application project, publish it into this device\'s Apps library, and verify the published application.\n\n## Choose the execution host and interfaces\n\nCurrent instructions.';
  for (const newline of ['\n', '\r\n']) {
    const prompt = { ...body, text: text.replace(/\n/g, newline) };
    const html = render(React.createElement(Stage, { liveItems: [skill, prompt], sessionKey: 'current-local-app' }));
    assert.match(html, /data-run-type="skill-instructions"/);
    assert.match(html, /aria-expanded="false"/);
    assert.doesNotMatch(html, /user-message-bubble|Show more|Choose the execution host/);
    assert.equal(skillInstructionNames([{ ...skill, view: { ...skill.view, primary: 'another-skill' } }, prompt]).size, 0);
    assert.equal(skillInstructionNames([{ ...skill, status: 'error' }, prompt]).size, 0);
    assert.equal(skillInstructionNames([prompt]).size, 0);
  }
});


test('host Skill identity survives restored text-only results, renamed documents and history paging', () => {
  const messages = [
    { role: 'assistant', blocks: [{ type: 'tool_use' as const, id: 'api', tool: 'Skill',
      input_json: '{"skill":"lingxi-api"}', header: { ...skill.view, primary: 'lingxi-api' } }] },
    { role: 'user', blocks: [{ type: 'tool_result' as const, id: 'api', tool: 'Skill',
      result_json: '"Launching skill: lingxi-api"', is_error: false,
      display: { headline: 'Launching skill: lingxi-api' } }] },
    { role: 'user', skill_name: 'lingxi-api', transcript_uuid: 'api-guide-row',
      blocks: [{ type: 'text' as const, text: '# Renamed guide\n\nCompletely revised instructions.' }] },
  ];
  for (const state of [conversationFromMessages(messages),
    prependHistory(conversationFromMessages(messages.slice(2)), messages.slice(0, 2), 'older')]) {
    const rows = transcriptRows(state.items, false);
    assert.equal(rows.at(-1)?.type, 'skill-instructions');
    const html = render(React.createElement(Stage, { liveItems: state.items, sessionKey: 'restored-skill' }));
    assert.match(html, /data-run-type="skill-instructions"/);
    assert.match(html, /data-message-uuid="api-guide-row"/);
    assert.doesNotMatch(html, /user-message-bubble|Show more|Completely revised instructions/);
  }
  const ordinary = conversationFromMessages([{ ...messages[2]!, skill_name: undefined }]);
  assert.equal(transcriptRows(ordinary.items, false)[0]?.type, 'narration');
});
