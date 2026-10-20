import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../', import.meta.url);
let instance = 0;

async function host(order, initialUsage = { tokens: 23000, window: 1000000, percent: 2.3 }) {
  const hooks = [];
  let usage = initialUsage;
  const $ = {
    session: { usage: async () => ({ context: usage }), cwd: async () => '/fixture' },
    command: { register: async () => {} },
    clock: { sleep: async () => {} },
    fs: { exists: async () => false },
    ui: {
      invalidate() {},
      open: async () => { throw new Error('Pane unavailable'); },
      close() {},
      resolve: () => Object.fromEntries(['Box', 'Text', 'Button'].map(type => [
        type, props => ({ type, props, children: props.children ?? [] }),
      ])),
    },
  };
  const id = instance++;
  for (const name of order) {
    const module = await import(new URL(`${name}/hooks/${name}.mjs?instance=${id}`, root));
    module.register((event, filter, fn) => hooks.push({ event, filter: fn ? filter : {}, fn: fn ?? filter }));
  }
  const dispatch = (event, input = {}, terminal = null) => {
    const matching = hooks.filter(h => h.event === event && Object.entries(h.filter).every(([k, v]) => input[k] === v));
    const next = i => i === matching.length ? Promise.resolve(terminal) : Promise.resolve(matching[i].fn($, input, () => next(i + 1)));
    return next(0);
  };
  return {
    dispatch,
    setUsage(value) { usage = value; },
    render: (base = { type: 'engine', ref: 'desktop:AbovePrompt:test' }) => dispatch('ui.render', {
      component: 'AbovePrompt', surface: 'desktop', bodyColumns: 100,
    }, base),
  };
}

function button(tree, label) {
  if (tree?.type === 'Button' && tree.props.label === label) return tree;
  for (const child of Array.isArray(tree?.children) ? tree.children : []) {
    const found = button(child, label);
    if (found) return found;
  }
}

for (const order of [['replay-theater', 'token-weather'], ['token-weather', 'replay-theater']]) {
  test(`${order.join(' -> ')} retains Weather through Replay open, paging and close`, async () => {
    const h = await host(order);
    await h.dispatch('session.start');
    await h.dispatch('turn.start');
    for (const name of ['a', 'b']) {
      await h.dispatch('tool.call', { tool: 'Write', file_path: `/fixture/${name}`, content: `${name}\n` });
    }
    h.setUsage({ tokens: 31000, window: 1000000, percent: 3.1 });
    await h.dispatch('turn.complete');
    const base = { type: 'Text', children: ['OTHER_MOD'] };
    const render = () => h.render(base);
    const assertShared = tree => {
      assert.match(JSON.stringify(tree), /Clear/);
      assert.match(JSON.stringify(tree), /31k/);
      assert.match(JSON.stringify(tree), /OTHER_MOD/);
    };
    let tree = await render();
    assertShared(tree);
    assert.match(JSON.stringify(tree), /Replay: 2 edits/);
    await button(tree, 'Replay').props.onPress();
    tree = await render();
    assertShared(tree);
    assert.match(JSON.stringify(tree), /step 1 of 2/);
    button(tree, 'Next ▶').props.onPress();
    tree = await render();
    assertShared(tree);
    assert.match(JSON.stringify(tree), /step 2 of 2/);
    button(tree, '◀ Prev').props.onPress();
    tree = await render();
    assert.match(JSON.stringify(tree), /step 1 of 2/);
    button(tree, 'Close').props.onPress();
    tree = await render();
    assertShared(tree);
    assert.match(JSON.stringify(tree), /Replay: 2 edits/);
    assert.doesNotMatch(JSON.stringify(tree), /Replay Theater|step 1 of 2/);
    assert.equal(button(tree, 'Close'), undefined);
    // A read-only turn keeps the last editing turn available for reopening.
    await h.dispatch('turn.start');
    await h.dispatch('turn.complete');
    assert.match(JSON.stringify(await render()), /Replay: 2 edits/);
  });

  test(`${order.join(' -> ')} populates an empty draw without embedding the Host marker`, async () => {
    const h = await host(order, null);
    await h.dispatch('session.start');
    let tree = await h.render();
    assert.equal(tree.type, 'Box');
    assert.doesNotMatch(JSON.stringify(tree), /desktop:AbovePrompt/);
    h.setUsage({ tokens: 52000, window: 200000, percent: 26 });
    await h.dispatch('turn.start');
    await h.dispatch('tool.call', { tool: 'Write', file_path: '/fixture/c', content: 'c\n' });
    await h.dispatch('turn.complete');
    tree = await h.render();
    assert.match(JSON.stringify(tree), /Cloudy/);
    assert.match(JSON.stringify(tree), /52k/);
    assert.match(JSON.stringify(tree), /Replay: 1 edit/);
    assert.doesNotMatch(JSON.stringify(tree), /desktop:AbovePrompt/);
  });
}

test('marketplace install paths, versions and module bytes match the maintained release', async () => {
  const marketplace = JSON.parse(await readFile(new URL('.lingxi-plugin/marketplace.json', root)));
  const hashes = JSON.parse(await readFile(new URL('module-hashes.json', root)));
  const provenance = JSON.parse(await readFile(new URL('provenance.json', root)));
  assert.equal(marketplace.name, 'lingxi-desktop-mods');
  assert.equal(marketplace.plugins.length, 2);
  for (const entry of marketplace.plugins) {
    const path = new URL(`${entry.source}/`, root);
    const moduleName = entry.name.replace(/^lingxi-/, '');
    const manifest = JSON.parse(await readFile(new URL('.lingxi-plugin/plugin.json', path)));
    assert.equal(manifest.name, entry.name);
    assert.equal(manifest.version, '0.1.1');
    assert.equal(entry.version, manifest.version);
    const hooks = JSON.parse(await readFile(new URL('hooks/hooks.json', path)));
    assert.deepEqual(hooks.modules, [`./${moduleName}.mjs`]);
    const source = await readFile(new URL(`hooks/${moduleName}.mjs`, path));
    assert.equal(createHash('sha256').update(source).digest('hex'), hashes[moduleName]);
    assert.match(source.toString(), /^\/\/ Modified by LingXi/);
    const acceptedBody = source.subarray(source.indexOf(10) + 1);
    assert.equal(createHash('sha256').update(acceptedBody).digest('hex'), provenance.gui_accepted_module_sha256[moduleName]);
  }
});
