#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${SCRIPT_DIR}/verify-local-app-supply-chain.py"
TEMPLATE="${REPO_ROOT}/lingxi-code/local-apps/templates/vite-react-static-v1"
TEMP_ROOT="$(mktemp -d)"
STAGED_OUTPUT="${REPO_ROOT}/clients/android/app/build/local-app-supply-chain-test-${RANDOM}"
IOS_STAGED_OUTPUT="${REPO_ROOT}/clients/ios/build/local-app-supply-chain-test-${RANDOM}"
trap 'chmod -R u+w "${TEMP_ROOT}" "${STAGED_OUTPUT}" "${IOS_STAGED_OUTPUT}" 2>/dev/null || true; rm -rf "${TEMP_ROOT}" "${STAGED_OUTPUT}" "${IOS_STAGED_OUTPUT}"' EXIT

python3 "${TOOL}" --repo-root "${REPO_ROOT}"

python3 - "${REPO_ROOT}/docs/mobile-linux/local-app-runtime-policy.json" <<'PY'
import json
import pathlib
import sys

policy = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert "next_executable" not in policy
assert "node_modules_mount" not in policy
assert "scaffold" not in policy
assert "package_manager_policy" not in policy
assert policy["vite_executable"] == "/var/lingxi/local-app-build/{app_id}/{channel}/project/node_modules/vite/bin/vite.js"
assert policy["dependency_snapshot"] == {
    "source": "embedded:vite-react-static-v1/pnpm-lock.yaml",
    "materialize_into": "/var/lingxi/local-app-build/{app_id}/{channel}/project/node_modules",
    "guest_mount": "forbidden",
    "selection_policy": "locked_template_only",
    "install_command": "pnpm install --frozen-lockfile --ignore-scripts --no-runtime --prefer-offline",
}
assert policy["build_mount"] == {
    "kind": "LocalAppBuild",
    "count": 1,
    "host_path_policy": "workspace_or_staging_or_store_root",
    "guest_path": "/var/lingxi/local-app-build/{app_id}/{channel}/project",
    "writable": True,
}
commands = policy["commands"]
assert sorted(commands) == ["vite_static_build"]
command = commands["vite_static_build"]
assert command["network_policy"] == "disabled"
assert command["memory_limit_policy"] == "physical_memory_tier"
assert "memory_limit_bytes" not in command
assert command["argv"][1] == "--max-old-space-size={build_node_old_space_size_mib}"
assert command["argv"][2] == "/var/lingxi/local-app-build/{app_id}/{channel}/project/node_modules/vite/bin/vite.js"
assert command["argv"][-4:] == ["build", "--outDir", "dist", "--emptyOutDir"]
assert command["cwd"] == "/var/lingxi/local-app-build/{app_id}/{channel}/project"
assert command["output_dir"] == "dist"
assert command["environment"] == {
    "NODE_ENV": "production",
    "HOME": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/home",
    "TMPDIR": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/tmp",
    "TMP": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/tmp",
    "TEMP": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/tmp",
    "XDG_CACHE_HOME": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/xdg-cache",
    "XDG_CONFIG_HOME": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/xdg-config",
    "XDG_DATA_HOME": "/var/lingxi/local-app-build/{app_id}/{channel}/project/.lingxi-build-state/xdg-data",
}
limits = policy["limits"]
assert limits["build_node_old_space_percent"] == 75
assert limits["build_memory_tiers"] == [
    {
        "physical_memory_max_exclusive_bytes": 6 * 1024**3,
        "process_tree_memory_bytes": 2048 * 1024**2,
        "node_max_old_space_size_mib": 1536,
    },
    {
        "physical_memory_max_exclusive_bytes": 8 * 1024**3,
        "process_tree_memory_bytes": 3072 * 1024**2,
        "node_max_old_space_size_mib": 2304,
    },
    {
        "physical_memory_max_exclusive_bytes": None,
        "process_tree_memory_bytes": 4096 * 1024**2,
        "node_max_old_space_size_mib": 3072,
    },
]
assert limits["runtime_process_tree_memory_bytes"] == 838860800
assert "node_process_tree_memory_bytes" not in limits
launcher = policy["android_network_policy_launcher"]
assert launcher["supported_network_policies"] == ["disabled", "loopback_only"]
assert launcher["loopback_only_ready"] is True
ish_policy = policy["ios_ish_execution_policy"]
assert ish_policy["supported_network_policies"] == ["disabled", "loopback_only"]
assert ish_policy["loopback_only_ready"] is True
assert ish_policy["hook_version"] == 1
assert ish_policy["runtime_memory_limit_bytes"] == 838860800
assert "memory_limit_bytes" not in ish_policy
assert ish_policy["watchdog_interval_ms"] == 250
assert ish_policy["memory_accounting"] == "guest_backed_pages_by_execution_context"
assert ish_policy["local_app_build_mount_layout"] == "single_root_materialized_snapshot"
assert ish_policy["nested_bind_mount_resolution"] == "longest_guest_prefix"
PY

