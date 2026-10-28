#!/usr/bin/env python3
"""Observe a real idle Bridge with hostile GStreamer/GIO paths on macOS.

The control modules are compiled into a private temporary directory. This is an
unsigned debug-binary experiment, not proof about the packaged app or active
media sessions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import time


def run(command, *, env=None, timeout=120):
    result = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f'command failed: {command!r}\n{result.stdout}')
    return result


def pids(marker):
    if not marker.exists():
        return []
    return [int(line) for line in marker.read_text().splitlines()]


def build_call_tracer(root, name, declaration, library):
    """Interpose one API without introducing a new GLib/GStreamer dependency."""
    marker = root / (name + '-calls')
    source = root / (name + '-trace.c')
    code = '''#include <dlfcn.h>\n#include <stdio.h>\n#include <unistd.h>\n'''
    code += 'static void record(const char *s) { FILE *f=fopen(' + json.dumps(str(marker)) + ', "a"); if(f){fprintf(f,"%s %ld\\n",s,(long)getpid()); fclose(f);} }\n'
    code += '__attribute__((constructor)) static void loaded(void) {record("loaded");}\n'
    code += declaration + '\n'
    if name == 'gst_init_check':
        code += 'static gboolean traced(int *a,char ***b,GError **c) { record("called"); return gst_init_check(a,b,c); }\n'
    elif name == 'gst_init':
        code += 'static void traced(int *a,char ***b) { record("called"); gst_init(a,b); }\n'
    else:
        code += 'static GTlsBackend *traced(void) { record("called"); return g_tls_backend_get_default(); }\n'
    code += '__attribute__((used)) static struct {const void *replacement_ptr; const void *original_ptr;} entry __attribute__((section("__DATA,__interpose"))) = {(const void *)&traced, (const void *)&' + name + '};\n'
    source.write_text(code)
    output = root / (name + '-trace.dylib')
    # --cflags only: linking to the target libraries here would contaminate
    # the real Bridge's normal load graph and make the observation circular.
    cflags = shlex.split(run(['pkg-config', '--cflags', library]).stdout)
    run(['cc', '-dynamiclib', '-Wl,-undefined,dynamic_lookup', str(source), '-o', str(output), *cflags])
    links = run(['otool', '-L', str(output)]).stdout
    if any(name in links.lower() for name in ('libgstreamer', 'libglib', 'libgio')):
        raise RuntimeError('call tracer introduced a media-library dependency')
    run(['codesign', '--force', '--sign', '-', str(output)])
    return output, marker


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bridge', type=Path, required=True)
    parser.add_argument('--controls-only', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'darwin' or not args.bridge.is_absolute() or not args.bridge.is_file():
        raise RuntimeError('requires macOS and an absolute existing Bridge binary')
    parent = os.environ.get('RUNNER_TEMP') or '/tmp'
    root = Path(tempfile.mkdtemp(prefix='bridge-media-probe-', dir=parent)).resolve()
    gio_dir, gst_dir, home = (root / name for name in ('gio', 'gst', 'home'))
    for path in (gio_dir, gst_dir, home):
        path.mkdir()
    gio_marker, gst_marker = root / 'gio-marker', root / 'gst-marker'
    gio_source, gst_source = root / 'gio.c', root / 'gst.c'
    gio_source.write_text('''#include <gio/gio.h>\n#include <stdio.h>\n#include <unistd.h>\n__attribute__((constructor)) static void hit(void) { FILE *f=fopen(''' + json.dumps(str(gio_marker)) + ''', "a"); if(f){fprintf(f,"%ld\\n",(long)getpid()); fclose(f);} }\nG_MODULE_EXPORT void g_io_module_load(GIOModule *m){(void)m;}\nG_MODULE_EXPORT void g_io_module_unload(GIOModule *m){(void)m;}\n''')
    gst_source.write_text('''#include <gst/gst.h>\n#include <stdio.h>\n#include <unistd.h>\n__attribute__((constructor)) static void hit(void) { FILE *f=fopen(''' + json.dumps(str(gst_marker)) + ''', "a"); if(f){fprintf(f,"%ld\\n",(long)getpid()); fclose(f);} }\nstatic gboolean init(GstPlugin *p){(void)p;return TRUE;}\n#ifndef PACKAGE\n#define PACKAGE "lingxi-bridge-probe"\n#endif\nGST_PLUGIN_DEFINE(GST_VERSION_MAJOR,GST_VERSION_MINOR,lingxibridgeprobe,"probe",init,"1.0","LGPL","lingxi","https://example.invalid")\n''')
    gio_control = root / 'gio-control.c'
    gio_control.write_text('#include <gio/gio.h>\n#include <stdio.h>\n#include <unistd.h>\nint main(void){printf("%ld\\n",(long)getpid()); (void)g_tls_backend_get_default(); return 0;}\n')
    gst_control = root / 'gst-control.c'
    gst_control.write_text('#include <gst/gst.h>\n#include <stdio.h>\n#include <unistd.h>\nint main(void){printf("%ld\\n",(long)getpid()); char *a[]={"lingxi-media","--gst-disable-registry-update","--gst-disable-registry-fork",0}; char **v=a; int n=3; GError *e=0; return gst_init_check(&n,&v,&e)?0:1;}\n')
    gst_init_control = root / 'gst-init-control.c'
    gst_init_control.write_text('#include <gst/gst.h>\n#include <stdio.h>\n#include <unistd.h>\nint main(void){printf("%ld\\n",(long)getpid()); char *a[]={"lingxi-media","--gst-disable-registry-update","--gst-disable-registry-fork",0}; char **v=a; int n=3; gst_init(&n,&v); return 0;}\n')
    gio_flags = shlex.split(run(['pkg-config', '--cflags', '--libs', 'gio-2.0']).stdout)
    gst_flags = shlex.split(run(['pkg-config', '--cflags', '--libs', 'gstreamer-1.0']).stdout)
    gio_lib = run(['pkg-config', '--variable=libdir', 'gio-2.0']).stdout.strip()
    gst_lib = run(['pkg-config', '--variable=libdir', 'gstreamer-1.0']).stdout.strip()
    gio_module = gio_dir / 'libgiobridgeprobe.so'
    gst_plugin = gst_dir / 'libgstlingxibridgeprobe.dylib'
    for source, output, flags, library in (
        (gio_source, gio_module, gio_flags, gio_lib),
        (gst_source, gst_plugin, gst_flags, gst_lib),
    ):
        run(['cc', '-dynamiclib', str(source), '-o', str(output), *flags, '-Wl,-rpath,' + library])
        run(['codesign', '--force', '--sign', '-', str(output)])
    controls = []
    for source, name, flags, library in (
        (gio_control, 'gio-control', gio_flags, gio_lib),
        (gst_control, 'gst-control', gst_flags, gst_lib),
        (gst_init_control, 'gst-init-control', gst_flags, gst_lib),
    ):
        output = root / name
        run(['cc', str(source), '-o', str(output), *flags, '-Wl,-rpath,' + library])
        run(['codesign', '--force', '--sign', '-', str(output)])
        controls.append(output)
    clean = {k: v for k, v in os.environ.items()
             if not k.upper().startswith(('GST_', 'GIO_', 'DYLD_', 'LD_'))}
    clean.update({
        'HOME': str(home),
        'GIO_MODULE_DIR': str(gio_dir),
        'GST_PLUGIN_PATH_1_0': str(gst_dir),
        'GST_PLUGIN_SYSTEM_PATH_1_0': '',
        'GST_REGISTRY_FORK': 'no',
        'GST_REGISTRY_1_0': str(root / 'control-registry.bin'),
    })
    for executable, marker in zip(controls, (gio_marker, gst_marker)):
        marker.unlink(missing_ok=True)
        result = run([str(executable)], env=clean)
        pid = int(result.stdout.strip().splitlines()[0])
        if pid not in pids(marker):
            raise RuntimeError(f'{executable.name} did not load its hostile module in the control process: {pids(marker)}')
        print(f'{executable.name}: hostile module constructor executed in process {pid}')
    tracers = [
        build_call_tracer(root, 'g_tls_backend_get_default', '#include <gio/gio.h>', 'gio-2.0'),
        build_call_tracer(root, 'gst_init_check', '#include <gst/gst.h>', 'gstreamer-1.0'),
        build_call_tracer(root, 'gst_init', '#include <gst/gst.h>', 'gstreamer-1.0'),
    ]
    for control, (tracer, calls) in zip(controls, tracers):
        result = run([str(control)], env=dict(clean, DYLD_INSERT_LIBRARIES=str(tracer)))
        pid = int(result.stdout.strip().splitlines()[0])
        observed = calls.read_text().splitlines() if calls.exists() else []
        if f'loaded {pid}' not in observed or f'called {pid}' not in observed:
            raise RuntimeError(f'call tracer missed actual positive control: {observed}')
        print(f'{tracer.name}: actual API call observed in control process {pid}')
        calls.unlink()
    gio_marker.unlink(missing_ok=True)
    gst_marker.unlink(missing_ok=True)
    if args.controls_only:
        return
    links = run(['otool', '-L', str(args.bridge)]).stdout
    workspace = root / 'workspace'; workspace.mkdir()
    config_home, bridge_dir = root / 'config', root / 'bridge'
    stdout_path, stderr_path = root / 'bridge.stdout', root / 'bridge.stderr'
    child_env = dict(clean, GST_REGISTRY_1_0=str(root / 'bridge-registry.bin'),
                     DYLD_PRINT_LIBRARIES='1', DYLD_PRINT_INITIALIZERS='1')
    command = [
        str(args.bridge), '--cwd', str(workspace), '--session-config-home', str(config_home),
        '--session-global-config-path', str(root / 'global.json'), '--bridge-dir', str(bridge_dir),
    ]
    with stdout_path.open('wb') as stdout, stderr_path.open('wb') as stderr:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   env=child_env)
        try:
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f'Bridge exited before publishing lock: {process.returncode}')
                if bridge_dir.is_dir() and list(bridge_dir.glob('*.lock')):
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError('Bridge did not publish its lock')
            time.sleep(1)
            process.send_signal(signal.SIGTERM)
            if process.wait(timeout=30) != 0:
                raise RuntimeError(f'Bridge did not terminate cleanly: {process.returncode}')
        except BaseException:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            raise
    dyld = stderr_path.read_text(errors='replace').splitlines()
    selected = [line for line in dyld if f'dyld[{process.pid}]' in line and
                any(name in line.lower() for name in ('libgstreamer', 'libglib', 'libgio', 'libgst'))]
    loaded = [line for line in selected if 'running initializer' not in line]
    initializers = [line for line in selected if 'running initializer' in line]
    if not any('libgstreamer' in line.lower() for line in loaded):
        raise RuntimeError('dyld trace did not show GStreamer loaded in the exact Bridge process')
    if not any('libglib' in line.lower() for line in loaded):
        raise RuntimeError('dyld trace did not show GLib loaded in the exact Bridge process')
    result = {
        'bridge_sha256': hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        'sdk_glib': run(['pkg-config', '--modversion', 'gio-2.0']).stdout.strip(),
        'bridge_pid': process.pid,
        'otool_direct_dependencies': [line for line in links.splitlines()
                                      if any(name in line.lower() for name in ('libgstreamer','libglib','libgio','libgst'))],
        'dyld_loaded_images': loaded,
        'dyld_initializers': initializers,
        'gio_constructor_pids': pids(gio_marker),
        'gst_constructor_pids': pids(gst_marker),
        'scope': 'idle Bridge through startup and graceful SIGTERM only; no active media RPC or signed package',
    }
    print(json.dumps(result, indent=2))
    if pids(gio_marker) or pids(gst_marker):
        raise RuntimeError('hostile module was loaded in Bridge or its descendants during this experiment')

    # This second run intentionally inserts debug interposers, separately from
    # the unmodified loader trace above. Absence is limited to these exact APIs
    # and this idle startup; it is not evidence about all future GIO calls.
    for path in bridge_dir.glob('*'):
        if path.is_file():
            path.unlink()
    traced_env = dict(clean, GST_REGISTRY_1_0=str(root / 'traced-registry.bin'),
                      DYLD_INSERT_LIBRARIES=':'.join(str(path) for path, _ in tracers))
    with (root / 'traced.stdout').open('wb') as stdout, (root / 'traced.stderr').open('wb') as stderr:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   env=traced_env)
        try:
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f'traced Bridge exited before publishing lock: {process.returncode}')
                if bridge_dir.is_dir() and list(bridge_dir.glob('*.lock')):
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError('traced Bridge did not publish its lock')
            time.sleep(1)
            process.send_signal(signal.SIGTERM)
            if process.wait(timeout=30) != 0:
                raise RuntimeError(f'traced Bridge did not terminate cleanly: {process.returncode}')
        except BaseException:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            raise
    trace_result = {}
    for tracer, marker in tracers:
        observed = marker.read_text().splitlines() if marker.exists() else []
        if f'loaded {process.pid}' not in observed:
            raise RuntimeError(f'{tracer.name} not loaded in exact Bridge process: {observed}')
        trace_result[tracer.name] = observed
    print(json.dumps({'traced_bridge_pid': process.pid, 'api_observations': trace_result,
                      'scope': 'debug interposed idle Bridge only; selected APIs, not all GIO'}, indent=2))


if __name__ == '__main__':
    main()
