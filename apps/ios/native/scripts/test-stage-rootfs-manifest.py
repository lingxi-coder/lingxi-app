#!/usr/bin/env python3
"""Exercise the Xcode rootfs build phase with isolated staged files."""
import os
from pathlib import Path
import subprocess
import tempfile

native = Path(__file__).resolve().parent.parent
extract = (
    'require "yaml"; project = YAML.load_file(ARGV.fetch(0)); '
    'phase = project.fetch("targets").fetch("LingxiCode").fetch("preBuildScripts")'
    '.find { |item| item["name"] == "Stage Alpine rootfs" }; '
    'abort "rootfs phase missing" unless phase; print phase.fetch("script")'
)
script = subprocess.check_output(['ruby', '-e', extract, str(native / 'project.yml')], text=True)

with tempfile.TemporaryDirectory(prefix='lingxi-ios-rootfs-phase-') as temporary:
    root = Path(temporary)
    source = root / 'checkout/apps/ios/native'
    stage = source / 'build/linux-runtime/openminis'
    resources = stage / 'resources'
    resources.mkdir(parents=True)
    archive = b'isolated rootfs fixture\n'
    (resources / 'alpine-rootfs.zip').write_bytes(archive)
    (resources / 'runtime-manifest.json').write_text('{"legacy":true}\n')
    (resources / 'RootfsPatch.bundle').mkdir()
    destination = root / 'products/LingxiCode.app'
    environment = dict(os.environ, SRCROOT=str(source), PLATFORM_NAME='iphoneos',
                       TARGET_BUILD_DIR=str(destination.parent),
                       UNLOCALIZED_RESOURCES_FOLDER_PATH=destination.name)

    missing = subprocess.run(['/bin/bash', '-euo', 'pipefail', '-c', script],
                             env=environment, capture_output=True, text=True)
    assert missing.returncode != 0, 'legacy metadata must not satisfy the current build phase'
    assert 'Linux runtime manifest is missing' in missing.stdout + missing.stderr
    assert not (destination / 'linux-runtime-manifest.json').exists()

    current = b'{"current":true}\n'
    (stage / 'manifest.json').write_bytes(current)
    subprocess.run(['/bin/bash', '-euo', 'pipefail', '-c', script],
                   env=environment, check=True, capture_output=True, text=True)
    assert (destination / 'linux-runtime-manifest.json').read_bytes() == current
    assert (destination / 'alpine-rootfs.zip').read_bytes() == archive

    simulator = dict(environment, PLATFORM_NAME='iphonesimulator', SRCROOT=str(root / 'no-stage'))
    subprocess.run(['/bin/bash', '-euo', 'pipefail', '-c', script],
                   env=simulator, check=True, capture_output=True, text=True)

print('Rootfs staging: current manifest required; legacy-only and simulator cases passed.')
