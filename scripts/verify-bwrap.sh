#!/usr/bin/env bash
# Runtime verification of the bwrap shapes the Rust sandbox builder produces.
# Requires Docker (LinuxKit). bwrap needs --privileged (Docker's default seccomp
# blocks unprivileged userns). arm64 host → arm64v8/debian.
#
# Usage: scripts/verify-bwrap.sh            (runs all checks)
#        scripts/verify-bwrap.sh net|userns|deny|scrub|netbridge   (one group)
#
# The `netbridge` group proves the socat/bwrap/env PLUMBING that
# wrap_command_with_sandbox_linux emits: a host stand-in HTTP CONNECT proxy +
# host socat UNIX-LISTEN->TCP:proxy + a bwrap child (--unshare-net, sock bound,
# sandbox-side socat TCP-LISTEN:3128->UNIX-CONNECT, HTTP_PROXY=localhost:3128).
# The in-container python CONNECT proxy is a STAND-IN for the Rust hyper proxy
# (P3b, separately tested) that only allows one host — the e2e proves the
# socat/bwrap/env bridge, NOT the proxy's domain-filtering logic.
set -euo pipefail
GROUP="${1:-all}"
IMG="arm64v8/debian:stable-slim"

if ! docker info >/dev/null 2>&1; then
  echo "FATAL: Docker daemon not running (open -a Docker)"; exit 2
fi

# The in-container script. Installs bubblewrap then runs the requested checks.
read -r -d '' INNER <<'EOS' || true
set -u
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq bubblewrap >/dev/null 2>&1
GROUP="$1"
fail=0
ok()   { echo "PASS: $1"; }
bad()  { echo "FAIL: $1"; fail=1; }

if [ "$GROUP" = net ] || [ "$GROUP" = all ]; then
  # LoopbackOnly/Disabled shape == --unshare-net : external blocked, loopback up.
  if bwrap --unshare-user-try --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'getent hosts example.com >/dev/null 2>&1' ; then
    bad "unshare-net leaked external DNS"
  else ok "unshare-net blocks external"; fi
  # Read the kernel's per-netns interface list directly (no iproute2 in the
  # slim image): a fresh netns from --unshare-net always exposes `lo` here.
  if bwrap --unshare-user-try --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'grep -qw "lo:" /proc/net/dev && echo lo-ok' | grep -q lo-ok ; then
    ok "unshare-net keeps loopback ns"
  else bad "unshare-net loopback missing"; fi
  # Allowed shape == --share-net : external resolvable (host has net).
  if bwrap --unshare-user-try --share-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'getent hosts example.com >/dev/null 2>&1' ; then
    ok "share-net allows external"
  else echo "WARN: share-net external unresolved (host offline?) — not a sandbox failure"; fi
fi

if [ "$GROUP" = userns ] || [ "$GROUP" = all ]; then
  if bwrap --unshare-user-try --unshare-pid --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'echo up' | grep -q up ; then ok "unshare-user-try starts"; else bad "unshare-user-try start"; fi
  sysctl -w user.max_user_namespaces=0 >/dev/null 2>&1 || true
  if bwrap --unshare-user-try --unshare-pid --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'echo deg' | grep -q deg ; then ok "unshare-user-try degrades (userns=0)"; else bad "degrade"; fi
  sysctl -w user.max_user_namespaces=15000 >/dev/null 2>&1 || true
fi

if [ "$GROUP" = deny ] || [ "$GROUP" = all ]; then
  mkdir -p /w && echo orig > /w/HEAD
  out=$(bwrap --unshare-user-try --ro-bind / / --bind /w /w --ro-bind /w/HEAD /w/HEAD --proc /proc --dev /dev -- \
       /bin/sh -c 'echo x > /w/HEAD 2>/dev/null && echo WROTE || echo DENIED')
  [ "$out" = DENIED ] && ok "ro-bind-in-place denies write (overrides --bind parent)" || bad "ro-bind-in-place write $out"
  [ "$(cat /w/HEAD)" = orig ] && ok "host file unchanged" || bad "host file mutated"
fi

if [ "$GROUP" = scrub ] || [ "$GROUP" = all ]; then
  mkdir -p /s && cd /s
  # The wrapped-string shape: inner plants HEAD in host cwd; suffix scrubs it, preserves rc.
  bash -c 'bwrap --unshare-user-try --ro-bind / / --bind /s /s --proc /proc --dev /dev -- \
            /bin/sh -c "echo planted > /s/HEAD; exit 7"
           rc=$?; rm -rf -- /s/HEAD 2>/dev/null; exit $rc' ; rc=$?
  [ "$rc" = 7 ] && ok "scrub suffix preserves inner exit code" || bad "exit code $rc != 7"
  [ ! -e /s/HEAD ] && ok "planted bare-repo file scrubbed" || bad "HEAD not scrubbed"
  # Pre-existing file must NOT be scrubbed (it would be in ro_bind_in_place, never the scrub list).
  echo keep > /s/config
  bash -c 'true; rc=$?; rm -rf -- /s/HEAD 2>/dev/null; exit $rc' >/dev/null 2>&1 || true
  [ -e /s/config ] && ok "pre-existing file not in scrub list (untouched)" || bad "config wrongly scrubbed"
fi

