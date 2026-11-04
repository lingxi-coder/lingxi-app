from pathlib import Path
import hashlib
import json
import subprocess
import time

here = Path(__file__).resolve().parent
g = here.parents[2]
src = g/'audit-finding-fixes/cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f'
stable = src/'stable_deref_trait-1.2.1/src/lib.rs'
untrusted = src/'untrusted-0.9.0/src/lib.rs'
packaged_tests = src/'untrusted-0.9.0/tests/tests.rs'
rustc=['rustc','+1.94.0']
steps = [
    rustc+['--version','--verbose'],
    rustc+['--edition=2015','--crate-name','stable_deref_trait','--crate-type=rlib',str(stable),'-o','libstable_core.rlib'],
    rustc+['--edition=2015','--crate-name','stable_deref_trait','--crate-type=rlib','--cfg','feature="alloc"',str(stable),'-o','libstable_alloc.rlib'],
    rustc+['--edition=2015','--crate-name','stable_deref_trait','--crate-type=rlib','--cfg','feature="alloc"','--cfg','feature="std"',str(stable),'-o','libstable_std.rlib'],
    rustc+['--edition=2018','--test','stable-reference.rs','--extern','stable_deref_trait=libstable_std.rlib','-o','stable-reference'],
    ['./stable-reference'],
    ['rustdoc','+1.94.0','--edition=2015','--test','--crate-name','stable_deref_trait','--cfg','feature="alloc"','--cfg','feature="std"',str(stable)],
    rustc+['--edition=2018','--crate-name','untrusted','--crate-type=rlib',str(untrusted),'-o','libuntrusted.rlib'],
    rustc+['--edition=2018','--test',str(packaged_tests),'--extern','untrusted=libuntrusted.rlib','-o','untrusted-packaged-tests'],
    ['./untrusted-packaged-tests'],
    rustc+['--edition=2018','--test','untrusted-reference.rs','--extern','untrusted=libuntrusted.rlib','-o','untrusted-reference'],
    ['./untrusted-reference'],
    rustc+['--edition=2018','--test','untrusted-invariant-probe.rs','--extern','untrusted=libuntrusted.rlib','-o','untrusted-invariant-probe'],
    ['./untrusted-invariant-probe','--nocapture'],
]
for target in ['aarch64-apple-ios','aarch64-linux-android','x86_64-pc-windows-gnu']:
    steps.append(rustc+['--edition=2015','--crate-name','stable_deref_trait','--crate-type=rlib','--target',target,'--cfg','feature="alloc"','--cfg','feature="std"',str(stable),'-o',f'libstable-{target}.rlib'])
    steps.append(rustc+['--edition=2018','--crate-name','untrusted','--crate-type=rlib','--target',target,str(untrusted),'-o',f'libuntrusted-{target}.rlib'])

result={'input_sha256':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in
                         [stable, untrusted, packaged_tests, here/'stable-reference.rs', here/'untrusted-reference.rs', here/'untrusted-invariant-probe.rs']},'steps':[]}
for cmd in steps:
    start=time.monotonic()
    p=subprocess.run(cmd,cwd=here,text=True,capture_output=True)
    result['steps'].append({'argv':cmd,'exit':p.returncode,'seconds':round(time.monotonic()-start,3),'stdout':p.stdout,'stderr':p.stderr})
    (here/'probe-results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(f'{p.returncode}: {" ".join(cmd[:5])}',flush=True)
    if p.returncode: raise SystemExit(p.returncode)
