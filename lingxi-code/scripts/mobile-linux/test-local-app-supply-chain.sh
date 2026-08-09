#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${SCRIPT_DIR}/verify-local-app-supply-chain.py"
TEMPLATE="${REPO_ROOT}/lingxi-code/local-apps/templates/next-static-v1"
TEMP_ROOT="$(mktemp -d)"
STAGED_OUTPUT="${REPO_ROOT}/clients/android/app/build/local-app-supply-chain-test-${RANDOM}"
trap 'chmod -R u+w "${TEMP_ROOT}" "${STAGED_OUTPUT}" 2>/dev/null || true; rm -rf "${TEMP_ROOT}" "${STAGED_OUTPUT}"' EXIT

python3 "${TOOL}" --repo-root "${REPO_ROOT}"

python3 - "${REPO_ROOT}/docs/mobile-linux/local-app-runtime-policy.json" <<'PY'
import json
import pathlib
import sys

policy = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
package_policy = policy["package_manager_policy"]
assert package_policy["npm_family_present"] is True
assert package_policy["npm_scope"] == "interactive_terminal_only"
assert package_policy["generation_jobs"] is False
assert package_policy["mcp"] is False
commands = policy["commands"]
assert commands["store_build"]["network_policy"] == "disabled"
assert commands["full_build"]["network_policy"] == "disabled"
assert commands["full_start"]["network_policy"] == "loopback_only"
assert all(command["memory_limit_bytes"] == 838860800 for command in commands.values())
PY

python3 "${SCRIPT_DIR}/generate-local-app-sbom.py" \
  --lock "${TEMPLATE}/package-lock.json" \
  --output "${TEMP_ROOT}/local-app-runtime.spdx.json"
cmp "${TEMP_ROOT}/local-app-runtime.spdx.json" \
  "${REPO_ROOT}/docs/mobile-linux/sbom/local-app-runtime.spdx.json"

if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --release; then
  echo "expected release validation to remain fail-closed" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/dependency-drift"
python3 - "${TEMP_ROOT}/dependency-drift/package.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text(encoding="utf-8"))
value["dependencies"]["next"] = "16.2.12"
path.write_text(json.dumps(value), encoding="utf-8")
PY
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/dependency-drift"; then
  echo "expected dependency drift to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/network-bypass"
printf '\nfetch("https://example.com");\n' >> "${TEMP_ROOT}/network-bypass/app/page.jsx"
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/network-bypass"; then
  echo "expected direct network access to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/symlink"
ln -s /tmp "${TEMP_ROOT}/symlink/public/escape"
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/symlink"; then
  echo "expected a template symlink to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/path-escape"
printf 'console.log("outside policy");\n' > "${TEMP_ROOT}/path-escape/server.js"
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/path-escape"; then
  echo "expected a source file outside writable roots to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/lock-drift"
printf '\n' >> "${TEMP_ROOT}/lock-drift/package-lock.json"
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/lock-drift"; then
  echo "expected package-lock byte drift to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/weak-csp"
python3 - "${TEMP_ROOT}/weak-csp/next.config.mjs" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(
    path.read_text(encoding="utf-8").replace("frame-ancestors 'none'", ""),
    encoding="utf-8",
)
PY
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/weak-csp"; then
  echo "expected a weakened server CSP to fail validation" >&2
  exit 1
fi

NODE_MODULES="${TEMP_ROOT}/node_modules"
mkdir -p \
  "${NODE_MODULES}/next/dist/bin" \
  "${NODE_MODULES}/react" \
  "${NODE_MODULES}/react-dom" \
  "${NODE_MODULES}/@next/swc-linux-arm64-musl" \
  "${NODE_MODULES}/@next/swc-linux-x64-musl"
