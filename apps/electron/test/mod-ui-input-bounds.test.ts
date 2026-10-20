import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { modUiRenderPropsWithinBounds, validateModUiPlainData } from '../src/shared/modUiPlainData';
import { ModUiParentSite, type ModUiClientHostApi } from '../src/renderer/components/modUiAbovePrompt';

(globalThis as { React?: typeof React }).React = React;

test('render props use the exact serialized UTF-16 boundary, including escaping and property keys', () => {
  for (const prefix of ['ascii', '正常🚲', '\ud800', '\udc00', '\ud800\ud800\udc00\udc00', '"\\\n\r\t\b\f\u0000\u001f', '\u2028\u2029']) {
    const value = { [prefix]: prefix, text: '' };
    value.text = 'x'.repeat(100_000 - JSON.stringify(value).length);
    assert.equal(JSON.stringify(value).length, 100_000);
    assert.equal(modUiRenderPropsWithinBounds(value), true);
    assert.equal(modUiRenderPropsWithinBounds({ ...value, text: value.text + 'x' }), false);
  }
  const data = { integers: [0, -0, 1e21, 1e-7, Number.MAX_SAFE_INTEGER], null: null, bool: false, text: '' };
  data.text = 'x'.repeat(100_000 - JSON.stringify(data).length);
  assert.equal(modUiRenderPropsWithinBounds(data), true);
  assert.equal(modUiRenderPropsWithinBounds({ ...data, text: data.text + 'x' }), false);
});

test('render props match Host depth and value limits without rejecting shared JSON subtrees', () => {
  let nested: unknown = null;
  for (let i = 0; i < 32; i++) nested = { nested };
  assert.equal(modUiRenderPropsWithinBounds(nested), true);
  assert.equal(modUiRenderPropsWithinBounds({ nested }), false);
  assert.equal(modUiRenderPropsWithinBounds({ entries: Array(19_998).fill(0) }), true);
  assert.equal(modUiRenderPropsWithinBounds({ entries: Array(19_999).fill(0) }), false);
  const shared = { text: 'keep both occurrences' };
  const value = { a: shared, b: shared };
  assert.doesNotThrow(() => validateModUiPlainData(value, 'test'));
  assert.equal(value.a, value.b);
});

test('oversized and pathological data is rejected without executing user serialization or getters', () => {
  const cyclic: Record<string, unknown> = {}; cyclic.self = cyclic;
  for (const value of [null, [], { value: undefined }, { value: NaN }, { value: Infinity }, { value: 1n }, { value: () => {} }, { value: new Date() }, cyclic, { items: Array(1_000_000) }]) {
    assert.equal(modUiRenderPropsWithinBounds(value), false);
  }
  const accessor = Object.defineProperty({}, 'value', { enumerable: true, get() { throw new Error('must not read'); } });
  assert.equal(modUiRenderPropsWithinBounds(accessor), false);
  const serialized = Object.defineProperty({}, 'toJSON', { value() { throw new Error('must not serialize'); } });
  assert.equal(modUiRenderPropsWithinBounds(serialized), false);
  const original = JSON.stringify;
  JSON.stringify = (() => { throw new Error('must not allocate serialized body'); }) as typeof JSON.stringify;
  try { assert.equal(modUiRenderPropsWithinBounds({ output: 'x'.repeat(1_000_000) }), false); }
  finally { JSON.stringify = original; }
});

test('oversized tool results retain full native UI while valid Mod sites remain mounted', () => {
  const output = '正常🚲'.repeat(40_000);
  const props = Object.freeze({ output });
  const host = {} as ModUiClientHostApi;
  const render = (value: Record<string, string>) => renderToStaticMarkup(React.createElement(ModUiParentSite, {
    sessionId: 'bounds-test', surface: 'desktop', component: 'ToolResult', instanceId: 'large-read',
    props: value, host,
  }, React.createElement('pre', null, value.output)));
  const html = render(props);
  assert.doesNotMatch(html, /mod-ui-parent-site/);
  assert.ok(html.includes(output));
  assert.equal(props.output, output);
  assert.match(render({ output: 'normal output' }), /mod-ui-parent-site/);
});
