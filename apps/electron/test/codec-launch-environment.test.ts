import assert from 'node:assert/strict';
import { once } from 'node:events';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge';

// This verifies the actual Bridge spawn environment and its downstream child;
// it does not claim the fake executable exercises a GStreamer package.
test('Bridge keeps workspace GST settings for tools while selecting a host codec directory',
  { skip: process.platform === 'win32' }, async (t) => {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-codec-launch-test-'));
    const resources = join(root, 'resources');
    const bin = join(resources, 'bin', 'bridge-server');
    mkdirSync(join(resources, 'bin'), { recursive: true });
    writeFileSync(bin, `#!/usr/bin/env node
const {spawnSync} = require('node:child_process');
const keys = ['GST_PLUGIN_PATH', 'GST_REGISTRY_1_0', 'GST_TRACERS', 'LINGXI_GSTREAMER_PLUGIN_DIR', 'lingxi_gstreamer_plugin_dir'];
const select = () => Object.fromEntries(keys.map(key => [key, process.env[key] ?? null]));
const source = '(' + select.toString() + ')()';
const child = spawnSync(process.execPath, ['-e', 'const keys=' + JSON.stringify(keys) + ';process.stdout.write(JSON.stringify(' + source + '))'], {encoding:'utf8'});
if (child.status !== 0) process.exit(3);
process.stdout.write(JSON.stringify({bridge:select(), tool:JSON.parse(child.stdout)}));
`);
    chmodSync(bin, 0o700);
    const saved = process.env.LINGXI_GSTREAMER_PLUGIN_DIR;
    process.env.LINGXI_GSTREAMER_PLUGIN_DIR = '/host/sdk/plugins';
    const runtimes: SessionRuntime[] = [];
    t.after(async () => {
      if (saved === undefined) delete process.env.LINGXI_GSTREAMER_PLUGIN_DIR;
      else process.env.LINGXI_GSTREAMER_PLUGIN_DIR = saved;
      for (const runtime of runtimes) await runtime.dispose();
      rmSync(root, { recursive: true, force: true });
    });
    const settingsEnvironment = {
      GST_PLUGIN_PATH: '/workspace/plugins', GST_REGISTRY_1_0: '/workspace/registry',
      GST_TRACERS: 'workspace-tracer', LINGXI_GSTREAMER_PLUGIN_DIR: '/workspace/override',
      lingxi_gstreamer_plugin_dir: '/workspace/alias',
    };
    for (const isPackaged of [false, true]) {
      const launch = { workspace: root, trusted: true, settingsEnvironment };
      const runtime = new SessionRuntime({
        serverBin: bin, isPackaged, resourcesPath: resources,
        sessionId: '11111111-2222-4333-8444-555555555555', projectPath: root,
        launchConfig: () => launch,
      });
      runtimes.push(runtime);
      const child = (runtime as any).spawnServer(launch, join(root, 'bridge'));
      let output = '', errors = '';
      child.stdout.on('data', (chunk: Buffer) => { output += chunk.toString(); });
      child.stderr.on('data', (chunk: Buffer) => { errors += chunk.toString(); });
      const [code] = await once(child, 'close');
      assert.equal(code, 0, errors);
      const observed = JSON.parse(output);
      const selected = !isPackaged ? '/host/sdk/plugins' : process.platform === 'darwin'
        ? resolve(resources, '..', 'Frameworks', 'GStreamerPlugins')
        : resolve(resources, 'gstreamer', 'lib', 'gstreamer-1.0');
      const expected = {
        GST_PLUGIN_PATH: '/workspace/plugins', GST_REGISTRY_1_0: '/workspace/registry',
        GST_TRACERS: 'workspace-tracer', LINGXI_GSTREAMER_PLUGIN_DIR: selected,
        lingxi_gstreamer_plugin_dir: null,
      };
      assert.deepEqual(observed.bridge, expected);
      assert.deepEqual(observed.tool, expected);
    }
  });