cp -R "${TEMPLATE}" "${TEMP_ROOT}/vite-compressed-size"
python3 - "${TEMP_ROOT}/vite-compressed-size/vite.config.mjs" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
config = path.read_text(encoding="utf-8")
if "reportCompressedSize: false" not in config:
    raise SystemExit("fixed Vite config is missing the compressed-size policy fixture")
path.write_text(
    config.replace(
        "reportCompressedSize: false",
        "reportCompressedSize: true,\n    // reportCompressedSize: false",
    ),
    encoding="utf-8",
)
PY
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" \
  --vite-template "${TEMP_ROOT}/vite-compressed-size"; then
  echo "expected Vite compressed-size reporting to fail validation" >&2
  exit 1
fi

python3 "${SCRIPT_DIR}/generate-local-app-sbom.py" \
  --lock "${TEMPLATE}/pnpm-lock.yaml" \
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
value["dependencies"]["vite"] = "8.2.2"
path.write_text(json.dumps(value), encoding="utf-8")
PY
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/dependency-drift"; then
  echo "expected dependency drift to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/network-bypass"
printf '\nfetch("https://example.com");\n' >> "${TEMP_ROOT}/network-bypass/app/main.jsx"
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
printf '\n' >> "${TEMP_ROOT}/lock-drift/pnpm-lock.yaml"
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/lock-drift"; then
  echo "expected pnpm-lock byte drift to fail validation" >&2
  exit 1
fi

cp -R "${TEMPLATE}" "${TEMP_ROOT}/external-script"
python3 - "${TEMP_ROOT}/external-script/index.html" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(
    path.read_text(encoding="utf-8").replace(
        "</body>",
        '    <script src="https://example.com/escape.js"></script>\n  </body>',
    ),
    encoding="utf-8",
)
PY
if python3 "${TOOL}" --repo-root "${REPO_ROOT}" --template "${TEMP_ROOT}/external-script"; then
  echo "expected an external script tag to fail validation" >&2
  exit 1
fi

NODE_MODULES="${TEMP_ROOT}/node_modules"
mkdir -p "${NODE_MODULES}"
python3 - "${NODE_MODULES}" "${TOOL}" <<'PY'
import importlib.util
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
source = pathlib.Path(sys.argv[2])
spec = importlib.util.spec_from_file_location("local_app_supply_chain", source)
if spec is None or spec.loader is None:
    raise SystemExit(f"cannot load {source}")
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
packages = dict(verify.EXPECTED_DEPENDENCIES)
packages.update({
    "rolldown": "1.2.4",
    "lightningcss": "1.33.0",
    "@tailwindcss/oxide": "4.3.3",
    "@rolldown/binding-linux-arm64-musl": "1.2.4",
    "@rolldown/binding-linux-x64-musl": "1.2.4",
    "lightningcss-linux-arm64-musl": "1.33.0",
    "lightningcss-linux-x64-musl": "1.33.0",
    "@tailwindcss/oxide-linux-arm64-musl": "4.3.3",
    "@tailwindcss/oxide-linux-x64-musl": "4.3.3",
})
for name, version in packages.items():
    package = root.joinpath(*name.split("/"))
    package.mkdir(parents=True, exist_ok=True)
    (package / "package.json").write_text(
        json.dumps({"name": name, "version": version}),
        encoding="utf-8",
    )
