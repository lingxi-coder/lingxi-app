import assert from 'node:assert/strict';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  bundlePortableGStreamerRuntime,
  PORTABLE_GSTREAMER_REQUIRED_PLUGINS,
  preflightPortableGStreamerRuntime,
  verifyPortableGStreamerRuntime,
} from '../scripts/gstreamer-portable.mjs';

const LINUX_RUNTIME_LIBRARIES = [
  'libgstreamer-1.0.so.0',
  'libgstapp-1.0.so.0',
  'libgstbase-1.0.so.0',
  'libgstaudio-1.0.so.0',
  'libgstvideo-1.0.so.0',
  'libglib-2.0.so.0',
  'libgobject-2.0.so.0',
  'libgio-2.0.so.0',
  'libopus.so.0',
  'libvpx.so.9',
];

const WINDOWS_RUNTIME_LIBRARIES = [
  'gstreamer-1.0-0.dll',
  'gstapp-1.0-0.dll',
  'gstbase-1.0-0.dll',
  'gstaudio-1.0-0.dll',
  'gstvideo-1.0-0.dll',
  'glib-2.0-0.dll',
  'gobject-2.0-0.dll',
  'gio-2.0-0.dll',
  'libopus-0.dll',
  'libvpx-9.dll',
];

function fakeElfX64() {
  return fakeElf(0x3e);
}

function fakeElfArm64() {
  return fakeElf(0xb7);
}

function fakeElf(machine) {
  const bytes = Buffer.alloc(64);
  bytes.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
  bytes.writeUInt16LE(machine, 18);
  return bytes;
}

function fakeElfX86() {
  return fakeElf(0x03);
}

function fakePeX64() {
  return fakePe(0x8664);
}

function fakePeArm64() {
  return fakePe(0xaa64);
}

function fakePe(machine) {
  const bytes = Buffer.alloc(256);
  bytes.write('MZ', 0, 'ascii');
  bytes.writeUInt32LE(128, 0x3c);
  bytes.write('PE\0\0', 128, 'ascii');
  bytes.writeUInt16LE(machine, 132);
  return bytes;
}

function fakePeX86() {
  return fakePe(0x014c);
}

function linuxSdk(root, arch = 'x64') {
  const lib = arch === 'arm64' ? join(root, 'lib', 'aarch64-linux-gnu') : join(root, 'lib');
  const plugins = join(lib, 'gstreamer-1.0');
  mkdirSync(plugins, { recursive: true });
  const binary = arch === 'arm64' ? fakeElfArm64 : fakeElfX64;
  for (const name of LINUX_RUNTIME_LIBRARIES) writeFileSync(join(lib, name), binary());
  for (const name of PORTABLE_GSTREAMER_REQUIRED_PLUGINS) {
    writeFileSync(join(plugins, `libgst${name}.so`), binary());
  }
  writeFileSync(join(plugins, 'libgstdebugutilsbad.so'), binary());
  const notices = join(root, 'share', 'licenses', 'gstreamer');
  mkdirSync(notices, { recursive: true });
  writeFileSync(join(notices, 'COPYING'), 'GStreamer test license\n');
  const packageDocs = join(root, 'share', 'doc', 'gstreamer1.0-plugins-base');
  mkdirSync(packageDocs, { recursive: true });
  writeFileSync(join(packageDocs, 'copyright'), 'GStreamer plugin package copyright\n');
  return { lib, plugins };
}

function windowsSdk(root, arch = 'x64') {
  const bin = join(root, 'bin');
  const plugins = join(root, 'lib', 'gstreamer-1.0');
  mkdirSync(bin, { recursive: true });
  mkdirSync(plugins, { recursive: true });
  const binary = arch === 'arm64' ? fakePeArm64 : fakePeX64;
  for (const name of WINDOWS_RUNTIME_LIBRARIES) writeFileSync(join(bin, name), binary());
  for (const name of PORTABLE_GSTREAMER_REQUIRED_PLUGINS) {
    writeFileSync(join(plugins, `gst${name}.dll`), binary());
  }
  return { bin, plugins };
}

