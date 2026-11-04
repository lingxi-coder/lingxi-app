from pathlib import Path
import hashlib
import io
import json
import subprocess
import tarfile
import tomllib

here = Path(__file__).resolve().parent
g = here.parents[2]
app = g / 'delivery-round1/lingxi-app'
registry = g / 'audit-finding-fixes/cargo-home/registry'
cache = registry / 'cache/index.crates.io-1949cf8c6b5b557f'
source = registry / 'src/index.crates.io-1949cf8c6b5b557f'
index = Path('/private/tmp/media-online-audit-20261103/cargo-home/registry/index/index.crates.io-1949cf8c6b5b557f/.cache')
lock_bytes = (app/'Cargo.lock').read_bytes()
lock = tomllib.loads(lock_bytes.decode())
result = {'app_head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=app, text=True).strip(),
          'lock_sha256': hashlib.sha256(lock_bytes).hexdigest(), 'packages': []}
for name, version in [('stable_deref_trait','1.2.1'),('untrusted','0.9.0')]:
    locked = next(x for x in lock['package'] if x['name']==name and x['version']==version)
    assert locked['source']=='registry+https://github.com/rust-lang/crates.io-index'
    archive = (here/f'{name}-{version}.crate').read_bytes()
    digest = hashlib.sha256(archive).hexdigest()
    local = (cache/f'{name}-{version}.crate').read_bytes()
    index_data = (index/name[:2]/name[2:4]/name).read_bytes().split(b'\0')
    entry = next(json.loads(x) for x in index_data if x.startswith(b'{') and json.loads(x).get('vers')==version)
    assert digest == hashlib.sha256(local).hexdigest() == locked['checksum'] == entry['cksum']
    root = source/f'{name}-{version}'
    files={}
    with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as tar:
        for member in tar.getmembers():
            assert member.name.startswith(f'{name}-{version}/')
            rel=Path(member.name).relative_to(f'{name}-{version}')
            assert not rel.is_absolute() and '..' not in rel.parts
            if member.isdir(): continue
            assert member.isfile()
            data=tar.extractfile(member).read()
            assert data == (root/rel).read_bytes(), str(rel)
            files[str(rel)]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest(),'mode':oct(member.mode)}
    extras=sorted(str(x.relative_to(root)) for x in root.rglob('*') if x.is_file() and str(x.relative_to(root)) not in files)
    result['packages'].append({'name':name,'version':version,'sha256':digest,'yanked':entry['yanked'],
                               'archive_url':f'https://static.crates.io/crates/{name}/{name}-{version}.crate',
                               'files':files,'extra_source_files':extras})
(here/'identity.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'app_head':result['app_head'],'lock_sha256':result['lock_sha256'],
                  'packages':[{'name':p['name'],'version':p['version'],'sha256':p['sha256'],
                               'files':len(p['files']),'extras':p['extra_source_files']} for p in result['packages']]},indent=2))
