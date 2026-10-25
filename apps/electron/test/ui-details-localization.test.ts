import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { withDesktopLocale } from './support/language';
import { DESKTOP_LOCALES, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { CodeBlock } from '../src/renderer/components/CodeBlock';
import { CommandOutput } from '../src/renderer/components/CommandOutput';
import { CommandResultPanel } from '../src/renderer/components/CommandResultPanel';
import { DiffView } from '../src/renderer/components/DiffView';
import { ErrorBanner } from '../src/renderer/components/ErrorBanner';
import { GoalStatus } from '../src/renderer/components/GoalStatus';
import { MarkdownContent } from '../src/renderer/components/MarkdownContent';
import { NeedsInputBadge } from '../src/renderer/components/NeedsInputBadge';
import { PlanDocument, PlanPreview } from '../src/renderer/components/PlanDocument';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { TranscriptAgents } from '../src/renderer/components/TranscriptAgents';
import { TurnFileReview, TurnFileSummary } from '../src/renderer/components/TurnFileSummary';
import { VisualizationCard, VisualizationContextBadge, VisualizationPlaceholder, VisualizationUnavailable } from '../src/renderer/components/VisualizationCard';
import { ModUiAbovePrompt } from '../src/renderer/components/modUiAbovePrompt';
import { ModUiClientTree } from '../src/renderer/components/modUiClientTree';
import type { StructuredDiffDto } from '@lingxi/bridge-client';
import type { ToolRunItem } from '../src/renderer/model/runItem';

(globalThis as { React?: typeof React }).React = React;

function render(locale: DesktopLocale, element: React.ReactElement): string {
  return withDesktopLocale(locale, () => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)));
}
const escaped = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));
function contains(html: string, locale: DesktopLocale, key: string, ...args: (string | number)[]): void {
  assert.ok(html.includes(escaped(translate(key, locale, ...args))), `${locale}: ${key}`);
}

const diff: StructuredDiffDto = {
  file_path: '/opaque/path.ts', additions: 4, removals: 2, gutter_width: 1, truncated_rows: 3, rows: [],
};

test('file and tool summaries translate counts, truncation, and accessible labels while preserving machine content', () => {
  const file = { path: '/opaque/path.ts', additions: 4, removals: 2, diffs: [diff, diff] };
  const tool: ToolRunItem = {
    type: 'tool', id: 'search', tool: 'WebSearch', status: 'done',
    view: { verb: 'generic', label: 'RawTool', title: 'Raw machine tool title' },
    result: { body: 'Raw engine output %1$s\nSecond line', body_lines: 9, body_truncated: true },
  };
  for (const locale of DESKTOP_LOCALES) {
    const summary = render(locale, React.createElement(TurnFileSummary, { files: [file] }));
    contains(summary, locale, 'desktop_ui_edited_one', 1);
    contains(summary, locale, 'desktop_ui_edit_counts', 4, 2);
    assert.ok(summary.includes('path.ts'));
    const review = render(locale, React.createElement(TurnFileReview, { files: [file] }));
    contains(review, locale, 'desktop_ui_edits', 2);
    contains(review, locale, 'desktop_ui_diff_more_lines', 3);
    const oneLine = render(locale, React.createElement(DiffView, { diff: { ...diff, truncated_rows: 1 } }));
    contains(oneLine, locale, 'desktop_ui_diff_more_line', 1);
    const call = render(locale, React.createElement(ToolCall, { item: tool, open: true, onSetOpen() {} }));
    contains(call, locale, 'desktop_ui_search_completed');
    contains(call, locale, 'desktop_ui_truncated_suffix');
    contains(call, locale, 'desktop_ui_truncated_lines', 2, 9);
    assert.ok(call.includes('Raw engine output %1$s'));
    assert.ok(call.includes('Raw machine tool title'));
    const group = render(locale, React.createElement(ToolGroup, {
      group: { type: 'tool-group', id: 'group', tools: [tool, { ...tool, id: 'failed', status: 'error' }] },
      open: true, toolOpen: () => false, onSetOpen() {},
    }));
    contains(group, locale, 'desktop_ui_failed_count', 1);
    contains(group, locale, 'chat_tool_batch_count %lld', 2);
  }
});

