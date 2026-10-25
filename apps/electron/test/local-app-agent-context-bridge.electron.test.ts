import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';

test('Electron 43 serializes browser File as an official inline AG-UI DataSource before IPC', async t => {
  if (process.platform !== 'darwin') { t.skip('the isolated Electron 43 smoke runs on macOS'); return; }
  const testDirectory = dirname(fileURLToPath(import.meta.url));
  const electronBinary = join(testDirectory, '../node_modules/electron/dist/Electron.app/Contents/MacOS/Electron');
  try { await fs.access(electronBinary); } catch { t.skip('Electron dev binary is unavailable'); return; }
  const appDirectory = await fs.mkdtemp(join(tmpdir(), 'lingxi-electron-file-bridge-'));
  t.after(() => fs.rm(appDirectory, { recursive: true, force: true }));
  await fs.writeFile(join(appDirectory, 'package.json'), JSON.stringify({ name: 'local-app-file-bridge-smoke', version: '1.0.0', main: 'main.cjs' }));
  await fs.writeFile(join(appDirectory, 'preload.cjs'), `
    const { contextBridge, ipcRenderer } = require('electron');
    contextBridge.exposeInMainWorld('__lingxiLocalAppBridge', {
      info: async () => ({}), clipboard: { read: async () => '', write: async () => {} }, notify: async () => {},
      process: { exec: async () => ({}) }, serviceSocketUrl: async () => '',
      files: { pick: async options => ipcRenderer.invoke('files-pick', options), pickDirectory: async () => null,
        read: async () => '', write: async () => {}, list: async () => [] },
    agent: { tasks: { start: async input => ipcRenderer.invoke('agent-start', input), get: async () => ({}), list: async () => ({ tasks: [] }),
        events: async () => ({ events: [], sequence: 0 }), cancel: async () => {} },
        chats: { create: async () => ({}), open: async () => ({}), get: async () => ({}), list: async () => [], archive: async () => {},
          delete: async () => {}, history: async () => ({ messages: [] }), send: async () => ({}), cancel: async () => {} } }
    });
  `);
  await fs.writeFile(join(appDirectory, 'index.html'), `<!doctype html><html><head></head><body><script>${LOCAL_APP_SDK}</script><script>
    window.localAppTest = (async () => {
      const [file] = await localApp.files.pick({ multiple: false, accept: '.txt' });
      const part = await localApp.agent.file(file);
      part.id = 'file-part-one';
      const started = await localApp.agent.tasks.start({ threadId: 'electron-thread', runId: 'electron-run',
        messages: [{ id: 'user-one', role: 'user', content: [part] }] });
      return { isFile: file instanceof File, fileBytes: Array.from(new Uint8Array(await file.arrayBuffer())),
        contentType: part.type, sourceType: part.source.type, runId: started.runId };
    })();
  </script></body></html>`);
  await fs.writeFile(join(appDirectory, 'main.cjs'), `
    const { app, BrowserWindow, ipcMain } = require('electron');
    const path = require('node:path');
    app.setPath('userData', path.join(__dirname, 'profile'));
    app.setPath('sessionData', path.join(__dirname, 'session'));
    app.commandLine.appendSwitch('disable-gpu');
    ipcMain.handle('files-pick', () => ({ files: [{ name: 'picked.txt', mediaType: 'text/plain', size: 3, lastModified: 7,
      bytes: new Uint8Array([4, 5, 6]).buffer }] }));
    ipcMain.handle('agent-start', (_event, input) => {
      const part = input.messages?.[0]?.content?.[0];
      global.__submitted = input;
      global.__fileWasSentDirectly = typeof File !== 'undefined' && part instanceof File;
      return { runId: input.runId, isArrayBuffer: part?.source?.value instanceof ArrayBuffer,
        dataSource: part?.source, partType: part?.type, fileWasSentDirectly: global.__fileWasSentDirectly };
    });
    app.whenReady().then(async () => {
      const window = new BrowserWindow({ show: false, webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, sandbox: true } });
      await window.loadFile(path.join(__dirname, 'index.html'));
      try {
        const result = await window.webContents.executeJavaScript('window.localAppTest');
        const submitted = global.__submitted;
        const part = submitted?.messages?.[0]?.content?.[0];
        const verified = result.isFile && JSON.stringify(result.fileBytes) === '[4,5,6]' && result.runId === 'electron-run'
          && result.contentType === 'document' && result.sourceType === 'data'
          && submitted?.protocolVersion === '1.0'
          && part?.source?.value === 'BAUG' && part?.source?.mimeType === 'text/plain'
          && part?.metadata?.['com.lingxi.localApp']?.filename === 'picked.txt'
          && part?.source?.value instanceof ArrayBuffer === false
          && !global.__fileWasSentDirectly;
        console.log('RESULT:' + JSON.stringify({ verified, result, contentPart: part,
          sourceValueWasArrayBuffer: part?.source?.value instanceof ArrayBuffer, fileWasSentDirectly: global.__fileWasSentDirectly }));
        app.exit(verified ? 0 : 1);
      } catch (error) { console.error('SMOKE_ERROR:' + (error?.stack || error)); app.exit(1); }
    });
  `);
  const child = spawn(electronBinary, [appDirectory], {
    cwd: appDirectory,
    env: { ...process.env, NODE_PATH: join(testDirectory, '../node_modules') },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '', stderr = '';
  child.stdout.setEncoding('utf8').on('data', chunk => { stdout += chunk; });
  child.stderr.setEncoding('utf8').on('data', chunk => { stderr += chunk; });
  let exitSignal: NodeJS.Signals | null = null;
  const exitCode = await new Promise<number>((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron file bridge smoke timed out\n${stdout}\n${stderr}`)); }, 30_000);
    child.once('error', error => { clearTimeout(timeout); reject(error); });
    child.once('close', (code, signal) => { clearTimeout(timeout); exitSignal = signal; resolve(code ?? -1); });
  });
  assert.equal(exitCode, 0, `signal=${exitSignal}\n${stdout}\n${stderr}`);
  const resultLine = stdout.split('\n').find(line => line.startsWith('RESULT:'));
  assert.ok(resultLine, `${stdout}\n${stderr}`);
  assert.deepEqual(JSON.parse(resultLine.slice('RESULT:'.length)), {
    verified: true,
    result: { isFile: true, fileBytes: [4, 5, 6], contentType: 'document', sourceType: 'data', runId: 'electron-run' },
    contentPart: { type: 'document', id: 'file-part-one', source: { type: 'data', value: 'BAUG', mimeType: 'text/plain' },
      metadata: { 'com.lingxi.localApp': { version: 1, filename: 'picked.txt' } } },
    sourceValueWasArrayBuffer: false,
    fileWasSentDirectly: false,
  });
});
