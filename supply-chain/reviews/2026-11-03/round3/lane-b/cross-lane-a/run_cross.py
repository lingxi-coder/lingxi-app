from pathlib import Path
import hashlib, json, os, subprocess, time

here=Path(__file__).resolve().parent
g=here.parents[3]
src=g/'audit-finding-fixes/cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f'
check=src/'version_check-0.9.5'
compare=src/'version-compare-0.2.1'
primary=g/'source-audits/round3-20261103/lane-a/probe'
rustc=['rustc','+1.94.0']
commands=[
    rustc+['--version','--verbose'],
    rustc+['--edition=2015','--crate-name','version_check','--crate-type=rlib',str(check/'src/lib.rs'),'-o','libversion_check.rlib'],
    rustc+['--edition=2021','--crate-name','version_compare','--crate-type=rlib',str(compare/'src/lib.rs'),'-o','libversion_compare.rlib'],
    rustc+['--edition=2015','--test',str(check/'src/lib.rs'),'-o','version_check_tests'],
    ['./version_check_tests'],
    rustc+['--edition=2021','--test',str(compare/'src/lib.rs'),'-o','version_compare_tests'],
    ['./version_compare_tests'],
    rustc+['--edition=2015',str(primary/'version_check_probe.rs'),'--extern','version_check=libversion_check.rlib','-o','version_check_probe'],
    ['./version_check_probe',str(primary/'nonzero-rustc')],
    rustc+['--edition=2021',str(primary/'version_compare_probe.rs'),'--extern','version_compare=libversion_compare.rlib','-o','version_compare_probe'],
    ['./version_compare_probe'],
]
environment=os.environ.copy()
environment['CARGO_MANIFEST_DIR']=str(check)
environment.pop('FORCE_STATIC',None)
environment.pop('KNOWN_CHANNEL',None)
data={'inputs':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in
                [check/'src/lib.rs',compare/'src/lib.rs',primary/'version_check_probe.rs',primary/'version_compare_probe.rs',primary/'nonzero-rustc']},'steps':[]}
for cmd in commands:
    stat=os.statvfs(here)
    available=stat.f_bavail*stat.f_frsize
    if available<4*1024**3:
        data['disk_blocked_bytes']=available
        (here/'cross-probe-results.json').write_text(json.dumps(data,indent=2)+'\n')
        raise SystemExit('less than 4 GiB free: no new probe started')
    start=time.monotonic()
    p=subprocess.run(cmd,cwd=here,env=environment,text=True,capture_output=True)
    data['steps'].append({'argv':cmd,'exit':p.returncode,'seconds':round(time.monotonic()-start,3),'stdout':p.stdout,'stderr':p.stderr})
    (here/'cross-probe-results.json').write_text(json.dumps(data,indent=2)+'\n')
    print(f'{p.returncode}: {" ".join(cmd[:5])}',flush=True)
    if p.returncode: raise SystemExit(p.returncode)
