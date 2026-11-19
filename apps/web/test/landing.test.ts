import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { createElement } from 'react';
import { HomePage } from '../src/pages/PublicPages';

const renderer = createRequire(import.meta.url)('react-test-renderer');
const textOf = (node: any): string => typeof node === 'string' ? node : (node?.children ?? []).map(textOf).join('');

for (const [locale, chat, app] of [['zh', '开始对话', '下载 App'], ['en', 'Start chatting', 'Get the app']] as const) {
  test(`the landing page leads with two entry cards (${locale})`, () => {
    let tree: any;
    renderer.act(() => { tree = renderer.create(createElement(HomePage, { locale, region: 'global' })); });
    const cards = tree.root.findAll((node: any) => node.type === 'a' && String(node.props.className ?? '').split(' ').includes('landing-card'));
    assert.equal(cards.length, 2);
    assert.equal(cards[0].props.href, '/chat', 'Chat is a separate document, reached by a full load');
    assert.ok(textOf(cards[0]).includes(chat));
    assert.equal(cards[1].props.href, '/download');
    assert.ok(textOf(cards[1]).includes(app));
  });
}
