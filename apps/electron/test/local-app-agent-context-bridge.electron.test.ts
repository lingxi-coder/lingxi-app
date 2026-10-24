import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';

test('Electron 43 transports File bytes through ArrayBuffer and reconstructs picker Files in the page world', async t => {
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
      agent: { tasks: { start: async input => ipcRenderer.invoke('agent-start', input), get: async () => ({}), list: async () => [],
        cancel: async () => {}, subscribe: async () => {}, unsubscribe: async () => {}, onEvent: () => () => {} },
        chats: { create: async () => ({}), open: async () => ({}), get: async () => ({}), list: async () => [], archive: async () => {},
          delete: async () => {}, history: async () => ({ messages: [] }), send: async () => ({}), cancel: async () => {} } }
    });
  `);
  await fs.writeFile(join(appDirectory, 'index.html'), `<!doctype html><html><head></head><body><script>${LOCAL_APP_SDK}</script><script>
    window.localAppTest = (async () => {
      const [file] = await localApp.files.pick({ multiple: false, accept: '.txt' });
      const started = await localApp.agent.tasks.start({ prompt: 'inspect', attachments: [file] });
      return { isFile: file instanceof File, fileBytes: Array.from(new Uint8Array(await file.arrayBuffer())), runId: started.runId };
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
      const attachment = input.attachments?.[0];
      global.__submitted = input;
      global.__fileWasSentDirectly = typeof File !== 'undefined' && attachment instanceof File;
      return { runId: 'electron-run', isArrayBuffer: attachment?.bytes instanceof ArrayBuffer,
        byteValues: attachment?.bytes instanceof ArrayBuffer ? Array.from(new Uint8Array(attachment.bytes)) : [],
        fileWasSentDirectly: global.__fileWasSentDirectly };
    });
    app.whenReady().then(async () => {
      const window = new BrowserWindow({ show: false, webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, sandbox: true } });
      await window.loadFile(path.join(__dirname, 'index.html'));
      try {
        const result = await window.webContents.executeJavaScript('window.localAppTest');
        const submitted = global.__submitted;
        const verified = result.isFile && JSON.stringify(result.fileBytes) === '[4,5,6]' && result.runId === 'electron-run'
          && submitted?.attachments?.[0]?.bytes instanceof ArrayBuffer
          && JSON.stringify(Array.from(new Uint8Array(submitted.attachments[0].bytes))) === '[4,5,6]'
          && !global.__fileWasSentDirectly;
        console.log('RESULT:' + JSON.stringify({ verified, result, submittedUsesArrayBuffer: submitted?.attachments?.[0]?.bytes instanceof ArrayBuffer,
          fileWasSentDirectly: global.__fileWasSentDirectly }));
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
  const exitCode = await new Promise<number>((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron file bridge smoke timed out\n${stdout}\n${stderr}`)); }, 30_000);
    child.once('error', error => { clearTimeout(timeout); reject(error); });
    child.once('close', code => { clearTimeout(timeout); resolve(code ?? -1); });
  });
  assert.equal(exitCode, 0, `${stdout}\n${stderr}`);
  const resultLine = stdout.split('\n').find(line => line.startsWith('RESULT:'));
  assert.ok(resultLine, `${stdout}\n${stderr}`);
  assert.deepEqual(JSON.parse(resultLine.slice('RESULT:'.length)), {
    verified: true,
    result: { isFile: true, fileBytes: [4, 5, 6], runId: 'electron-run' },
    submittedUsesArrayBuffer: true,
    fileWasSentDirectly: false,
  });
});
