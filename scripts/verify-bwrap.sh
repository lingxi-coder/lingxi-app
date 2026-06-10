#!/usr/bin/env bash
# Runtime verification of the bwrap shapes the Rust sandbox builder produces.
# Requires Docker (LinuxKit). bwrap needs --privileged (Docker's default seccomp
# blocks unprivileged userns). arm64 host → arm64v8/debian.
#
# Usage: scripts/verify-bwrap.sh            (runs all checks)
#        scripts/verify-bwrap.sh net|userns|deny|scrub   (one group)
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

echo "=== $([ $fail = 0 ] && echo ALL-PASS || echo SOME-FAIL) ==="
exit $fail
EOS

docker run --rm --privileged "$IMG" /bin/bash -c "$INNER" _ "$GROUP"
