#!/usr/bin/env python3
"""Force GIO and installer lookup inside a codec launch using private test copies.

The copied SDK libraries are modified only in this temporary fixture. This
checks the launch environment and the real Bridge worker, not a signed bundle.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile


def run(command, *, env=None, check=True):
    result = subprocess.run(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180)
    if check and result.returncode:
        raise RuntimeError(f"command failed: {command!r}\n{result.stdout}")
    return result


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def install_id(path):
    output = run(['otool', '-arch', 'arm64', '-D', str(path)]).stdout.splitlines()
    values = [line.strip() for line in output if line.strip() and not line.rstrip().endswith(':')]
    if len(values) != 1:
        raise RuntimeError(f'unexpected install ID for {path}: {output}')
    return values[0]


def replace_c_string(path, suffix, target):
    data = path.read_bytes()
    strings = [part for part in data.split(b'\0') if part.startswith(b'/') and part.endswith(suffix)]
    strings = sorted(set(strings))
    if len(strings) != 1:
        raise RuntimeError(f'expected one compiled path ending {suffix!r} in {path}: {strings!r}')
    original = strings[0] + b'\0'
    replacement = os.fsencode(target) + b'\0'
    if len(replacement) > len(original):
        raise RuntimeError('test path is too long for copied SDK string')
    count = data.count(original)
    path.write_bytes(data.replace(original, replacement + b'\0' * (len(original) - len(replacement))))
    return {'original': os.fsdecode(strings[0]), 'copies': count}


def sign(path):
    run(['codesign', '--force', '--sign', '-', str(path)])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bridge', required=True, type=Path)
    parser.add_argument('--expect', choices=['blocked', 'unsafe'], required=True)
    parser.add_argument('--controls-only', action='store_true',
                        help='check the small native fixture without invoking Cargo')
    args = parser.parse_args()
    if sys.platform != 'darwin' or not args.bridge.is_absolute() or not args.bridge.is_file():
        raise RuntimeError('diagnostic requires macOS and an absolute existing Bridge binary')
    root = Path(tempfile.mkdtemp(prefix='lgi', dir='/tmp'))
    empty = root / 'empty'; empty.mkdir()
    disabled = root / 'disabled'; disabled.mkdir()
    gio_hit = root / 'gio-hit'; installer_hit = root / 'installer-hit'; wrapper_hit = root / 'wrapper-hit'
    installer = root / 'helper'
    installer.write_text('#!/bin/sh\nprintf invoked >> ' + shlex.quote(str(installer_hit)) + '\nexit 0\n')
    installer.chmod(0o700)
    module_source = root / 'gio-module.c'
    module_source.write_text('''#include <gio/gio.h>\n#include <stdio.h>\n#include <unistd.h>\n__attribute__((constructor)) static void hit(void) { FILE *f=fopen(''' + json.dumps(str(gio_hit)) + ''', "a"); if(f){fprintf(f,"%ld\\n",(long)getpid());fclose(f);} }\nG_MODULE_EXPORT void g_io_module_load(GIOModule *m){(void)m;}\nG_MODULE_EXPORT void g_io_module_unload(GIOModule *m){(void)m;}\n''')
    wrapper_source = root / 'wrapper.c'
    wrapper_source.write_text('''#include <gio/gio.h>\n#include <gst/gst.h>\n#include <gst/pbutils/install-plugins.h>\n#include <stdio.h>\n#include <string.h>\n#include <unistd.h>\nint main(int argc,char **argv){ if(argc!=2)return 64; FILE *f=fopen(''' + json.dumps(str(wrapper_hit)) + ''', "a"); if(!f)return 65; fprintf(f,"%ld\\n",(long)getpid()); fclose(f); (void)g_tls_backend_get_default(); gst_init(0,0); const gchar *details[]={"gstreamer|1.0|lingxi-test|decoder-test|",0}; (void)gst_install_plugins_sync(details,0); if(!strcmp(argv[1],"--probe-only"))return 0; execl(argv[1],argv[1],"--desktop-codec-worker",(char*)0);return 127;}\n''')
    flags = shlex.split(run(['pkg-config', '--cflags', '--libs', 'gio-2.0', 'gstreamer-pbutils-1.0']).stdout)
    gio_lib = Path(run(['pkg-config', '--variable=libdir', 'gio-2.0']).stdout.strip()) / 'libgio-2.0.0.dylib'
    gst_lib = Path(run(['pkg-config', '--variable=libdir', 'gstreamer-pbutils-1.0']).stdout.strip()) / 'libgstpbutils-1.0.0.dylib'
    sdk_lib = str(gio_lib.parent.resolve())
    gio_id, gst_id = install_id(gio_lib), install_id(gst_lib)
    base = {key: value for key, value in os.environ.items() if not key.upper().startswith(('GIO_', 'GST_', 'DYLD_', 'LD_'))}
    base.update({'GST_PLUGIN_PATH_1_0': '', 'GST_PLUGIN_SYSTEM_PATH_1_0': '', 'GST_REGISTRY_DISABLE': 'yes',
                 'GST_REGISTRY_FORK': 'no', 'GST_REGISTRY_1_0': str(root / 'registry.bin')})
    receipt = {'sdk_glib': run(['pkg-config', '--modversion', 'gio-2.0']).stdout.strip(),
               'sdk_gio_sha256': sha(gio_lib), 'sdk_pbutils_sha256': sha(gst_lib),
               'bridge_sha256': sha(args.bridge), 'expect': args.expect, 'variants': {}}
    for variant in ['adjacent', 'fallback']:
        folder = root / ('a' if variant == 'adjacent' else 'f'); folder.mkdir()
        copied_gio = folder / 'g.dylib'; copied_gst = folder / 'p.dylib'
        shutil.copy2(gio_lib, copied_gio); copied_gio.chmod(0o700)
        shutil.copy2(gst_lib, copied_gst); copied_gst.chmod(0o700)
        if variant == 'adjacent':
            modules = folder / 'gio/modules'; modules.mkdir(parents=True)
            gio_path = {'mode': 'adjacent'}
        else:
            modules = root / 'fallback'; modules.mkdir()
            gio_path = replace_c_string(copied_gio, b'/gio/modules', modules)
        helper_path = replace_c_string(copied_gst, b'/gst-install-plugins-helper', installer)
        sign(copied_gio); sign(copied_gst)
        module = modules / 'libgioprobe.so'
        run(['cc', '-dynamiclib', str(module_source), '-o', str(module), *flags, '-Wl,-rpath,' + sdk_lib])
        sign(module)
        wrapper = folder / 'wrapper'
        run(['cc', str(wrapper_source), '-o', str(wrapper), *flags, '-Wl,-headerpad_max_install_names', '-Wl,-rpath,' + sdk_lib])
        run(['install_name_tool', '-change', gio_id, str(copied_gio), '-change', gst_id, str(copied_gst), str(wrapper)])
        sign(wrapper)
        def clear():
            gio_hit.unlink(missing_ok=True); installer_hit.unlink(missing_ok=True); wrapper_hit.unlink(missing_ok=True)
        def hits(): return {'gio': gio_hit.exists(), 'installer': installer_hit.exists()}
        clear(); direct = run([str(wrapper), '--probe-only'], env=base); direct_hits = hits()
        if direct_hits != {'gio': True, 'installer': True}:
            raise RuntimeError(f'{variant}: hostile default control did not execute both paths: {direct_hits}; {direct.stdout}')
        clear(); protected = run([str(wrapper), '--probe-only'], env=dict(base, GIO_MODULE_DIR=str(empty), GST_INSTALL_PLUGINS_HELPER=str(disabled)))
        if any(hits().values()):
            raise RuntimeError(f'{variant}: explicit safe directories did not block both paths: {protected.stdout}')
        actual_hits = None
        if not args.controls_only:
            clear(); env = dict(os.environ, LINGXI_TEST_CODEC_GIO_WRAPPER=str(wrapper), LINGXI_TEST_CODEC_BRIDGE_BINARY=str(args.bridge))
            actual = run(['cargo', 'test', '--locked', '-p', 'remote-control-client', '--features', 'native-media', '--test', 'codec_worker', 'real_rust_codec_worker_encodes_audio_and_confirms_exit', '--', '--exact', '--test-threads=1', '--nocapture', '--color', 'never'], env=env)
            if 'test result: ok. 1 passed; 0 failed; 0 ignored;' not in actual.stdout or not wrapper_hit.exists():
                raise RuntimeError(f'{variant}: expected one test and an executed wrapper: {actual.stdout}')
            actual_hits = hits(); wanted = args.expect == 'unsafe'
            if actual_hits != {'gio': wanted, 'installer': wanted}:
                raise RuntimeError(f'{variant}: worker marker expectation failed: {actual_hits}; {actual.stdout}')
        print(f'{variant}: direct hostile control executed both; explicit override blocked both; worker markers={actual_hits}')
        receipt['variants'][variant] = {'gio_path': gio_path, 'helper_path': helper_path, 'actual_markers': actual_hits,
                                       'wrapper_sha256': sha(wrapper), 'module_sha256': sha(module)}
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
