#!/usr/bin/env python3
"""Install checksum-pinned SDK support and rootfs assets for a clean product build."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.request

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from mobile_linux_source import resolve_sdk


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def download(record, base, cache, downloader):
    digest = record['sha256']
    name = record['name']
    if len(digest) != 64 or any(c not in '0123456789abcdef' for c in digest):
        raise ValueError('invalid release SHA256')
    if Path(name).name != name or name in ('', '.', '..') or '\\' in name:
        raise ValueError('invalid release asset name')
    destination = cache / (digest + '.zip')
    if destination.is_symlink():
        raise ValueError('symlinked release cache')
    if destination.exists():
        if sha(destination) != digest:
            raise ValueError('cached release digest mismatch')
        return destination
    url = base + '/' + name
    downloader.require_https(url)
    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            with urllib.request.build_opener(downloader.HTTPSOnly()).open(url, timeout=120) as response:
                total = 0
                while chunk := response.read(1024 * 1024):
                    total += len(chunk)
                    if total > downloader.MAX_ARCHIVE:
                        raise ValueError('SDK release exceeds archive size limit')
                    stream.write(chunk)
            stream.close()
            if sha(temporary) != digest:
                raise ValueError('downloaded SDK release digest mismatch')
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)
    return destination


def publish(source, destination):
    """Replace only this script's generated output, with rollback on failure."""
    if destination.is_symlink():
        raise ValueError('symlinked generated destination')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.sdk-release-', dir=destination.parent) as temporary:
        staging = Path(temporary) / 'new'
        backup = Path(temporary) / 'old'
        shutil.copytree(source, staging)
        if destination.exists():
            destination.rename(backup)
        try:
            staging.rename(destination)
        except BaseException:
            if backup.exists():
                backup.rename(destination)
            raise


def prepare(platform, cache):
    resolved = resolve_sdk()
    sdk = Path(resolved['root'])
    config = json.loads((ROOT / 'scripts/lib/mobile-linux-sdk-release.json').read_text())
    rootfs_config = json.loads((ROOT / 'scripts/lib/mobile-linux-rootfs-release.json').read_text())
    if config['sdk_source_revision'] != resolved['revision'] or config['tag'] != rootfs_config['tag']:
        raise ValueError('published SDK does not match Cargo.lock or rootfs release')
    repository = json.loads((ROOT / 'scripts/lib/mobile-linux-packages.json').read_text())['repository'].removesuffix('.git')
    base = repository + '/releases/download/' + config['tag']
    identity = module('release_identity', sdk / 'scripts/lib/sdk_artifact_identity.py')
    installer = module('release_installer', sdk / 'scripts/release/install-maven-bundle.py')
    cache.mkdir(parents=True, exist_ok=True)
    if cache.is_symlink():
        raise ValueError('symlinked release cache directory')
    native_check = module('product_native', ROOT / 'apps/android/native/scripts/mobile-linux-native.py')
    with tempfile.TemporaryDirectory(prefix='mobile-linux-release-') as temporary:
        staging = Path(temporary)
        def unpack(key):
            directory = staging / key
            directory.mkdir()
            installer.extract(download(config['assets'][key], base, cache, installer), directory)
            return directory
        source = {key: resolved[key] for key in ('source', 'revision')}
        if platform in ('android', 'all'):
            native = unpack('android_native')
            identity.validate_artifacts(native, 'native-manifest.json', resolved['revision'])
            subprocess.run([sys.executable, str(sdk / 'scripts/checks/verify-android-native.py'), '--artifact-dir', str(native)], check=True)
            maven = unpack('android_maven')
            native_check.verify_maven(maven, resolved['revision'])
            manifest = json.loads((maven / 'sdk-artifacts.json').read_text())
            if manifest.get('validation_only') is not False or identity.file_hashes(maven, 'sdk-artifacts.json') != manifest['files']:
                raise ValueError('Maven release cleanliness or file set mismatch')
            android = ROOT / 'apps/android/native'
            publish(maven, android / 'build/mobileLinuxSdk/maven')
            for variant in ('shared', 'play', 'direct'):
                destination = android / 'app/build/mobileLinuxNative' / variant
                publish(native, destination / 'native-support')
                (destination / 'sdk-source.json').write_text(json.dumps(source, indent=2) + '\n')
            rootfs = android / 'app/build/mobileLinuxRootfs'
            subprocess.run([sys.executable, str(ROOT / 'scripts/mobile-linux/fetch-sdk-rootfs.py'), '--output', str(rootfs)], check=True)
            runtime = subprocess.check_output([sys.executable, str(ROOT / 'scripts/lib/runtime_source.py'), '--root'], text=True).strip()
            stager = module('product_rootfs', ROOT / 'apps/android/native/scripts/stage-mobile-linux-assets.py')
            for variant in ('play', 'direct'):
                stager.stage(rootfs, android / f'app/build/generated/mobileLinux/{variant}/assets/mobile-linux', sdk, Path(runtime), android / f'app/build/mobileLinuxNative/{variant}/native-support')
        if platform in ('ios', 'all'):
            native = unpack('ios_native')
            identity.validate_ios_native(native, native / 'MobileLinuxNativeSupport.xcframework', resolved['revision'], source_root=sdk)
            subprocess.run([sys.executable, str(sdk / 'scripts/checks/verify-ios-native.py'), '--artifact-dir', str(native)], check=True)
            (native / 'sdk-source.json').write_text(json.dumps(source, indent=2) + '\n')
            rootfs = unpack('ios_rootfs')
            manifest = json.loads((rootfs / 'manifest.json').read_text())
            pins = json.loads((sdk / 'docs/toolchains/runtime-pins.json').read_text())
            if (manifest.get('sdk_source') != source or manifest.get('rootfs_version') != pins['rootfs_version']
                    or manifest.get('source_archive_sha256') != pins['alpine']['minirootfs']['aarch64']['sha256']
                    or manifest.get('source_manifest_sha256') != sha(sdk / 'native/ios/sources.json')
                    or manifest.get('rootfs_zip_sha256') != sha(rootfs / 'resources/alpine-rootfs.zip')):
                raise ValueError('iOS rootfs identity mismatch')
            ios = ROOT / 'apps/ios/native'
            destination = ios / 'build/linux-runtime/openminis'
            manifest['resources'] = {key: str(destination / 'resources' / name) for key, name in {
                'rootfs_zip': 'alpine-rootfs.zip', 'rootfs_dir': 'alpine-rootfs',
                'default_mount': 'default_mount', 'vdso': 'libvdso.so.elf'}.items()}
            (rootfs / 'resources/default_mount').mkdir(exist_ok=True)
            (rootfs / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
            publish(rootfs, destination)
            publish(native, ios / 'build/mobile-linux-sdk')
            publish(native / 'MobileLinuxNativeSupport.xcframework', ios / 'Frameworks/MobileLinuxNativeSupport.xcframework')
            subprocess.run(['bash', str(ios / 'scripts/verify-linux-runtime.sh')], check=True)
    print(f'Prepared published {config["tag"]} SDK support and rootfs for {platform}; product Rust FFI remains source-built.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=('android', 'ios', 'all'), required=True)
    parser.add_argument('--cache-dir', type=Path, default=ROOT / 'target/mobile-linux-release-cache')
    args = parser.parse_args()
    try:
        prepare(args.platform, args.cache_dir)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'prepare-sdk-release: {error}\n')
