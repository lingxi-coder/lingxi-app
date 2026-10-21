#!/usr/bin/env python3
"""Generate public model catalog seed from the SDK's provider definitions."""
import argparse
import hashlib
import json
import subprocess
import tomllib
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[4] / 'llm-client')
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
source = args.source.resolve()
providers, models = [], []
digest = hashlib.sha256()
for path in sorted((source / 'data/providers').glob('*.toml')):
    raw = path.read_bytes()
    source_text = raw.decode('utf-8')
    digest.update(path.name.encode() + b'\0' + raw + b'\0')
    data = tomllib.loads(source_text)
    url = data.get('base_url', '')
    if not url.startswith('https://'):
        continue
    identity = path.stem
    providers.append(dict(id=identity, name=data.get('display_name', identity).strip(),
        description=data.get('description', '')[:4000], baseUrl=url, protocol=data.get('protocol', 'open_ai_chat'),
        enabled=True, order=len(providers), regions=data.get('regions', []),
        executionDefinition=dict(format='llm-client-preset-toml', source=source_text,
            sha256=hashlib.sha256(raw).hexdigest())))
    for entry in data.get('model', []):
        original = entry.get('pricing', {})
        price = None
        if 'input_per_million' in original and 'output_per_million' in original:
            price = dict(currency=original.get('currency', 'USD'), inputPerMillion=original['input_per_million'], outputPerMillion=original['output_per_million'])
            for old, new in [('cache_read_per_million', 'cacheReadPerMillion'), ('cache_write_per_million', 'cacheWritePerMillion')]:
                if old in original:
                    price[new] = original[old]
        context = entry.get('context_window', 128000)
        models.append(dict(providerId=identity, id=entry['id'].strip(), name=entry.get('name', entry['id']).strip(),
            description=entry.get('description', '')[:8000], enabled=not bool(entry.get('deprecated', False)),
            order=len(models), platforms=['desktop', 'android', 'ios', 'cli'], minAppVersion='0.0.0', maxAppVersion=None,
            contextWindow=context, maxOutputTokens=min(entry.get('max_output', 8192), context),
            capabilities=dict(tools=entry.get('tool_call', False), vision='image' in entry.get('input_modalities', []),
                reasoning=entry.get('reasoning', False), audio='audio' in entry.get('input_modalities', []) or 'audio' in entry.get('output_modalities', [])),
            inputModalities=entry.get('input_modalities', ['text']), outputModalities=entry.get('output_modalities', ['text']), pricing=price))

output = Path(__file__).resolve().parents[1] / 'src'
content = json.dumps(dict(schemaVersion=1, providers=providers, models=models), ensure_ascii=False, indent=2) + '\n'
revision = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
dirty = bool(subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain', '--', 'data/providers'], text=True))
provenance = json.dumps(dict(source='llm-client/data/providers', revision=revision, workingTreeChanges=dirty,
    sourceSha256=digest.hexdigest(), seedSha256=hashlib.sha256(content.encode()).hexdigest(), providers=len(providers), models=len(models)), indent=2) + '\n'
for name, value in [('seed.json', content), ('seed-provenance.json', provenance)]:
    path = output / name
    if args.check:
        if not path.exists() or path.read_text() != value:
            raise SystemExit(f'{name} is stale; regenerate from the intended SDK checkout')
    else:
        path.write_text(value)
print(f'Catalog seed: {len(providers)} providers, {len(models)} models; source {revision[:12]}')