python3 - "${NODE_MODULES}" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
packages = {
    "next": "16.2.11",
    "react": "19.2.8",
    "react-dom": "19.2.8",
    "@next/swc-linux-arm64-musl": "16.2.11",
    "@next/swc-linux-x64-musl": "16.2.11",
}
for name, version in packages.items():
    package = root.joinpath(*name.split("/"))
    (package / "package.json").write_text(
        json.dumps({"name": name, "version": version}),
        encoding="utf-8",
    )
(root / "next/dist/bin/next").write_text("#!/usr/bin/env node\n", encoding="utf-8")
(root / "@next/swc-linux-arm64-musl/next-swc.linux-arm64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "@next/swc-linux-x64-musl/next-swc.linux-x64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
PY
python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${STAGED_OUTPUT}" \
  --platform android \
  --variant play
test -f "${STAGED_OUTPUT}/runtime-manifest.json"
test ! -w "${STAGED_OUTPUT}/node_modules/next/package.json"
python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${STAGED_OUTPUT}" \
  --platform android \
  --variant play

# The iOS bundle install step. The staged tree is 0555/0444, so a second build
# cannot `rm -rf` the previous copy without first restoring write permission —
# and `cp -R` into a destination that still exists nests the new runtime inside
# the stale one, shipping build #1's node_modules. That removal-and-copy used to
# live inline in the Xcode build phase where nothing could run it; it is now
# clients/ios/scripts/install-staged-local-app-runtime.sh.
INSTALL_STAGED="${TEMP_ROOT}/install-src/store"
INSTALL_DEST="${TEMP_ROOT}/install-dest/local-app-runtime"
mkdir -p "${INSTALL_STAGED}/node_modules/next/dist/bin"
printf '{"schema_version":1}\n' > "${INSTALL_STAGED}/runtime-manifest.json"
printf '#!/usr/bin/env node\n' > "${INSTALL_STAGED}/node_modules/next/dist/bin/next"
# Mirror stage-local-app-runtime.py: files 0444, directories 0555, root 0755.
find "${INSTALL_STAGED}" -type f -exec chmod 0444 {} +
find "${INSTALL_STAGED}" -mindepth 1 -type d -exec chmod 0555 {} +
chmod 0755 "${INSTALL_STAGED}"

for _ in 1 2; do
  "${REPO_ROOT}/clients/ios/scripts/install-staged-local-app-runtime.sh" \
    --staged "${INSTALL_STAGED}" \
    --destination "${INSTALL_DEST}"
done
if [ -e "${INSTALL_DEST}/store" ] || [ -e "${INSTALL_DEST}/local-app-runtime" ]; then
  echo "second install nested the staged runtime inside the previous copy" >&2
  exit 1
fi
test -f "${INSTALL_DEST}/runtime-manifest.json"
test -f "${INSTALL_DEST}/node_modules/next/dist/bin/next"
if "${REPO_ROOT}/clients/ios/scripts/install-staged-local-app-runtime.sh" \
  --staged "${TEMP_ROOT}/install-src/absent" \
  --destination "${INSTALL_DEST}" 2>/dev/null; then
  echo "expected a missing staged runtime to fail the install" >&2
  exit 1
fi

ROOTFS_PINS="${TEMP_ROOT}/rootfs-pins"
mkdir -p "${ROOTFS_PINS}"
printf 'package-augmented rootfs fixture\n' > "${ROOTFS_PINS}/rootfs.tar.gz"
python3 - "${ROOTFS_PINS}" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
digest = hashlib.sha256((root / "rootfs.tar.gz").read_bytes()).hexdigest()
(root / "source-only-pins.json").write_text(
    json.dumps({"rootfs": {"archives": {"arm64-v8a": {"sha256": "e" * 64}}}}),
    encoding="utf-8",
)
(root / "wrong-pins.json").write_text(
    json.dumps({"rootfs": {"release_archives": {"arm64-v8a": {"sha256": "0" * 64}}}}),
    encoding="utf-8",
)
(root / "pins.json").write_text(
    json.dumps({"rootfs": {"release_archives": {"arm64-v8a": {"sha256": digest}}}}),
    encoding="utf-8",
)
PY
if python3 "${SCRIPT_DIR}/rootfs_tool.py" verify-release-archive \
  --pins "${ROOTFS_PINS}/source-only-pins.json" --abi arm64-v8a \
  --archive "${ROOTFS_PINS}/rootfs.tar.gz"; then
  echo "expected an uncommitted release rootfs digest to fail validation" >&2
  exit 1
fi
if python3 "${SCRIPT_DIR}/rootfs_tool.py" verify-release-archive \
  --pins "${ROOTFS_PINS}/wrong-pins.json" --abi arm64-v8a \
  --archive "${ROOTFS_PINS}/rootfs.tar.gz"; then
  echo "expected a release rootfs digest mismatch to fail validation" >&2
  exit 1
fi
python3 "${SCRIPT_DIR}/rootfs_tool.py" verify-release-archive \
  --pins "${ROOTFS_PINS}/pins.json" --abi arm64-v8a \
  --archive "${ROOTFS_PINS}/rootfs.tar.gz"

# KNOWN-GAP ANCHOR. Everything above drives SYNTHETIC pin fixtures, so it proves
# the comparator works, not that the shipped bytes are anchored to anything. They
# are not: docs/mobile-linux/mobile-linux-pins.json commits no
# `rootfs.release_archives` digest, because the package-augmented rootfs cannot be
# reproducibly built while docs/mobile-linux/local-app-runtime-pins.json reports
# closure_status "blocked" for both ABIs. See the "KNOWN UNANCHORED STEP" section
# of docs/mobile-linux/README.md.
#
# This block asserts the ABSENCE of that anchor, by its exact reason, for every
# ABI staging iterates over. It goes RED the moment a digest is committed — that
# is deliberate: replace it with a positive match assertion and update the README
# section named above in the same change.
COMMITTED_PINS="${REPO_ROOT}/docs/mobile-linux/mobile-linux-pins.json"
# The key must be absent, checked structurally. `verify-release-archive` reports
# a MALFORMED digest with the same "no committed release rootfs digest" message
# as an absent one, so the reason check below cannot tell the two apart on its
# own — and a botched pin must not read as an untouched gap.
COMMITTED_ABIS="$(python3 - "${COMMITTED_PINS}" <<'PY'
import json
import pathlib
import sys

rootfs = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["rootfs"]
if "release_archives" in rootfs:
    raise SystemExit(
        "known-gap anchor is stale: mobile-linux-pins.json now carries "
        "rootfs.release_archives. Replace the known-gap anchor in "
        "test-local-app-supply-chain.sh with a real match assertion and update "
        "the 'KNOWN UNANCHORED STEP' section of docs/mobile-linux/README.md."
    )
archives = rootfs["archives"]
if not archives:
    raise SystemExit("expected committed pins to list at least one rootfs ABI")
print(" ".join(sorted(archives)))
PY
)"
for abi in ${COMMITTED_ABIS}; do
  gap_reason="$(python3 "${SCRIPT_DIR}/rootfs_tool.py" verify-release-archive \
    --pins "${COMMITTED_PINS}" --abi "${abi}" \
    --archive "${ROOTFS_PINS}/rootfs.tar.gz" 2>&1 || true)"
  case "${gap_reason}" in
    "no committed release rootfs digest for ${abi}: "*) ;;
    *)
      echo "known-gap anchor is stale for ${abi}: ${gap_reason}" >&2
      echo "docs/mobile-linux/mobile-linux-pins.json appears to carry a release" >&2
      echo "digest now. Replace this block with a real match assertion and update" >&2
      echo "the 'KNOWN UNANCHORED STEP' section of docs/mobile-linux/README.md." >&2
      exit 1
      ;;
  esac
done

echo "local-app supply-chain tests passed (release rootfs digest: KNOWN GAP, unanchored)"
