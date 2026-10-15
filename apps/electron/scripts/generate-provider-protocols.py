#!/usr/bin/env python3
"""Generate desktop format choices from the pinned SDK's committed catalog."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--sdk', type=Path, required=True)
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
product = Path(__file__).resolve().parents[3]
revision = re.search(r'lingxi-llm-client = .*rev = "([0-9a-f]+)"', (product / 'Cargo.toml').read_text())[1]
def git(*arguments):
    return subprocess.check_output(['git', '-C', str(args.sdk), *arguments], text=True)

profiles = []
for name in sorted(git('ls-tree', '--name-only', revision, 'data/providers/').splitlines()):
    if not name.endswith('.toml'):
        continue
    provider = tomllib.loads(git('show', f'{revision}:{name}'))
    connection = provider.get('connection', {})
    if connection.get('hidden'):
        continue
    profiles.append({
        'profile': Path(name).stem, 'provider': provider['provider_id'],
        'regions': provider.get('regions', []), 'native': provider['protocol'],
        'formats': list(dict.fromkeys([provider['protocol'], *connection.get('protocol_profiles', {})])),
    })
content = (
    f'// Generated from the pinned llm-client provider catalog ({revision[:12]}).\n'
    '// Change provider facts in llm-client, then regenerate this catalog.\n'
    'export const PROVIDER_PROTOCOLS = [\n'
    '  // Host-owned ChatGPT account transport; separate from the OpenAI API-key account.\n'
    '  { profile: "openai-chatgpt", provider: "openai-chatgpt", regions: ["international"], native: "open_ai_responses", formats: ["open_ai_responses"] },'
    + json.dumps(profiles, ensure_ascii=False, indent=2)[1:] + ';\n'
)
destination = product / 'apps/electron/src/shared/providerProtocols.ts'
if args.check:
    if destination.read_text() != content:
        raise SystemExit('Provider format catalog is stale; regenerate it with this script.')
else:
    destination.write_text(content)
print(f'Provider format catalog: {len(profiles)} SDK accounts, revision {revision[:12]}')
