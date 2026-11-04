#!/usr/bin/env python3
"""Smoke-test actual XCFramework support selection against a built SDK artifact.

The harness stops the real script after native-support verification, before any
FFI build or output replacement.  GStreamer staging and Cargo SDK discovery
are isolated; the SDK's verifier and the support artifact are real.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--sdk-root', type=Path, required=True)
parser.add_argument('--sdk-output', type=Path, required=True)
parser.add_argument('--frameworks', type=Path, required=True)
args = parser.parse_args()
sdk_root = args.sdk_root.resolve(strict=True)
sdk_output = args.sdk_output.resolve(strict=True)
frameworks = args.frameworks.resolve(strict=True)
recorded = json.loads((sdk_output / 'sdk-source.json').read_text())
script = Path(__file__).resolve().with_name('build-xcframework.sh').read_text()
marker = 'SIMULATOR_SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"'
if script.count(marker) != 1:
    raise SystemExit('Cannot identify the native-support verification boundary')
prefix = script.split(marker, 1)[0] + '\nprintf "native-support selection complete\\n"\n'

with tempfile.TemporaryDirectory(prefix='lingxi-native-support-selection-') as temporary:
    repo = Path(temporary) / 'repo'
    scripts = repo / 'apps/ios/native/scripts'
    library = repo / 'scripts/lib'
    scripts.mkdir(parents=True)
    library.mkdir(parents=True)
    (scripts / 'build-xcframework.sh').write_text(prefix)
    (library / 'mobile_gstreamer_cargo.py').write_text('pass\n')
    (library / 'mobile_linux_source.py').write_text('''
import json, os, sys
if '--root' in sys.argv:
    print(os.environ['TEST_SDK_ROOT'])
else:
    source = json.loads(os.environ['TEST_SDK_SOURCE'])
    source['revision'] = os.environ.get('TEST_SDK_REVISION', source['revision'])
    print(json.dumps(source))
''')
    default_stage = repo / 'apps/ios/native/build/mobile-linux-sdk'
    default_stage.parent.mkdir(parents=True)
    default_stage.symlink_to(sdk_output, target_is_directory=True)
    invalid_stage = Path(temporary) / 'wrong-kind'
    invalid_stage.mkdir()
    manifest = json.loads((sdk_output / 'native-support-manifest.json').read_text())
    manifest['kind'] = 'not-native-support'
    (invalid_stage / 'native-support-manifest.json').write_text(json.dumps(manifest))

    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(('LINGXI_FFI_', 'LINGXI_REUSE_', 'LINGXI_SIM_', 'LINGXI_DEVICE_', 'LINGXI_ARM64_', 'TEST_SDK_'))}
    environment.update(TEST_SDK_ROOT=str(sdk_root), TEST_SDK_SOURCE=json.dumps(recorded),
                       LINGXI_FRAMEWORKS_DIR=str(frameworks), IPHONEOS_DEPLOYMENT_TARGET='26.0')

    def run(name, changes, succeeds, expected):
        run_environment = environment.copy()
        run_environment.pop('LINGXI_IOS_NATIVE_SUPPORT_OUTPUT', None)
        run_environment.update(changes)
        result = subprocess.run(['bash', str(scripts / 'build-xcframework.sh')],
                                env=run_environment, capture_output=True, text=True)
        combined = result.stdout + result.stderr
        if (result.returncode == 0) != succeeds or expected not in combined:
            raise RuntimeError(f'{name}: exit={result.returncode}\n{combined}')
        print(f'PASS {name}')

    sim = {'LINGXI_SIM_ARM64_ONLY': '1', 'LINGXI_REUSE_STAGED_LINUX_RUNTIME': '1'}
    reuse = {'LINGXI_SIM_ARM64_ONLY': '1', 'LINGXI_FFI_REUSE_NATIVE_SUPPORT_ONLY': '1'}
    override = {'LINGXI_IOS_NATIVE_SUPPORT_OUTPUT': str(sdk_output)}
    run('simulator reuse, explicit stage', sim | override, True, 'native-support selection complete')
    run('native-support reuse, explicit stage', reuse | override, True, 'native-support selection complete')
    run('simulator reuse, default stage', sim, True, 'native-support selection complete')
    run('reject incorrect SDK artifact kind', sim | {'LINGXI_IOS_NATIVE_SUPPORT_OUTPUT': str(invalid_stage)},
        False, 'expected a native-support artifact without a Rust core')
    run('reject simulator-only support for device',
        {'LINGXI_DEVICE_ONLY': '1', 'LINGXI_FFI_REUSE_NATIVE_SUPPORT_ONLY': '1'} | override,
        False, 'staged native support is missing a required execution slice')
    run('reject a different SDK revision', sim | override | {'TEST_SDK_REVISION': 'mismatched'},
        False, 'native support SDK revision differs from Cargo.lock')
