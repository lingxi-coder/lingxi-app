import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { AskUserQuestionRequestDto, ToolHeaderDto } from '@lingxi/bridge-client';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import {
  askUserQuestionWithResponseProps,
  assistantFirstOfReplyByItem,
  nativeAskUserQuestionProps,
  nativeCommandOutputProps,
  nativeNarrationProps,
  nativeToolGroupProps,
  nativeToolResultProps,
  nativeToolUseProps,
} from '../src/renderer/components/nativeUiSiteProps';

const header: ToolHeaderDto = { verb: 'read', label: 'Read', title: 'Read(file)' };

test('Native AskUserQuestion site carries only the actual question request facts', () => {
  const request: AskUserQuestionRequestDto = {
    request_id: 12,
    timeout_secs: 30,
    questions: [{
      question: 'Which one?', header: 'Choose', multi_select: false,
      options: [{ label: 'A', description: 'first', preview: 'preview' }],
    }],
  };
  assert.deepEqual(nativeAskUserQuestionProps(request), {
    tool: 'AskUserQuestion',
    questions: [{
      question: 'Which one?', header: 'Choose', multi_select: false,
      options: [{ label: 'A', description: 'first', preview: 'preview' }],
    }],
  });
  assert.deepEqual(askUserQuestionWithResponseProps(request, {
    questions: [{ question: 'Rewritten?', header: 'New', multi_select: true, options: [] }],
  }).questions[0]?.question, 'Rewritten?');
  assert.equal(askUserQuestionWithResponseProps(request, { questions: 'invalid' }).questions[0]?.question, 'Which one?');
});

test('Native tool sites preserve raw input/result alongside the existing derived transcript view', () => {
  const running: ToolRunItem = {
    type: 'tool', id: 'toolu-1', tool: 'Read', status: 'running', view: header,
    nativeInput: { file_path: 'src/main.rs' },
  };
  const settled: ToolRunItem = {
    ...running,
    status: 'done',
    nativeOutput: { content: 'contents' },
  };
  assert.deepEqual(nativeToolUseProps(running), {
    tool_use_id: 'toolu-1', tool: 'Read', input: { file_path: 'src/main.rs' },
    isRunning: true, isErrored: false,
  });
  assert.deepEqual(nativeToolUseProps(settled), {
    tool_use_id: 'toolu-1', tool: 'Read', input: { file_path: 'src/main.rs' },
    isRunning: false, isErrored: false, output: { content: 'contents' },
  });
  assert.deepEqual(nativeToolResultProps(settled), {
    tool_use_id: 'toolu-1', tool: 'Read', output: { content: 'contents' }, isErrored: false,
  });
  assert.deepEqual(nativeToolGroupProps({ type: 'tool-group', id: 'g', tools: [running, settled] }, true), {
    calls: [nativeToolUseProps(running), nativeToolUseProps(settled)], isActive: true, isExpanded: true,
  });
});

test('Native message, command output, and first-of-reply props come from visible rows', () => {
  const first = { type: 'narration', id: 'a1', role: 'assistant', text: 'answer', streamed: true } as const;
  const second = { type: 'narration', id: 'a2', role: 'assistant', text: 'continued' } as const;
  const user = { type: 'narration', id: 'u1', role: 'user', text: 'next question' } as const;
  const nextReply = { type: 'narration', id: 'a3', role: 'assistant', text: 'next answer' } as const;
  const firstFlags = assistantFirstOfReplyByItem([first, second, user, nextReply]);
  assert.equal(firstFlags.get('a1'), true);
  assert.equal(firstFlags.get('a2'), false);
  assert.equal(firstFlags.get('a3'), true);
  assert.deepEqual(nativeNarrationProps(first, true), { text: 'answer', isFirstOfReply: true });
  assert.deepEqual(nativeNarrationProps(user, false), { text: 'next question' });
  assert.deepEqual(nativeCommandOutputProps({
    type: 'command', id: 'c1', name: '/status', args: 'detail', output: 'ready', isError: false,
  }), { command: '/status', args: 'detail', text: 'ready', isErrored: false });
});
