import assert from 'node:assert/strict';
import test, { before } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ComputerAccessRequestDto, ComputerAccessResponseDto } from '@lingxi/bridge-client';
import { assertComputerAccessSubset } from '../src/main/computerAccessApproval';
import { validateComputerAccessResponse } from '../src/main/validation';
import { ComputerAccessPrompt } from '../src/renderer/components/ComputerAccessPrompt';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, loadLocaleCatalog, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
before(async () => { await Promise.all(DESKTOP_LOCALES.map(loadLocaleCatalog)); });
const request: ComputerAccessRequestDto = {
  request_id: 9, reason: 'Explain the requested action', apps: [{ label: 'Notes' }],
  tier: 'full', clipboard_read: true, clipboard_write: false, system_key_combos: false,
};
const denied: ComputerAccessResponseDto = { granted_apps: [], clipboard_read: false, clipboard_write: false, system_key_combos: false };

test('trusted interaction approval permits only actions belonging to the pending kind', () => {
  for (const kind of ['takeover', 'teach_access', 'teach_step'] as const) {
    const pending = { ...request, interaction: { kind } };
    assert.doesNotThrow(() => assertComputerAccessSubset(pending, denied));
    for (const action of ['allow', 'continue', 'exit'] as const) {
      const response = { ...denied, interaction_action: action };
      const valid = kind === 'teach_step' ? action !== 'allow' : action === 'allow';
      if (valid) assert.doesNotThrow(() => assertComputerAccessSubset(pending, response));
      else assert.throws(() => assertComputerAccessSubset(pending, response), /exceeds/);
    }
    const appGrant = { ...denied, granted_apps: ['Notes'], interaction_action: 'allow' as const };
    if (kind === 'teach_access') assert.doesNotThrow(() => assertComputerAccessSubset(pending, appGrant));
    else assert.throws(() => assertComputerAccessSubset(pending, appGrant), /exceeds/);
    assert.throws(() => assertComputerAccessSubset(pending, { ...denied, granted_apps: ['Notes'] }), /exceeds/);
    assert.throws(() => assertComputerAccessSubset(pending, { ...denied, clipboard_read: true }), /exceeds/);
  }
  assert.throws(() => assertComputerAccessSubset(request, { ...denied, interaction_action: 'allow' }), /exceeds/);
  assert.throws(() => assertComputerAccessSubset({ ...request, tcc_state: { accessibility: false, screen_recording: true }, interaction: { kind: 'takeover' } }, { ...denied, interaction_action: 'allow' }), /exceeds/);
});

test('IPC validation retains a bounded interaction choice and rejects fabricated values', () => {
  for (const action of ['allow', 'continue', 'exit'] as const) {
    assert.deepEqual(validateComputerAccessResponse({ ...denied, interaction_action: action }), { ...denied, interaction_action: action });
  }
  assert.deepEqual(validateComputerAccessResponse(denied), denied);
  for (const action of ['', 'approve_all', null, true, {}, ['allow']]) {
    assert.throws(() => validateComputerAccessResponse({ ...denied, interaction_action: action }), /invalid/);
  }
});

test('interaction preference is explicit and cannot be smuggled into control-only or denied approvals', () => {
  for (const mode of ['background', 'full_control'] as const) {
    const response = { ...denied, preferred_mode: mode };
    assert.deepEqual(validateComputerAccessResponse(response), response);
    assert.doesNotThrow(() => assertComputerAccessSubset(request, response));
    assert.doesNotThrow(() => assertComputerAccessSubset({ ...request, interaction: { kind: 'teach_access' } }, { ...response, interaction_action: 'allow' }));
    assert.throws(() => assertComputerAccessSubset({ ...request, interaction: { kind: 'teach_access' } }, response), /exceeds/);
    assert.throws(() => assertComputerAccessSubset({ ...request, interaction: { kind: 'takeover' } }, { ...response, interaction_action: 'allow' }), /exceeds/);
    assert.throws(() => assertComputerAccessSubset({ ...request, interaction: { kind: 'teach_step' } }, { ...response, interaction_action: 'continue' }), /exceeds/);
    assert.throws(() => assertComputerAccessSubset({ ...request, tcc_state: { accessibility: false, screen_recording: true } }, response), /exceeds/);
  }
  for (const mode of ['', 'takeover', null, true, {}, ['background']]) {
    assert.throws(() => validateComputerAccessResponse({ ...denied, preferred_mode: mode }), /invalid/);
  }
});

function renderInLocale(locale: DesktopLocale, pending: ComputerAccessRequestDto): string {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null } });
  try {
    return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
      React.createElement(ComputerAccessPrompt, { request: pending, onSubmit: () => {}, onDeny: () => {}, onOpenSystemSettings: () => {} })));
  } finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

test('takeover and teaching confirmations expose localized choices and inert explanatory text', () => {
  for (const locale of DESKTOP_LOCALES) for (const kind of ['takeover', 'teach_step', 'teach_access'] as const) {
    const html = renderInLocale(locale, { ...request, interaction: { kind,
      explanation: '<script>untrusted()</script>', next_preview: 'Review the next step', anchor_json: '[20,30]' } });
    assert.ok(html.includes(translate(kind === 'takeover' ? 'desktop_computer_takeover_title' : kind === 'teach_step' ? 'desktop_computer_teach_step_title' : 'desktop_computer_teach_access_title', locale)));
    assert.ok(html.includes('&lt;script&gt;untrusted()&lt;/script&gt;'));
    assert.ok(html.includes('Review the next step'));
    assert.doesNotMatch(html, /<script>|desktop_computer_|\[20,30\]/);
    if (kind !== 'teach_access') {
      assert.match(html, /<dialog /);
      assert.match(html, /<form method="dialog"/);
      assert.doesNotMatch(html, /role="dialog"|aria-modal|<input/);
      assert.ok(html.includes(`value="${kind === 'takeover' ? 'allow' : 'continue'}"`));
      if (kind === 'teach_step') assert.match(html, /value="exit"/);
    } else {
      assert.ok(html.includes('Notes'));
      assert.ok(html.includes(translate('desktop_computer_teach_allow', locale)));
    }
  }
});
