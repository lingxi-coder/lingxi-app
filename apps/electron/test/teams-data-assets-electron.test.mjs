import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));

test('Teams data assets load from a production split main chunk through the real sandboxed preload', { timeout: 60000 }, async t => {
  if (process.platform === 'linux' && !process.env.DISPLAY) { t.skip('Electron requires a display'); return; }
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-teams-assets-'));
  let child;
  try {
    await Promise.all([
      build({ entryPoints: { index: join(root, 'test/fixtures/teams-data-assets-main.ts') }, outdir: join(temp, 'out/main'),
        chunkNames: 'chunks/[name]-[hash]', splitting: true, bundle: true, platform: 'node', format: 'esm', packages: 'external',
        banner: { js: "import { fileURLToPath as moduleFile } from 'node:url'; import { dirname as moduleDirectory } from 'node:path'; const __dirname = moduleDirectory(moduleFile(import.meta.url));" }, logLevel: 'error' }),
      build({ entryPoints: [join(root, 'src/preload/teamsData.ts')], outfile: join(temp, 'out/preload/teamsData.cjs'),
        bundle: true, platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'error' }),
      build({ entryPoints: [join(root, 'src/renderer/teams-data.ts')], outfile: join(temp, 'out/renderer/teams-data.js'),
        bundle: true, platform: 'browser', format: 'esm', logLevel: 'error' }),
    ]);
    const html = await readFile(join(root, 'src/renderer/teams-data.html'), 'utf8');
    await writeFile(join(temp, 'out/renderer/teams-data.html'), html.replace('./teams-data.ts', './teams-data.js'));
    await writeFile(join(temp, 'package.json'), JSON.stringify({ type: 'module', main: 'out/main/index.js' }));
    const env = { ...process.env }; delete env.ELECTRON_RUN_AS_NODE; delete env.ELECTRON_RENDERER_URL;
    const electron = createRequire(import.meta.url)('electron');
    let output = '', errors = '';
    child = spawn(electron, [temp], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    const code = await new Promise((resolveExit, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(new Error(`Teams asset fixture timed out\n${output}\n${errors}`)); }, 40000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', value => { clearTimeout(timer); resolveExit(value); });
    });
    assert.equal(code, 0, `${output}\n${errors}`);
    assert.ok(output.includes('"passed":true'), output);
  } finally {
    if (child && child.exitCode === null) child.kill();
    await rm(temp, { recursive: true, force: true });
  }
});