(root / "vite/bin").mkdir(parents=True, exist_ok=True)
(root / "vite/bin/vite.js").write_text("#!/usr/bin/env node\n", encoding="utf-8")
(root / "@rolldown/binding-linux-arm64-musl/rolldown-binding.linux-arm64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "@rolldown/binding-linux-x64-musl/rolldown-binding.linux-x64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "lightningcss-linux-arm64-musl/lightningcss.linux-arm64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "lightningcss-linux-x64-musl/lightningcss.linux-x64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "@tailwindcss/oxide-linux-arm64-musl/tailwindcss-oxide.linux-arm64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
(root / "@tailwindcss/oxide-linux-x64-musl/tailwindcss-oxide.linux-x64-musl.node").write_bytes(bytes.fromhex("7f454c46") + b"fixture")
PY
python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${STAGED_OUTPUT}" \
  --platform android \
  --variant play
test -f "${STAGED_OUTPUT}/runtime-manifest.json"
test ! -w "${STAGED_OUTPUT}/node_modules/vite/package.json"
test ! -e "${STAGED_OUTPUT}/node_modules/@next"
python3 - "${STAGED_OUTPUT}/runtime-manifest.json" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert manifest["resolved_rolldown_bindings"] == [
    "@rolldown/binding-linux-arm64-musl",
    "@rolldown/binding-linux-x64-musl",
], manifest
assert manifest["resolved_lightningcss_bindings"] == [
    "lightningcss-linux-arm64-musl",
    "lightningcss-linux-x64-musl",
], manifest
assert manifest["resolved_oxide_bindings"] == [
    "@tailwindcss/oxide-linux-arm64-musl",
    "@tailwindcss/oxide-linux-x64-musl",
], manifest
PY
python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${STAGED_OUTPUT}" \
  --platform android \
  --variant play

NEXT_DRIFT_NODE_MODULES="${TEMP_ROOT}/node-modules-next-drift"
cp -R "${NODE_MODULES}" "${NEXT_DRIFT_NODE_MODULES}"
mkdir -p "${NEXT_DRIFT_NODE_MODULES}/next" "${NEXT_DRIFT_NODE_MODULES}/@next/swc-linux-arm64-musl"
python3 - "${NEXT_DRIFT_NODE_MODULES}" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
(root / "next/package.json").write_text(
    json.dumps({"name": "next", "version": "0.0.0-forbidden"}),
    encoding="utf-8",
)
(root / "@next/swc-linux-arm64-musl/package.json").write_text(
    json.dumps({"name": "@next/swc-linux-arm64-musl", "version": "0.0.0-forbidden"}),
    encoding="utf-8",
)
(root / "@next/swc-linux-arm64-musl/next-swc.linux-arm64-musl.node").write_bytes(
    bytes.fromhex("7f454c46") + b"fixture"
)
PY
if python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${NEXT_DRIFT_NODE_MODULES}" \
  --output "${TEMP_ROOT}/next-drift-output" \
  --platform android \
  --variant play; then
  echo "expected staged runtime to reject Next/SWC drift" >&2
  exit 1
fi

IOS_NODE_MODULES="${TEMP_ROOT}/node_modules-ios"
cp -R "${NODE_MODULES}" "${IOS_NODE_MODULES}"
rm -rf \
  "${IOS_NODE_MODULES}/@rolldown/binding-linux-x64-musl" \
  "${IOS_NODE_MODULES}/lightningcss-linux-x64-musl" \
  "${IOS_NODE_MODULES}/@tailwindcss/oxide-linux-x64-musl"
python3 "${SCRIPT_DIR}/stage-local-app-runtime.py" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${IOS_NODE_MODULES}" \
  --output "${IOS_STAGED_OUTPUT}" \
  --platform ios \
  --variant store
python3 - "${IOS_STAGED_OUTPUT}/runtime-manifest.json" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert manifest["resolved_rolldown_bindings"] == ["@rolldown/binding-linux-arm64-musl"], manifest
assert manifest["resolved_lightningcss_bindings"] == ["lightningcss-linux-arm64-musl"], manifest
assert manifest["resolved_oxide_bindings"] == ["@tailwindcss/oxide-linux-arm64-musl"], manifest
PY

# The iOS bundle install step. The staged tree is 0555/0444, so a second build
# cannot `rm -rf` the previous copy without first restoring write permission —
# and `cp -R` into a destination that still exists nests the new runtime inside
# the stale one, shipping build #1's node_modules. That removal-and-copy used to
# live inline in the Xcode build phase where nothing could run it; it is now
# clients/ios/scripts/install-staged-local-app-runtime.sh.
INSTALL_STAGED="${TEMP_ROOT}/install-src/store"
INSTALL_DEST="${TEMP_ROOT}/install-dest/local-app-runtime"
mkdir -p \
  "${INSTALL_STAGED}/node_modules/vite/bin"
printf '{"schema_version":1}\n' > "${INSTALL_STAGED}/runtime-manifest.json"
printf '#!/usr/bin/env node\n' > "${INSTALL_STAGED}/node_modules/vite/bin/vite.js"
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
test -f "${INSTALL_DEST}/node_modules/vite/bin/vite.js"
if "${REPO_ROOT}/clients/ios/scripts/install-staged-local-app-runtime.sh" \
  --staged "${TEMP_ROOT}/install-src/absent" \
  --destination "${INSTALL_DEST}" 2>/dev/null; then
  echo "expected a missing staged runtime to fail the install" >&2
  exit 1
fi

RUNTIME_ASSET_VALIDATOR="${REPO_ROOT}/clients/ios/scripts/validate-local-app-build-assets.sh"
ROOTFS_MANIFEST="${TEMP_ROOT}/ios-rootfs-manifest.json"
printf '{"local_app_runtime":false}\n' > "${ROOTFS_MANIFEST}"
if "${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --staged "${INSTALL_STAGED}" \
  --rootfs-manifest "${ROOTFS_MANIFEST}"; then
  echo "expected FullDebug iphoneos to reject a bare rootfs" >&2
  exit 1
fi
printf '{"local_app_runtime":true}\n' > "${ROOTFS_MANIFEST}"
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --staged "${INSTALL_STAGED}" \
  --rootfs-manifest "${ROOTFS_MANIFEST}"
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration StoreDebug \
  --platform iphoneos \
  --staged "${TEMP_ROOT}/install-src/absent" \
  --rootfs-manifest "${TEMP_ROOT}/missing-manifest.json"

# --rootfs-only judges the ROOTFS ALONE. The Xcode staging phase has no staged
# local-app node_modules tree (commit 1a4385f09 removed the phase that built
# one, on purpose), so the full validator can never run there. This scoped mode
# is what the build wires in. Assert the EXACT exit code: an unrecognized flag
# also exits non-zero, which would make a bare `if ! ...` pass for the wrong
# reason and certify a validator that never understood the request.
printf '{"local_app_runtime":false}\n' > "${ROOTFS_MANIFEST}"
set +e
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --rootfs-only \
  --rootfs-manifest "${ROOTFS_MANIFEST}"
rootfs_only_rc=$?
set -e
if [ "${rootfs_only_rc}" -ne 1 ]; then
  echo "expected --rootfs-only to REJECT a bare rootfs with exit 1, got ${rootfs_only_rc}" >&2
  exit 1
fi

printf '{"local_app_runtime":true}\n' > "${ROOTFS_MANIFEST}"
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --rootfs-only \
  --rootfs-manifest "${ROOTFS_MANIFEST}"

# A simulator build stays exempt even in rootfs-only mode.
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphonesimulator \
  --rootfs-only \
  --rootfs-manifest "${TEMP_ROOT}/missing-manifest.json"

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
