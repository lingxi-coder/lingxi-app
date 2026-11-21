#!/bin/bash
set -euo pipefail
umask 077
meta=http://metadata.google.internal/computeMetadata/v1/instance
attribute() { curl -fsS -H 'Metadata-Flavor: Google' "$meta/attributes/$1"; }
emit() { curl -fsS -H 'Metadata-Flavor: Google' -X PUT --data-binary "$2" "$meta/guest-attributes/render/$1" >/dev/null; }
trap 'emit failure "regional browser failed at line $LINENO"; if [[ -s /tmp/reader/wire.log ]]; then tail -c 1400 /tmp/reader/wire.log | curl -fsS -H "Metadata-Flavor: Google" -X PUT --data-binary @- "$meta/guest-attributes/render/diagnostic" >/dev/null; fi' ERR
export DEBIAN_FRONTEND=noninteractive
emit phase dependencies
apt-get -o Acquire::Retries=3 -qq update
apt-get -o Acquire::Retries=3 -qy install --no-install-recommends chromium chromium-driver python3-selenium nftables libnss3-tools fonts-noto-cjk >/dev/null
install -d -m 0755 /opt/lingxi-wire
cd /opt/lingxi-wire
attribute fixture | base64 -d >fixture.tgz
printf '%s  fixture.tgz\n' "$(attribute fixture-sha256)" | sha256sum --check -
tar --no-same-owner -xzf fixture.tgz
chmod 0444 wire_client.py wire_worker.py
install -d -m 0700 -o nobody -g nogroup /tmp/reader /tmp/reader/.pki /tmp/reader/.pki/nssdb
for attempt in {1..150}; do
  if attribute worker-ca >/tmp/reader/ca.crt 2>/dev/null; then break; fi
  sleep 3
done
openssl x509 -in /tmp/reader/ca.crt -noout
chmod 0444 /tmp/reader/ca.crt
runuser -u nobody -- certutil -N -d sql:/tmp/reader/.pki/nssdb --empty-password
runuser -u nobody -- certutil -A -d sql:/tmp/reader/.pki/nssdb -n 'Synthetic staging renderer' -t 'C,,' -i /tmp/reader/ca.crt
peer=$(attribute worker-ip)
python3 -c 'import ipaddress,sys; assert ipaddress.IPv4Address(sys.argv[1]).is_private' "$peer"
# Two match-only counters measure this TCP connection without retaining network payloads.
nft add table inet dataset_wire
nft 'add chain inet dataset_wire inbound { type filter hook input priority 0; policy accept; }'
nft 'add chain inet dataset_wire outbound { type filter hook output priority 0; policy accept; }'
nft add rule inet dataset_wire inbound ip saddr "$peer" tcp sport 18444 counter
nft add rule inet dataset_wire outbound ip daddr "$peer" tcp dport 18444 counter
emit phase browser
runuser -u nobody -- env HOME=/tmp/reader PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 wire_client.py >/tmp/reader/report.json 2>/tmp/reader/wire.log
nft -j list table inet dataset_wire >/tmp/reader/counters.json
python3 - <<'PY'
import json
from pathlib import Path
d=Path('/tmp/reader')
r=json.loads((d/'report.json').read_text())
r['ipTransport']={}
for item in json.loads((d/'counters.json').read_text())['nftables']:
    if 'rule' in item:
        rule=item['rule']
        counters=[e['counter'] for e in rule['expr'] if 'counter' in e]
        assert len(counters)==1
        r['ipTransport'][rule['chain']]=counters[0]
assert r['ipTransport']['inbound']['bytes']>=r['pngPayloadBytes'] and r['ipTransport']['outbound']['packets']>0
(d/'report.json').write_text(json.dumps(r))
PY
emit result "@/tmp/reader/report.json"