test('code, markdown, plan, goal, command, and notification controls render the selected language', () => {
  for (const locale of DESKTOP_LOCALES) {
    const code = render(locale, React.createElement(CodeBlock, { code: 'Raw code %2$s', language: 'plaintext', closed: false }));
    contains(code, locale, 'desktop_ui_plain_text');
    contains(code, locale, 'chat_copy_code');
    assert.ok(code.includes('Raw code %2$s'));
    const generic = render(locale, React.createElement(CodeBlock, { code: '', closed: false }));
    contains(generic, locale, 'drawer_tab_code');
    const markdown = render(locale, React.createElement(MarkdownContent, { text: '![Raw image alt %1$s](https://example.com/image.png)' }));
    contains(markdown, locale, 'desktop_ui_image_prefix');
    assert.ok(markdown.includes('Raw image alt %1$s'));
    const plan = render(locale, React.createElement(PlanDocument, { content: 'Raw plan %1$s' }));
    contains(plan, locale, 'plan_document_copy');
    assert.ok(plan.includes('Raw plan %1$s'));
    const preview = render(locale, React.createElement(PlanPreview, { content: '', status: 'pending', onOpen() {} }));
    contains(preview, locale, 'desktop_ui_plan_pending');
    contains(preview, locale, 'desktop_ui_approval_pending');
    const goal = render(locale, React.createElement(GoalStatus, {
      objective: 'Raw user goal %lld', disabled: false, running: true, cancelling: false,
      onClear: async () => {}, onPause: async () => {}, onResume: async () => {},
    }));
    contains(goal, locale, 'desktop_ui_goal_pursuing');
    contains(goal, locale, 'desktop_ui_goal_pause');
    assert.ok(goal.includes('Raw user goal %lld'));
    contains(render(locale, React.createElement(NeedsInputBadge)), locale, 'desktop_ui_needs_input');
    const item = { type: 'command' as const, id: 'command', name: '/goal', output: 'Raw command output %1$s', isError: false };
    const command = render(locale, React.createElement(CommandOutput, { item, open: true, onSetOpen() {} }));
    contains(command, locale, 'desktop_ui_goal');
    assert.ok(command.includes('Raw command output %1$s'));
    contains(render(locale, React.createElement(CommandResultPanel, { item, onClose() {} })), locale, 'desktop_ui_close_command');
    contains(render(locale, React.createElement(ErrorBanner, { bridge: { error: 'Raw backend error', clearError() {} } })), locale, 'desktop_ui_dismiss_notification');
  }
});

test('agent, visualization, and Mod surfaces translate app labels and preserve supplied names and trees', () => {
  for (const locale of DESKTOP_LOCALES) {
    const agents = render(locale, React.createElement(TranscriptAgents, {
      agents: [{ agent_id: 'agent-1', name: 'Raw agent name %lld', agent_type: 'explorer', status: 'waiting_for_input', latest_activity: 'Raw activity' }],
      onOpenAgent() {},
    }));
    contains(agents, locale, 'desktop_ui_agent_waiting');
    contains(agents, locale, 'desktop_ui_agent_open_details');
    assert.ok(agents.includes('Raw agent name %lld'));
    assert.ok(agents.includes('Raw activity'));
    contains(render(locale, React.createElement(VisualizationPlaceholder)), locale, 'visualization_preparing');
    contains(render(locale, React.createElement(VisualizationUnavailable)), locale, 'visualization_unavailable');
    contains(render(locale, React.createElement(VisualizationCard, { sessionId: 's', reference: { id: 'v', revision: 1 } })), locale, 'visualization_loading');
    const badge = render(locale, React.createElement(VisualizationContextBadge, { chip: { id: 'v', revision: 1, title: 'Raw chart name' }, onDismiss() {} }));
    contains(badge, locale, 'desktop_ui_visualization_about', 'Raw chart name');
    contains(badge, locale, 'visualization_remove_context');
    const mod = render(locale, React.createElement(ModUiAbovePrompt, { sessionId: 's', children: 'Raw host fallback' }));
    assert.ok(mod.includes('Raw host fallback'));
    const tree = render(locale, React.createElement(ModUiClientTree, {
      tree: { type: 'Text', children: ['Raw Mod tree text %1$s'] },
      site: { component: 'AbovePrompt', instanceId: 'site' }, client: { plugin: 'raw-plugin', key: 'raw-client', module: 'raw-module' }, onPress() {},
    }));
    assert.ok(tree.includes('Raw Mod tree text %1$s'));
    assert.ok(tree.includes('raw-plugin'));
  }
});