if [ "$GROUP" = netbridge ] || [ "$GROUP" = all ]; then
  # Extra deps for the bridge e2e (curl + socat + a python CONNECT proxy + CA certs).
  apt-get install -y -qq socat curl python3 ca-certificates >/dev/null 2>&1
  ALLOWED=example.com
  SOCK=/tmp/claude-http-e2e.sock
  rm -f "$SOCK"

  # --- Stand-in HTTP CONNECT proxy (allows exactly one host). ---
  # This is a STAND-IN for the Rust hyper proxy (P3b). It proves the socat/bwrap
  # bridge plumbing, not the proxy's filtering logic.
  cat > /tmp/connproxy.py <<'PY'
import socket, threading, sys, select
ALLOWED = sys.argv[1] if len(sys.argv) > 1 else "example.com"
def pipe(a, b):
    try:
        while True:
            r,_,_ = select.select([a,b],[],[])
            for s in r:
                d = s.recv(65536)
                if not d: return
                (b if s is a else a).sendall(d)
    except OSError:
        pass
def handle(c):
    try:
        req = b""
        while b"\r\n\r\n" not in req:
            d = c.recv(4096)
            if not d: return
            req += d
        line = req.split(b"\r\n",1)[0].decode("latin1")
        parts = line.split()
        if len(parts) < 2 or parts[0] != "CONNECT":
            c.sendall(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n"); return
        hostport = parts[1]
        host = hostport.rsplit(":",1)[0]
        port = int(hostport.rsplit(":",1)[1]) if ":" in hostport else 443
        if host != ALLOWED:
            c.sendall(b"HTTP/1.1 403 Forbidden\r\n\r\n"); return
        try:
            u = socket.create_connection((host, port), timeout=10)
        except OSError:
            c.sendall(b"HTTP/1.1 502 Bad Gateway\r\n\r\n"); return
        c.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        pipe(c, u); u.close()
    finally:
        c.close()
srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 8888)); srv.listen(64)
print("PROXY_UP", flush=True)
while True:
    c,_ = srv.accept()
    threading.Thread(target=handle, args=(c,), daemon=True).start()
PY
  python3 /tmp/connproxy.py "$ALLOWED" >/tmp/proxy.log 2>&1 &
  PROXY_PID=$!
  # Wait for PROXY_UP.
  for _ in $(seq 1 50); do grep -q PROXY_UP /tmp/proxy.log 2>/dev/null && break; sleep 0.1; done

  # --- Host socat: UNIX-LISTEN <sock> -> TCP:localhost:8888 (the proxy). ---
  # EXACT shape of initialize_linux_network_bridge's host-side socat.
  socat "UNIX-LISTEN:$SOCK,fork,reuseaddr" \
        "TCP:localhost:8888,keepalive,keepidle=10,keepintvl=5,keepcnt=3" \
        >/dev/null 2>&1 &
  SOCAT_PID=$!
  for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
  if [ -S "$SOCK" ]; then ok "host socat UNIX-LISTEN socket created"; else bad "host socat socket missing"; fi

  # --- bwrap child: EXACT shape wrap_command_with_sandbox_linux emits. ---
  # --unshare-net + --bind <sock> <sock> + sandbox-side socat TCP-LISTEN:3128
  # -> UNIX-CONNECT + HTTP_PROXY=http://localhost:3128. curl the ALLOWED host
  # THROUGH the bridge (-> NET_OK).
  SANDBOX_INNER='socat TCP-LISTEN:3128,fork,reuseaddr UNIX-CONNECT:'"$SOCK"' >/dev/null 2>&1 &
trap "kill %1 2>/dev/null; exit" EXIT
sleep 0.5
curl -sS --max-time 20 -o /dev/null https://'"$ALLOWED"'/ && echo NET_OK || echo NET_FAIL'
  OUT=$(bwrap --unshare-user-try --unshare-net \
          --bind "$SOCK" "$SOCK" \
          --ro-bind / / --proc /proc --dev /dev --unshare-pid \
          --setenv HTTP_PROXY http://localhost:3128 \
          --setenv HTTPS_PROXY http://localhost:3128 \
          -- /bin/bash -c "$SANDBOX_INNER" 2>/dev/null || true)
  echo "[netbridge] bridged curl -> $OUT"
  case "$OUT" in
    *NET_OK*) ok "allowed host reachable THROUGH the socat bridge (NET_OK)";;
    *)        bad "allowed host NOT reachable through bridge: $OUT";;
  esac

  # --- DIRECT curl in a fresh netns (no proxy) must be blocked. ---
  DOUT=$(bwrap --unshare-user-try --unshare-net \
           --ro-bind / / --proc /proc --dev /dev --unshare-pid \
           -- /bin/bash -c \
           'curl -sS --noproxy "*" --max-time 8 -o /dev/null https://'"$ALLOWED"'/ && echo NET_LEAK || echo NET_BLOCKED' \
           2>/dev/null || true)
  echo "[netbridge] direct curl -> $DOUT"
  case "$DOUT" in
    *NET_BLOCKED*) ok "direct egress blocked in fresh netns (NET_BLOCKED)";;
    *)             bad "direct egress NOT blocked: $DOUT";;
  esac

  kill "$SOCAT_PID" "$PROXY_PID" 2>/dev/null || true
fi

echo "=== $([ $fail = 0 ] && echo ALL-PASS || echo SOME-FAIL) ==="
exit $fail
EOS

docker run --rm --privileged "$IMG" /bin/bash -c "$INNER" _ "$GROUP"
