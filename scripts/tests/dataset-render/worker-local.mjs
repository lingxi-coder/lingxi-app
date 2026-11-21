import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { createInterface } from 'node:readline';

const chromium = process.argv[2];
assert.ok(chromium && isAbsolute(chromium), 'Provide the absolute path to a local Chromium executable');
const tmp = await mkdtemp(join(tmpdir(), 'lingxi-dataset-worker-'));
try {
  const source = await readFile(new URL('../../../apps/dataset-renderer/worker.mjs', import.meta.url), 'utf8');
  const workerPath = join(tmp, 'worker.mjs');
  await writeFile(workerPath, source.replace("'/usr/bin/chromium'", JSON.stringify(chromium))
    .replace("'--user-data-dir=/tmp/viewer'", JSON.stringify('--user-data-dir=' + join(tmp, 'profile'))));
  const worker = spawn(process.execPath, [workerPath], { stdio: ['pipe', 'pipe', 'inherit'] });
  const exit = once(worker, 'exit');
  const responses = createInterface({ input: worker.stdout })[Symbol.asyncIterator]();
  const timeout = setTimeout(() => worker.kill('SIGKILL'), 30_000);

  async function send(command) {
    worker.stdin.write(JSON.stringify(command) + '\n');
    const response = await responses.next();
    assert.equal(response.done, false, 'Chromium worker ended early');
    const frame = JSON.parse(response.value);
    assert.equal(frame.error, undefined, `Worker rejected ${command.event?.kind ?? command.action}`);
    assert.equal(Buffer.from(frame.png, 'base64').readUInt32BE(0), 0x89504e47);
    return frame;
  }

  try {
    const html = `<style>#result { position: fixed; top: 135px; left: 20px; font: 24px sans-serif; margin: 0 }</style>
      <label>订单 <input id="memo"></label>
      <button id="save">保存</button><p id="result"></p>
      <script>
        document.getElementById('save').addEventListener('click', async () => {
          const memo = document.getElementById('memo').value;
          await lingxi.saveState({ memo });
          document.getElementById('result').textContent = '已保存 ' + memo;
        });
      </script>`;
    const frame = await send({ action: 'mount', dimensions: { width: 620, height: 480 },
      state: null, stateVersion: 0, bundle: { html, assets: {}, actions: {} } });
    const field = frame.accessibility.find(item => item.role === 'textbox');
    const button = frame.accessibility.find(item => item.role === 'button' && item.name === '保存');
    assert.ok(field?.target && button?.target);
    const input = event => send({ action: 'input', event });
    await input({ kind: 'focusNode', node: field.target });
    await input({ kind: 'composition', text: '定单' });
    await input({ kind: 'text', text: '订单' });
    await input({ kind: 'focusNode', node: button.target });
    const activated = await input({ kind: 'key', key: 'Enter' });
    const pending = activated.actions[0] ?? (await send({ action: 'frame' })).actions[0];
    assert.ok(pending, 'Keyboard activation must invoke the button');
    assert.deepEqual(JSON.parse(pending.state), { memo: '订单' });
    const rendered = await send({ action: 'result', id: pending.id, result: { version: 1 } });
    assert.ok(rendered.accessibility.some(item => item.name === '已保存 订单'));
    assert.equal(rendered.selectedText, undefined);
    await input({ kind: 'select', start: { x: 21, y: 149 }, end: { x: 147, y: 149 } });
    const copied = await input({ kind: 'selection' });
    assert.equal(copied.selectedText, '已保存 订单');
    console.log('PASS Chromium accessibility focus, Chinese IME commit, keyboard activation, reader state and text selection');
  } finally {
    worker.stdin.end();
    await exit;
    clearTimeout(timeout);
  }
} finally {
  await rm(tmp, { recursive: true, force: true });
}
