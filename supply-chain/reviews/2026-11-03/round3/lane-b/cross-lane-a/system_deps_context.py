from pathlib import Path
import hashlib, json, tarfile, tomllib

here=Path(__file__).resolve().parent
g=here.parents[3]
registry=g/'audit-finding-fixes/cargo-home/registry'
lock=tomllib.loads((g/'delivery-round1/lingxi-app/Cargo.lock').read_text())
result=[]
for v in ['6.2.2','7.0.8','9.0.0']:
    name=f'system-deps-{v}'
    archive=registry/'cache/index.crates.io-1949cf8c6b5b557f'/f'{name}.crate'
    digest=hashlib.sha256(archive.read_bytes()).hexdigest()
    assert digest==next(p['checksum'] for p in lock['package'] if p['name']=='system-deps' and p['version']==v)
    with tarfile.open(archive) as tar:
        expected=tar.extractfile(f'{name}/src/lib.rs').read()
    actual=(registry/'src/index.crates.io-1949cf8c6b5b557f'/name/'src/lib.rs').read_bytes()
    assert actual==expected
    result.append({'version':v,'archive_sha256':digest,'src_lib_sha256':hashlib.sha256(actual).hexdigest()})
(here/'system-deps-context.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
