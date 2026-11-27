#!/bin/sh
set -eu
umask 077
work="$(mktemp -d)"
export NODE_EXTRA_CA_CERTS="$work/core.crt"
export LINGXI_DATA_CORE_TLS_KEY="$work/core.key"
export LINGXI_DATA_ACTOR_FILE="$work/actors.json"
export LINGXI_DATA_INTERNAL_ORIGIN="https://127.0.0.1:8787"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -noenc -days 3650 \
  -keyout "$LINGXI_DATA_CORE_TLS_KEY" -out "$NODE_EXTRA_CA_CERTS" -subj '/CN=127.0.0.1' \
  -addext 'subjectAltName=IP:127.0.0.1' -addext 'basicConstraints=critical,CA:FALSE' 2>/dev/null
exec node scripts/serve-lingxi-data.mjs
