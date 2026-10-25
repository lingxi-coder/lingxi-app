#!/usr/bin/env python3
"""Build LingXi identity assets. Requires fontTools and local licensed fonts.

This creates design assets only; it does not edit application configuration.
SVG exports contain paths, not font dependencies. No font files are distributed.
"""
from pathlib import Path
import argparse
import html
import json
import subprocess
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen

ROOT = Path(__file__).resolve().parent
BLUE, INK, PAPER = '#4167ED', '#202632', '#F6F5F0'
LOOP = 'M24 64C24 38 43 31 64 64C85 97 104 90 104 64C104 38 85 31 64 64C43 97 24 90 24 64Z'
PRODUCTS = [
    dict(id='code', en='LingXi Code', zh='灵犀 Code', launcher_en='LingXi', launcher_zh='灵犀', descriptor='Code', color=BLUE, role='product', purpose='把想法变成作品', source='lingxi-app'),
    dict(id='chat', en='LingXi Chat', zh='灵犀聊天', launcher_en='LingXi Chat', launcher_zh='灵犀聊天', descriptor='Chat', color='#137C70', role='product', purpose='让交流自然发生', source='chat'),
    dict(id='computer-use', en='LingXi Control', zh='灵犀操控', launcher_en='LingXi Control', launcher_zh='灵犀操控', descriptor='Control', color='#6653B9', role='product', purpose='让意图化为行动', source='computer-use'),
    dict(id='apps', en='LingXi Apps', zh='灵犀应用', launcher_en='LingXi Apps', launcher_zh='灵犀应用', descriptor='Apps', color='#AF5B2F', role='product', purpose='发现触手可及的工具', source='local-app-market'),
    dict(id='remote', en='LingXi Remote', zh='灵犀远控', launcher_en=None, launcher_zh=None, descriptor='Remote', color='#1F728F', role='feature', purpose='随时连接你的设备', source='lingxi-app / Remote Control'),
    dict(id='harness', en='LingXi Harness', zh='灵犀 Harness', launcher_en=None, launcher_zh=None, descriptor='Harness', color='#4F5B69', role='developer', purpose='代理运行时', source='harness-runtime'),
    dict(id='sdk', en='LingXi SDK', zh='灵犀 SDK', launcher_en=None, launcher_zh=None, descriptor='SDK', color='#5B6553', role='developer', purpose='构建与接入', source='SDK family'),
]
GLYPHS = {
    'code': '<path d="m58 94-6 6 6 6m12-12 6 6-6 6"/>',
    'chat': '<path d="M55 92h18a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3H62l-8 5v-8a3 3 0 0 1-2-3v-6a3 3 0 0 1 3-3Z"/>',
    'computer-use': '<path d="m57 91 17 12-9 1-4 8-4-21Z"/>',
    'apps': '<rect x="53" y="91" width="8" height="8" rx="1.6"/><rect x="67" y="91" width="8" height="8" rx="1.6"/><rect x="53" y="105" width="8" height="8" rx="1.6"/><rect x="67" y="105" width="8" height="8" rx="1.6"/>',
    'remote': '<path d="M52 96h23m-5-5 5 5-5 5m6 5H53m5-5-5 5 5 5"/>',
    'harness': '<path d="m52 96 12-6 12 6-12 6-12-6Zm0 7 12 6 12-6m-24 7 12 6 12-6"/>',
    'sdk': '<path d="M58 92h-5v20h5m12-20h5v20h-5m-4-18-4 16"/>',
}

def svg(body, width=128, height=128, title='LingXi'):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{html.escape(title, quote=True)}"><title>{html.escape(title)}</title>{body}</svg>\n'

def mark(color=BLUE):
    return f'<path d="{LOOP}" transform="rotate(-35 64 64)" fill="none" stroke="{color}" stroke-width="14" stroke-linecap="round" stroke-linejoin="round"/>'