test('portable Linux GStreamer bundle stages bounded runtime and verifies its manifest', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-linux-'));
  try {
    const sdk = join(root, 'sdk');
    mkdirSync(sdk);
    linuxSdk(sdk);
    const sidecar = join(root, 'bridge-server');
    writeFileSync(sidecar, fakeElfX64());
    const plan = preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
      LINGXI_GSTREAMER_SDK_LINUX: sdk,
    });
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const result = bundlePortableGStreamerRuntime(plan, resources);

    assert.equal(result.runtimeDirectory, join(resources, 'gstreamer', 'lib'));
    assert.equal(result.pluginDirectory, join(resources, 'gstreamer', 'lib', 'gstreamer-1.0'));
    assert.equal(result.runtimeFiles, LINUX_RUNTIME_LIBRARIES.length + PORTABLE_GSTREAMER_REQUIRED_PLUGINS.length);
    assert.equal(existsSync(join(result.pluginDirectory, 'libgstopus.so')), true);
    assert.equal(existsSync(join(result.pluginDirectory, 'libgstdebugutilsbad.so')), false);
    assert.equal(existsSync(join(resources, 'gstreamer', 'licenses', 'share', 'licenses', 'gstreamer', 'COPYING')), true);
    assert.equal(existsSync(join(resources, 'gstreamer', 'licenses', 'share', 'doc', 'gstreamer1.0-plugins-base', 'copyright')), true);
    const manifest = JSON.parse(readFileSync(join(resources, 'gstreamer', 'runtime-manifest.json'), 'utf8'));
    assert.equal(manifest.architecture, 'x64');
    assert.equal(manifest.platform, 'linux');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable verifier rejects unmanifested plugins in the runtime tree', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-extra-plugin-'));
  try {
    const sdk = join(root, 'sdk');
    mkdirSync(sdk);
    linuxSdk(sdk);
    const sidecar = join(root, 'bridge-server');
    writeFileSync(sidecar, fakeElfX64());
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const plan = preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
      LINGXI_GSTREAMER_SDK_LINUX: sdk,
    });
    const result = bundlePortableGStreamerRuntime(plan, resources);
    writeFileSync(join(result.pluginDirectory, 'libgstdebugutilsbad.so'), fakeElfX64());
    assert.throws(() => verifyPortableGStreamerRuntime(resources, 'linux', 'x64'), /unmanifested/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable Linux GStreamer bundle validates and packages arm64 multiarch SDKs', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-linux-arm64-'));
  try {
    const sdk = join(root, 'sdk');
    mkdirSync(sdk);
    linuxSdk(sdk, 'arm64');
    const sidecar = join(root, 'bridge-server');
    writeFileSync(sidecar, fakeElfArm64());
    const plan = preflightPortableGStreamerRuntime(sidecar, 'linux', 'arm64', {
      LINGXI_GSTREAMER_SDK_LINUX: sdk,
    });
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const result = bundlePortableGStreamerRuntime(plan, resources);
    assert.equal(result.runtimeDirectory, join(resources, 'gstreamer', 'lib'));
    assert.equal(JSON.parse(readFileSync(result.manifestPath, 'utf8')).architecture, 'arm64');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable Windows GStreamer bundle uses the official bin/lib SDK layout', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-win32-'));
  try {
    const sdk = join(root, 'msvc_x86_64');
    mkdirSync(sdk);
    windowsSdk(sdk);
    const sidecar = join(root, 'bridge-server.exe');
    writeFileSync(sidecar, fakePeX64());
    const plan = preflightPortableGStreamerRuntime(sidecar, 'win32', 'x64', {
      LINGXI_GSTREAMER_SDK_WINDOWS: sdk,
    });
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const result = bundlePortableGStreamerRuntime(plan, resources);

    assert.equal(result.runtimeDirectory, join(resources, 'gstreamer', 'bin'));
    assert.equal(result.pluginDirectory, join(resources, 'gstreamer', 'lib', 'gstreamer-1.0'));
    assert.equal(existsSync(join(result.runtimeDirectory, 'gstreamer-1.0-0.dll')), true);
    assert.equal(existsSync(join(result.pluginDirectory, 'gstvideoconvertscale.dll')), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable Windows GStreamer bundle validates arm64 PE runtime files', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-win32-arm64-'));
  try {
    const sdk = join(root, 'msvc_arm64');
    mkdirSync(sdk);
    windowsSdk(sdk, 'arm64');
    const sidecar = join(root, 'bridge-server.exe');
    writeFileSync(sidecar, fakePeArm64());
    const plan = preflightPortableGStreamerRuntime(sidecar, 'win32', 'arm64', {
      LINGXI_GSTREAMER_SDK_WINDOWS: sdk,
    });
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const result = bundlePortableGStreamerRuntime(plan, resources);
    assert.equal(result.runtimeDirectory, join(resources, 'gstreamer', 'bin'));
    assert.equal(JSON.parse(readFileSync(result.manifestPath, 'utf8')).architecture, 'arm64');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable preflight fails without an explicit matching SDK and leaves previous output alone', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-preflight-'));
  try {
    const sidecar = join(root, 'bridge-server');
    writeFileSync(sidecar, fakeElfX64());
    const oldArtifact = join(root, 'LingXi-Code-linux-x64.tar.gz');
    writeFileSync(oldArtifact, 'previous artifact');
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {}),
      /set LINGXI_GSTREAMER_SDK_LINUX/,
    );
    assert.equal(readFileSync(oldArtifact, 'utf8'), 'previous artifact');
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
        LINGXI_GSTREAMER_SDK_LINUX: join(root, 'missing-sdk'),
      }),
      /does not exist/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable preflight rejects missing plugin files and wrong SDK architectures', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-invalid-'));
  try {
    const sdk = join(root, 'sdk');
    const { plugins } = linuxSdk(sdk);
    const sidecar = join(root, 'bridge-server');
    writeFileSync(sidecar, fakeElfX64());
    rmSync(join(plugins, 'libgstvpx.so'));
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
        LINGXI_GSTREAMER_SDK_LINUX: sdk,
      }),
      /missing required plugin libgstvpx\.so/,
    );

    writeFileSync(join(plugins, 'libgstvpx.so'), fakeElfX86());
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
        LINGXI_GSTREAMER_SDK_LINUX: sdk,
      }),
      /Linux x86_64 ELF/,
    );
    writeFileSync(join(plugins, 'libgstvpx.so'), fakeElfArm64());
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'linux', 'x64', {
        LINGXI_GSTREAMER_SDK_LINUX: sdk,
      }),
      /Linux x86_64 ELF/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('portable package verification catches modified runtime files and invalid PE architecture', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-integrity-'));
  try {
    const sdk = join(root, 'msvc_x86_64');
    const { plugins } = windowsSdk(sdk);
    const sidecar = join(root, 'bridge-server.exe');
    writeFileSync(sidecar, fakePeX64());
    const resources = join(root, 'payload', 'resources');
    mkdirSync(resources, { recursive: true });
    const plan = preflightPortableGStreamerRuntime(sidecar, 'win32', 'x64', {
      LINGXI_GSTREAMER_SDK_WINDOWS: sdk,
    });
    const result = bundlePortableGStreamerRuntime(plan, resources);
    const plugin = join(result.pluginDirectory, 'gstapp.dll');
    writeFileSync(plugin, fakePeX86());
    assert.throws(() => verifyPortableGStreamerRuntime(resources, 'win32', 'x64'), /integrity verification/);

    writeFileSync(plugin, fakePeX64());
    writeFileSync(join(plugins, 'gstvpx.dll'), fakePeX86());
    assert.throws(
      () => preflightPortableGStreamerRuntime(sidecar, 'win32', 'x64', {
        LINGXI_GSTREAMER_SDK_WINDOWS: sdk,
      }),
      /Windows x64 PE/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
