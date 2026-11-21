#!/bin/bash
set -euo pipefail
umask 077
meta=http://metadata.google.internal/computeMetadata/v1/instance
work=/var/lib/lingxi-render-probe
install -d -m 0700 "$work"
cd "$work"
exec >>bootstrap.log 2>&1
emit() { curl -fsS -H 'Metadata-Flavor: Google' -X PUT --data-binary "$2" "$meta/guest-attributes/render/$1" >/dev/null; }
attribute() { curl -fsS -H 'Metadata-Flavor: Google' "$meta/attributes/$1"; }
phase=dependencies
fail() {
  local log=bootstrap.log
  if [[ -s probe.log ]]; then log=probe.log; fi
  tail -c 1800 "$log" | curl -fsS -H 'Metadata-Flavor: Google' -X PUT --data-binary @- "$meta/guest-attributes/render/diagnostic" >/dev/null
  if [[ -f measurements.json ]]; then emit partial "@measurements.json"; fi
  emit failure "$phase at line $1"
}
trap 'fail "$LINENO"' ERR
export DEBIAN_FRONTEND=noninteractive
emit phase "$phase"
apt-get -o Acquire::Retries=3 update -qq
apt-get -o Acquire::Retries=3 install -y -qq --no-install-recommends docker.io ca-certificates curl gnupg
curl -fsSL --retry 3 https://gvisor.dev/archive.key -o archive.key
gpg --batch --dearmor --output /usr/share/keyrings/lingxi-gvisor.gpg archive.key
printf '%s\n' 'deb [arch=amd64 signed-by=/usr/share/keyrings/lingxi-gvisor.gpg] https://storage.googleapis.com/gvisor/releases release main' >/etc/apt/sources.list.d/lingxi-gvisor.list
chmod 0644 /usr/share/keyrings/lingxi-gvisor.gpg /etc/apt/sources.list.d/lingxi-gvisor.list
apt-get -o Acquire::Retries=3 update -qq
apt-get -o Acquire::Retries=3 install -y -qq runsc
python3 - <<'PY'
import json,shutil
from pathlib import Path
p=Path('/etc/docker/daemon.json')
conf=json.loads(p.read_text()) if p.exists() else {}
conf.setdefault('runtimes',{})['lingxi-closed']={'path':shutil.which('runsc'),'runtimeArgs':['--platform=systrap','--network=none']}
p.write_text(json.dumps(conf))
PY
systemctl restart docker
phase=browser-image
emit phase "$phase"
attribute fixture | base64 -d >fixture.tgz
digest=$(attribute fixture-sha256)
printf '%s  fixture.tgz\n' "$digest" | sha256sum --check -
tar --no-same-owner -xzf fixture.tgz
docker build -t lingxi-render-probe:fixture . >probe.log 2>&1
phase=browser-and-observers
emit phase "$phase"
install -d -m 0700 tls
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=LingXi synthetic renderer test CA' -addext 'basicConstraints=critical,CA:TRUE' -keyout tls/ca.key -out tls/ca.crt
openssl req -new -newkey rsa:2048 -nodes -subj '/CN=observer.fixture.test' -keyout tls/server.key -out tls/server.csr
printf '%s\n' 'basicConstraints=CA:FALSE' 'extendedKeyUsage=serverAuth' 'subjectAltName=IP:172.29.76.2,DNS:*.fixture.test' >tls/server.ext
openssl x509 -req -in tls/server.csr -CA tls/ca.crt -CAkey tls/ca.key -CAcreateserial -days 1 -extfile tls/server.ext -out tls/server.crt
openssl verify -CAfile tls/ca.crt tls/server.crt
chmod 0644 tls/ca.crt tls/server.crt
case "$(attribute workload)" in
  isolation) timeout 360 python3 observe.py >probe.log 2>&1 ;;
  benchmark) timeout 1050 python3 observe.py benchmark >probe.log 2>&1 ;;
  wire) timeout 1150 python3 observe.py wire >probe.log 2>&1 ;;
  *) exit 1 ;;
esac
emit result "@result.json"
