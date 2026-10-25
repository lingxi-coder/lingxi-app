#!/usr/bin/env python3
"""Export checked-in platform icons from assets/brand/identity-v1.

Generation needs Pillow and rsvg-convert. --check only verifies
the source/output manifest and can run on every platform without those tools.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'assets/brand/identity-v1'
MANIFEST = ROOT / 'assets/brand/platform-assets.json'

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.check:
        data = json.loads(MANIFEST.read_text())
        stale = [name for section in ('sources', 'outputs') for name, sha in data[section].items()
                 if not (ROOT/name).is_file() or digest(ROOT/name) != sha]
        if stale:
            raise SystemExit('Stale brand assets: ' + ', '.join(stale))
        print(f"Brand assets verified: {len(data['outputs'])} files")
        return
    from PIL import Image
    outputs = []
    sources = [SOURCE/'products/code/app-master.svg', SOURCE/'products/code/icon.svg',
               SOURCE/'products/code/small.svg', SOURCE/'svg/favicon.svg',
               SOURCE/'svg/lockup-bilingual-ink.svg', SOURCE/'svg/lockup-bilingual-white.svg']
    master = sources[0].read_text()
    rounded = sources[1].read_text()
    foreground = master.replace('<rect width="128" height="128" rx="0" fill="#4167ED"/>', '')
    assert foreground != master
    def wrap(body):
        return '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">'+body+'</svg>'
    def inner(s):
        return s[s.index('>')+1:s.rindex('</svg>')]
    adaptive = wrap('<g transform="translate(26 26) scale(.59375)">'+inner(foreground)+'</g>')
    desktop = wrap('<g transform="translate(12 12) scale(.8125)">'+inner(rounded)+'</g>')
    circle = wrap('<defs><clipPath id="round"><circle cx="64" cy="64" r="64"/></clipPath></defs><g clip-path="url(#round)">'+inner(master)+'</g>')
    def render(source, path, size, opaque=False):
        path = ROOT/path
        path.parent.mkdir(parents=True, exist_ok=True)
        data = subprocess.run(['rsvg-convert','-w',str(size),'-h',str(size)],input=source.encode(),stdout=subprocess.PIPE,check=True).stdout
        path.write_bytes(data)
        if opaque:
            with Image.open(path) as im:
                im.convert('RGB').save(path)
        outputs.append(path)
    def copy(source, target):
        target=ROOT/target
        target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(source,target)
        outputs.append(target)
    render(master, 'assets/brand/lingxi-app-icon-source.png',1024,True)
    render(desktop,'apps/electron/assets/icons/icon.png',1024)
    render(rounded,'apps/electron/src/renderer/assets/app-icon.png',512)
    ico=ROOT/'apps/electron/assets/icons/icon.ico'
    with Image.open(ROOT/'apps/electron/assets/icons/icon.png') as im:
        im.save(ico,format='ICO',sizes=[(s,s) for s in (16,24,32,48,64,128,256)])
    outputs.append(ico)
    icns=ROOT/'apps/electron/assets/icons/icon.icns'
    with Image.open(ROOT/'apps/electron/assets/icons/icon.png') as im:
        im.save(icns,format='ICNS')
    outputs.append(icns)
    catalog=ROOT/'apps/ios/native/Resources/Assets.xcassets/AppIcon.appiconset'
    for item in json.loads((catalog/'Contents.json').read_text())['images']:
        size=round(float(item['size'].split('x')[0])*float(item['scale'][:-1]))
        render(master,(catalog/item['filename']).relative_to(ROOT),size,True)
    res=Path('apps/android/native/app/src/main/res')
    for density,scale in [('mdpi',1),('hdpi',1.5),('xhdpi',2),('xxhdpi',3),('xxxhdpi',4)]:
        for name,source,size in [('ic_launcher',rounded,48),('ic_launcher_round',circle,48),('ic_launcher_foreground',adaptive,108)]:
            render(source,res/f'mipmap-{density}'/f'{name}.png',round(size*scale))
    copy(SOURCE/'svg/favicon.svg','apps/web/public/favicon.svg')
    for name in ('lockup-bilingual-ink.svg','lockup-bilingual-white.svg'):
        copy(SOURCE/'svg'/name,'apps/web/public/brand/'+name)
    for product in ('harness', 'sdk'):
        source = SOURCE / 'products' / product / 'small.svg'
        sources.append(source)
        copy(source, f'apps/web/public/brand/{product}.svg')
    render(master,'apps/web/public/apple-touch-icon.png',180,True)
    MANIFEST.write_text(json.dumps({'version':1,'sources':{str(p.relative_to(ROOT)):digest(p) for p in sources},
                                  'outputs':{str(p.relative_to(ROOT)):digest(p) for p in outputs}},indent=2)+'\n')
    print(f'Generated {len(outputs)} platform brand assets')

if __name__=='__main__':
    main()
