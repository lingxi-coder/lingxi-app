from pathlib import Path
import hashlib, io, json, tarfile, tomllib

here=Path(__file__).resolve().parent
g=here.parents[3]
lock=tomllib.loads((g/'delivery-round1/lingxi-app/Cargo.lock').read_text())
registry=g/'audit-finding-fixes/cargo-home/registry'
src=registry/'src/index.crates.io-1949cf8c6b5b557f'
cache=registry/'cache/index.crates.io-1949cf8c6b5b557f'
results=[]
for name,version in [('version_check','0.9.5'),('version-compare','0.2.1')]:
    p=next(p for p in lock['package'] if p['name']==name and p['version']==version)
    data=(here/f'{name}-{version}.crate').read_bytes()
    digest=hashlib.sha256(data).hexdigest()
    assert p['source']=='registry+https://github.com/rust-lang/crates.io-index'
    assert digest==p['checksum']==hashlib.sha256((cache/f'{name}-{version}.crate').read_bytes()).hexdigest()
    files={}
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
        for item in archive.getmembers():
            assert item.name.startswith(f'{name}-{version}/')
            rel=Path(item.name).relative_to(f'{name}-{version}')
            assert not rel.is_absolute() and '..' not in rel.parts
            if item.isdir(): continue
            assert item.isfile()
            contents=archive.extractfile(item).read()
            assert contents==(src/f'{name}-{version}'/rel).read_bytes()
            files[str(rel)]=hashlib.sha256(contents).hexdigest()
    results.append({'name':name,'version':version,'sha256':digest,'files':files})
(here/'identity.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps([{'name':p['name'],'version':p['version'],'sha256':p['sha256'],'files':len(p['files'])} for p in results],indent=2))