def icon(color, product=None, rounded=True, small=False):
    body = f'<rect width="128" height="128" rx="{28 if rounded else 0}" fill="{color}"/>'
    if not product or small:
        body += '<g transform="translate(8 8) scale(.875)">' + mark('white') + '</g>'
    else:
        body += '<g transform="translate(20 5) scale(.6875)">' + mark('white') + '</g>'
        body += f'<g fill="none" stroke="white" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round">{GLYPHS[product]}</g>'
    return body

def outline(text, font, size, x=0, baseline=0, spacing=0, color=INK):
    glyphs, cmap = font.getGlyphSet(), font.getBestCmap()
    scale, advance, output = size / font['head'].unitsPerEm, 0, []
    for char in text:
        if ord(char) not in cmap:
            raise ValueError(f'Missing glyph: {char!r}')
        glyph = glyphs[cmap[ord(char)]]
        pen = SVGPathPen(glyphs)
        glyph.draw(pen)
        if pen.getCommands():
            output.append(f'<path d="{pen.getCommands()}" transform="translate({x+advance:.3f} {baseline}) scale({scale:.7f} {-scale:.7f})"/>')
        advance += glyph.width * scale + spacing
    return f'<g fill="{color}">'+''.join(output)+'</g>', advance-spacing

def write(name, data):
    path = ROOT / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(data, encoding='utf-8')

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--latin-font', default='/System/Library/Fonts/Avenir Next.ttc')
    parser.add_argument('--cjk-font', default='/System/Library/Fonts/Hiragino Sans GB.ttc')
    parser.add_argument('--png', action='store_true', help='Export PNGs with rsvg-convert')
    args = parser.parse_args()
    latin = TTFont(args.latin_font, fontNumber=2)
    medium = TTFont(args.latin_font, fontNumber=5)
    cjk = TTFont(args.cjk_font, fontNumber=2)
    manifest = {
        'version':'1.0.0', 'status':'design-proposal', 'name':'灵犀结 / Resonance Link',
        'brand': {'en':'LingXi', 'zh-Hans':'灵犀', 'zh-Hant':'靈犀', 'bilingual':'灵犀 LingXi'},
        'colors':{'primary':BLUE,'ink':INK,'paper':PAPER,'white':'#FFFFFF'},
        'geometry': {'viewBox':[0,0,128,128], 'path':LOOP,'rotation':-35,'stroke':14},
        'products':PRODUCTS,
        'developer_labels':[
            {'en':'LingXi WebRTC','zh':'灵犀 WebRTC','family':'sdk','icon':'sdk'},
            {'en':'LingXi Mobile Linux','zh':'灵犀 Mobile Linux','family':'sdk','icon':'sdk'},
            {'en':'LingXi LLM Client','zh':'灵犀 LLM Client','family':'sdk','icon':'sdk'}
        ],
        'rules': {'wordmark_min_width':96, 'mark_min_size':16,'clear_space_mark_units':20,'product_glyph_min_size':48,'small_icon_requires_visible_product_label':True,'application_identifiers_changed':False}
    }
    write('brand.json',json.dumps(manifest,ensure_ascii=False,indent=2)+'\n')
    write('brand-data.js','window.LINGXI_BRAND = '+json.dumps(manifest,ensure_ascii=False,indent=2)+';\n')
    css = ':root {\n  --lingxi-blue: '+BLUE+';\n  --lingxi-ink: '+INK+';\n  --lingxi-paper: '+PAPER+';\n'
    css += ''.join(f"  --lingxi-{p['id']}: {p['color']};\n" for p in PRODUCTS) + '}\n'
    write('tokens.css',css)
    for name,color in [('blue',BLUE),('ink',INK),('white','#FFFFFF')]:
        write(f'svg/mark-{name}.svg',svg(mark(color)))
    write('svg/favicon.svg',svg(icon(BLUE)))
    write('svg/app-master.svg',svg(icon(BLUE,rounded=False)))
    write('svg/app-preview.svg',svg(icon(BLUE)))
    write('svg/maskable-icon.svg',svg(f'<rect width="128" height="128" fill="{BLUE}"/><g transform="translate(26 26) scale(.59375)">{mark("white")}</g>'))
    write('svg/adaptive-foreground.svg',svg('<g transform="translate(26 26) scale(.59375)">'+mark('white')+'</g>'))
    for color_name,color in [('ink',INK),('white','#FFFFFF')]:
        en, width = outline('LingXi',latin,100,x=147,baseline=104,spacing=-2,color=color)
        en_width = int(147+width+14)
        word,ww = outline('LingXi',latin,100,x=7,baseline=104,spacing=-2,color=color)
        write(f'svg/wordmark-en-{color_name}.svg',svg(word,int(ww+21),142,'LingXi'))
        write(f'svg/lockup-en-{color_name}.svg',svg('<g transform="translate(0 7)">'+mark(color if color_name=='white' else BLUE)+'</g>'+en,en_width,142,'LingXi'))
        zh, zw = outline('灵犀',cjk,88,x=153,baseline=106,spacing=8,color=color)
        write(f'svg/lockup-zh-{color_name}.svg',svg('<g transform="translate(0 7)">'+mark(color if color_name=='white' else BLUE)+'</g>'+zh,int(153+zw+14),142,'灵犀'))
        mixed = en + f'<path d="M{en_width+16} 41v61" stroke="{color}" stroke-opacity=".25" stroke-width="1.5"/>'
        z2,z2w = outline('灵犀',cjk,56,x=en_width+44,baseline=96,spacing=4,color=color)
        write(f'svg/lockup-bilingual-{color_name}.svg',svg('<g transform="translate(0 7)">'+mark(color if color_name=='white' else BLUE)+'</g>'+mixed+z2,int(en_width+44+z2w+16),142,'灵犀 LingXi'))
    for product in PRODUCTS:
        pid, color = product['id'], product['color']
        for suffix,rounded,small in [('icon',True,False),('app-master',False,False),('small',True,True)]:
            write(f'products/{pid}/{suffix}.svg',svg(icon(color,pid,rounded,small),title=product['en']))
        for variant,fg in [('light',INK),('dark','#FFFFFF')]:
            brand,bw=outline('LingXi',latin,60,x=128,baseline=74,spacing=-1,color=fg)
            desc,dw=outline(product['descriptor'],medium,24,x=131,baseline=110,spacing=.2,color=fg)
            body='<g transform="translate(0 0) scale(.86)">'+mark(color if variant=='light' else '#FFFFFF')+'</g>'+brand+desc
            write(f'products/{pid}/lockup-{variant}.svg',svg(body,int(max(128+bw,131+dw)+20),128,product['en']))
    if args.png:
        png_sources = [('svg/app-master.svg','png/lingxi-app-1024.png',1024),('svg/app-preview.svg','png/lingxi-preview-512.png',512),('svg/maskable-icon.svg','png/lingxi-maskable-512.png',512),('svg/mark-blue.svg','png/lingxi-mark-1024.png',1024),('svg/lockup-bilingual-ink.svg','png/lingxi-label.png',1600)]
        for size in [16,32,48,64,128,256,512]:
            png_sources.append(('svg/favicon.svg',f'png/lingxi-{size}.png',size))
        for p in PRODUCTS:
            for size in [16,32,48,64,128,256,512,1024]:
                source='app-master' if size==1024 else ('small' if size<48 else 'icon')
                png_sources.append((f"products/{p['id']}/{source}.svg",f"products/{p['id']}/icon-{size}.png",size))
        for source,target,size in png_sources:
            output=ROOT/target
            output.parent.mkdir(parents=True,exist_ok=True)
            subprocess.run(['rsvg-convert','--width',str(size),'--output',str(output),str(ROOT/source)],check=True)
    print(f'Built identity system at {ROOT}')

if __name__=='__main__':
    main()
