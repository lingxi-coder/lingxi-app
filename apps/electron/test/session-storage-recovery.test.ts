import { withDesktopLocale } from './support/language';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { validateClientEvent } from '../../../packages/bridge-client/src/validation';
import { emptyDesktopState, reduceDesktopEvent } from '../src/renderer/bridge/desktopState';
import { classifyDesktopError } from '../src/renderer/bridge/errors';
import { SessionStorageNotice } from '../src/renderer/components/SessionStorageNotice';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { TRANSCRIPT_REPLAY_EVENTS } from '../src/main/bridgeEvents';

Object.assign(globalThis, { React });

test('disk-full failures take priority over paths and connection details in errors', () => {
  for (const error of [
    "ENOSPC: no space left on device, open '/workspace/history.jsonl'",
    'cost durability preflight failed: cost persistence is frozen: io error: No space left on device (os error 28)',
    'Session paused because disk space is insufficient (ENOSPC)',
  ]) {
    const result = withDesktopLocale('zh-Hans', () => classifyDesktopError(error));
    assert.equal(result.title, '磁盘空间不足，任务已暂停');
    assert.match(result.detail, /当前会话/);
    assert.match(result.detail, /历史记录会保留/);
    assert.doesNotMatch(result.detail, /clear|新建/);
  }
});

test('storage pause remains until an explicit successful recovery and stays session-scoped', () => {
  const initial = emptyDesktopState();
  let paused = reduceDesktopEvent(initial, { type: 'session_storage_state', disk_full: true });
  paused = reduceDesktopEvent(paused, { type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'error',
    cost: { total_usd: 1, input_tokens: 10, output_tokens: 20, api_calls: 1, session_duration_secs: 1, formatted: '$1' } });
  assert.equal(paused.storagePaused, true);
  assert.equal(initial.storagePaused, false);
  const resumed = reduceDesktopEvent(paused, { type: 'session_storage_state', disk_full: false });
  assert.equal(resumed.storagePaused, false);
  assert.equal(resumed.lastCost, paused.lastCost);
  const other = reduceDesktopEvent(paused, { type: 'session_resumed', session_id: 'other', messages: [] });
  assert.equal(other.storagePaused, false);
  assert.equal(paused.storagePaused, true);
});

test('storage state is validated and included in renderer replay', () => {
  assert.deepEqual(validateClientEvent({ type: 'session_storage_state', disk_full: true }),
    { type: 'session_storage_state', disk_full: true });
  assert.throws(() => validateClientEvent({ type: 'session_storage_state', disk_full: 'true' }));
  assert.throws(() => validateClientEvent({ type: 'session_storage_state' }));
  assert.equal(TRANSCRIPT_REPLAY_EVENTS.has('session_storage_state'), true);
});

test('storage pause presents a persistent actionable notice in both themes', () => {
  for (const dark of [false, true]) {
    const html = withDesktopLocale('zh-Hans', () => renderToStaticMarkup(createElement(Theme.Provider, { value: tokens(dark) },
      createElement(SessionStorageNotice, { disabled: true, onRetry: async () => {} }))));
    assert.match(html, /role="alert"/);
    assert.match(html, /磁盘空间不足，任务已暂停/);
    assert.match(html, /重试并继续/);
    assert.match(html, /待写入的记录/);
    assert.match(html, /disabled=""/);
  }
});
