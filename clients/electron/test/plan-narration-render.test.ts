import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { narrationPlans } from '../src/renderer/bridge/planDocuments';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { RunItem } from '../src/renderer/model/runItem';
(globalThis as { React?: typeof React }).React = React;
test('structured historical proposal uses the Plan card instead of collapsed narration', () => {
  const text = '下面给出完整修改方案。\n\n# 环境切换 / org info 失效 — 修改方案\n\n## 不变量\n\n保持一致。\n\n## 实现步骤\n\n' + '具体实施步骤\n'.repeat(90);
  const items: RunItem[] = [{ type: 'narration', id: 'proposal', role: 'assistant', text }];
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
    React.createElement(Stage, { sessionKey: 'restored', liveItems: items, submittedPlans: narrationPlans(items) })));
  assert.match(html, /Open full plan/);
  assert.match(html, /Copy plan/);
  assert.match(html, /环境切换/);
  assert.doesNotMatch(html, /Show more/);
});
