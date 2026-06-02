#!/usr/bin/env bash
#
# M10-P1b — build-xcframework.sh
#
# Reproduces the GITIGNORED iOS integration artifacts the XcodeGen project links
# against, so a fresh checkout can build the app after one `xcodegen generate`:
#
#   1. Host cdylib build of `ios-framework` (+ `uniffi-bindgen --library`) →
#      Swift bindings + FFI header + modulemap into  clients/ios/Generated/
#   2. Per-arch `staticlib` builds:
#        - aarch64-apple-ios          (device)
#        - aarch64-apple-ios-sim      (Apple-silicon simulator)
#        - x86_64-apple-ios           (Intel simulator)
#   3. lipo the two simulator slices into one fat sim archive.
#   4. `xcodebuild -create-xcframework` over { device slice, fat-sim slice },
#      each paired with the generated headers+modulemap →
#        clients/ios/Frameworks/LingxiCodeFFI.xcframework
#
# Idempotent: regenerated dirs are cleaned first; safe to re-run. Prints the
# output paths on success.
#
# NO secrets here: the LLM API key (ANTHROPIC_API_KEY) is read by the engine
# from the runtime environment / app-config — never baked into the framework.

set -euo pipefail

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"                       # clients/ios
REPO_ROOT="$(cd "${IOS_DIR}/../.." && pwd)"                     # worktree root
CARGO_DIR="${REPO_ROOT}/lingxi-code"                           # Rust workspace
CARGO_TARGET_DIR="${CARGO_DIR}/target"

CRATE="ios-framework"
LIB_STEM="ios_framework"            # cargo turns the `-` into `_`
STATICLIB="lib${LIB_STEM}.a"
HOST_DYLIB="lib${LIB_STEM}.dylib"

# Cargo build profile for the device/sim staticlibs.
PROFILE="release"
PROFILE_DIR="release"               # `--release` → target/<triple>/release

GEN_DIR="${IOS_DIR}/Generated"
FRAMEWORKS_DIR="${IOS_DIR}/Frameworks"
XCFRAMEWORK="${FRAMEWORKS_DIR}/LingxiCodeFFI.xcframework"

# Scratch area for the lipo'd fat-sim archive + assembled header dirs.
BUILD_DIR="${IOS_DIR}/build/xcframework"

DEVICE_TARGET="aarch64-apple-ios"
SIM_TARGETS=("aarch64-apple-ios-sim" "x86_64-apple-ios")

# Use the Xcode-toolchain lipo (xcrun resolves it) — avoids a stray `lipo` on
# PATH (e.g. anaconda) that does not understand Mach-O fat archives correctly.
LIPO=(xcrun lipo)

log() { printf '\033[1;34m[build-xcframework]\033[0m %s\n' "$*"; }

# ---------------------------------------------------------------------------
# 0. Preflight — tools + Rust targets
# ---------------------------------------------------------------------------
for tool in cargo rustc xcrun xcodebuild; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: required tool not found: ${tool}" >&2; exit 1; }
done

# Resolve the toolchain cargo/rustc actually use here (the workspace pins one via
# rust-toolchain.toml), and check the iOS std targets are installed FOR THAT
# toolchain — `rustup target list --installed` without `--toolchain` reports the
# default toolchain's targets, which can differ from the pinned one.
ACTIVE_TOOLCHAIN="$(cd "${CARGO_DIR}" && rustup show active-toolchain 2>/dev/null | awk '{print $1}')"
if [[ -n "${ACTIVE_TOOLCHAIN}" ]]; then
  INSTALLED_TARGETS="$(rustup target list --toolchain "${ACTIVE_TOOLCHAIN}" --installed 2>/dev/null || true)"
  MISSING=()
  for t in "${DEVICE_TARGET}" "${SIM_TARGETS[@]}"; do
    if [[ -n "${INSTALLED_TARGETS}" ]] && ! grep -qx "${t}" <<<"${INSTALLED_TARGETS}"; then
      MISSING+=("${t}")
    fi
  done
  if [[ ${#MISSING[@]} -gt 0 ]]; then
    log "Installing missing Rust std targets for ${ACTIVE_TOOLCHAIN}: ${MISSING[*]}"
    rustup target add --toolchain "${ACTIVE_TOOLCHAIN}" "${MISSING[@]}"
  fi
fi

# ---------------------------------------------------------------------------
# 1. Host cdylib + Swift bindings (uniffi-bindgen --library)
# ---------------------------------------------------------------------------
log "Building host cdylib for bindgen introspection…"
cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features uniffi

HOST_DYLIB_PATH="${CARGO_TARGET_DIR}/debug/${HOST_DYLIB}"
[[ -f "${HOST_DYLIB_PATH}" ]] || { echo "ERROR: host dylib not produced: ${HOST_DYLIB_PATH}" >&2; exit 1; }

log "Generating Swift bindings into ${GEN_DIR} …"
rm -rf "${GEN_DIR}"
mkdir -p "${GEN_DIR}"
# The `ios-framework` `uniffi-bindgen` bin is NOT the stock
# `uniffi::uniffi_bindgen_main()`. Stock UniFFI 0.28.3 `--library` mode PANICS on
# our surface (`interface/mod.rs:1126` — "unknown throw type") because the shared
# host's async export `MobileEngineHandle::submit() -> Result<(), client_protocol
# ::ClientError>` throws an error type that lives in a DIFFERENT crate, which
# 0.28.3 records as `Type::External` and cannot render in the throws position.
#
# Our bin reimplements the Swift `generate --library` path over UniFFI's public
# library APIs and re-tags that one external error throw type so 0.28.3 can emit
# it (see the bin's module docs). It ships no `uniffi.toml`, so it uses an empty
# config supplier and NEVER runs `cargo metadata` — no `--metadata-no-deps` flag,
# no CWD-must-be-the-workspace requirement, no edition-2024 manifest-parse hazard.
cargo run --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features cli \
  --bin uniffi-bindgen -- \
  generate \
  --library "${HOST_DYLIB_PATH}" \
  --language swift \
  --out-dir "${GEN_DIR}"

# UniFFI 0.28 emits, per FFI namespace: <name>.swift, <name>FFI.h, <name>FFI.modulemap.
# Merge the per-namespace modulemaps into a single `module.modulemap` so a single
# Clang module wraps every generated header for the xcframework + Swift importer.
MODULEMAP="${GEN_DIR}/module.modulemap"
: > "${MODULEMAP}"
shopt -s nullglob
for mm in "${GEN_DIR}"/*.modulemap; do
  [[ "${mm}" == "${MODULEMAP}" ]] && continue
  cat "${mm}" >> "${MODULEMAP}"
  printf '\n' >> "${MODULEMAP}"
  rm -f "${mm}"
done
shopt -u nullglob

# ---------------------------------------------------------------------------
# 1b. Single-Swift-module dedup pass
# ---------------------------------------------------------------------------
# UniFFI 0.28 `--library` mode emits ONE self-contained `.swift` per namespace
# (client_protocol / client_adapter / engine_mobile / ios_framework). The iOS
# app compiles all four into ONE Swift module — which UniFFI itself REQUIRES for
# cross-namespace (external) type access: a throwing method in `engine_mobile`
# (`MobileEngineHandle.submit`) references `FfiConverterTypeClientError` /
# `FfiConverterTypeClientCommand` that are DEFINED in `client_protocol` with no
# explicit Swift import (UniFFI docs, types/remote_ext_types.md: "all generated
# .swift files must be compiled together in a single module").
#
# But each file ALSO emits its own copy of the shared runtime scaffolding
# (RustBuffer/Data readers+writers, the `FfiConverter` / `FfiConverterRustBuffer`
# protocols, `rustCall`, `UniffiHandleMap`, `UniffiInternalError`, …) marked
# `private`/`fileprivate` (file-scoped) so the copies don't COLLIDE in one
# module. That compiles for every namespace in isolation, but breaks the ONE
# cross-file reference that matters: `engine_mobile.swift` calling
# `FfiConverterTypeClientError.lift` — a `public` converter type whose `lift`
# witness comes from `client_protocol.swift`'s `private protocol
# FfiConverterRustBuffer`, so it is "inaccessible due to 'private' protection"
# from another file. (Swift error: engine_mobile.swift … 'lift' is inaccessible.)
#
# Fix (deterministic, no engine/bindgen-semantics change): keep the shared
# scaffolding in ONE canonical file and make it module-visible; strip the
# duplicate scaffolding from the others so module-internal references resolve to
# the single canonical copy. The scaffolding block is bounded EXACTLY by
# UniFFI's stable markers: it runs from the first line AFTER the per-namespace
#   `#if canImport(<ns>FFI) … #endif`
# import block, up to (but excluding) the line `// Public interface members
# begin here.`. The per-file `#if canImport(<ns>FFI)` import is PRESERVED in
# every file (each namespace still imports its own C/FFI module), and every
# below-marker per-namespace helper (`uniffiEnsureInitialized`, the async
# future callbacks, the checksum `initializationResult`) stays file-local.
#
# Canonical file = client_protocol.swift: it defines the cross-referenced
# `ClientCommand` / `ClientError`, so its namespace's `ffi_client_protocol_
# rustbuffer_*` symbols (used by the shared `RustBuffer` extension) are the ones
# the deduped scaffolding calls — all present in the linked staticlib.
log "Deduplicating shared Swift scaffolding into a single module…"
CANON="client_protocol"
MARKER="// Public interface members begin here."

# Sanity: every generated file must carry the marker (else the bindgen output
# changed shape and this surgery would be unsafe — fail loudly rather than
# emit a silently-broken module).
for sw in "${GEN_DIR}"/*.swift; do
  grep -qF "${MARKER}" "${sw}" || {
    echo "ERROR: dedup: marker not found in ${sw}; bindgen output shape changed" >&2
    exit 1
  }
done

for sw in "${GEN_DIR}"/*.swift; do
  stem="$(basename "${sw}" .swift)"
  tmp="${sw}.dedup"
  if [[ "${stem}" == "${CANON}" ]]; then
    # Canonical: drop the leading `private `/`fileprivate ` from TOP-LEVEL
    # (column-0) scaffolding decls ABOVE the marker so the kept copy is
    # module-internal and the `public` converter types' inherited `lift`/`lower`
    # witnesses are reachable from the other namespaces' files. Lines from the
    # marker onward are emitted verbatim.
    awk -v marker="${MARKER}" '
      index($0, marker) == 1 { seen_marker = 1 }
      !seen_marker && /^(private|fileprivate) (func|protocol|struct|enum|class|extension|let|var|typealias) / {
        sub(/^(private|fileprivate) /, "")
      }
      { print }
    ' "${sw}" > "${tmp}"
  else
    # Non-canonical: delete the scaffolding block — everything from the line
    # AFTER the `#endif` that closes the `#if canImport(<ns>FFI)` import up to
    # the line BEFORE the marker. The import block (and the marker + all
    # below-marker namespace code) is preserved verbatim.
    awk -v marker="${MARKER}" '
      index($0, marker) == 1 { in_scaffold = 0 }                 # marker ends the strip region
      in_scaffold { next }                                       # drop scaffolding lines
      { print }
      !done_import && $0 ~ /^#endif$/ { in_scaffold = 1; done_import = 1 }  # first #endif = end of canImport block
    ' "${sw}" > "${tmp}"
  fi
  mv "${tmp}" "${sw}"
done

SWIFT_COUNT="$(ls "${GEN_DIR}"/*.swift 2>/dev/null | wc -l | tr -d ' ')"
[[ "${SWIFT_COUNT}" -gt 0 ]] || { echo "ERROR: no Swift bindings generated in ${GEN_DIR}" >&2; exit 1; }
log "Swift bindings: ${SWIFT_COUNT} .swift file(s) + headers + module.modulemap (single-module deduped)"

# ---------------------------------------------------------------------------
# 2. Per-arch staticlib builds
# ---------------------------------------------------------------------------
for t in "${DEVICE_TARGET}" "${SIM_TARGETS[@]}"; do
  log "Building staticlib (${PROFILE}) for ${t} …"
  cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features uniffi \
    --target "${t}" --"${PROFILE}"
  arch_lib="${CARGO_TARGET_DIR}/${t}/${PROFILE_DIR}/${STATICLIB}"
  [[ -f "${arch_lib}" ]] || { echo "ERROR: staticlib not produced: ${arch_lib}" >&2; exit 1; }
done

DEVICE_LIB="${CARGO_TARGET_DIR}/${DEVICE_TARGET}/${PROFILE_DIR}/${STATICLIB}"

# ---------------------------------------------------------------------------
# 3. lipo the simulator slices into one fat archive
# ---------------------------------------------------------------------------
rm -rf "${BUILD_DIR}"
mkdir -p "${BUILD_DIR}"
FAT_SIM_LIB="${BUILD_DIR}/${STATICLIB}"

sim_inputs=()
for t in "${SIM_TARGETS[@]}"; do
  sim_inputs+=("${CARGO_TARGET_DIR}/${t}/${PROFILE_DIR}/${STATICLIB}")
done
log "lipo-ing simulator slices → ${FAT_SIM_LIB}"
"${LIPO[@]}" -create "${sim_inputs[@]}" -output "${FAT_SIM_LIB}"

# ---------------------------------------------------------------------------
# 4. Assemble headers + modulemap, then create the xcframework
# ---------------------------------------------------------------------------
HEADERS_DIR="${BUILD_DIR}/Headers"
rm -rf "${HEADERS_DIR}"
mkdir -p "${HEADERS_DIR}"
shopt -s nullglob
for h in "${GEN_DIR}"/*.h; do cp "${h}" "${HEADERS_DIR}/"; done
shopt -u nullglob
cp "${MODULEMAP}" "${HEADERS_DIR}/module.modulemap"

# Idempotent: -create-xcframework refuses to overwrite an existing output.
rm -rf "${XCFRAMEWORK}"
mkdir -p "${FRAMEWORKS_DIR}"

log "Creating ${XCFRAMEWORK} …"
xcodebuild -create-xcframework \
  -library "${DEVICE_LIB}" -headers "${HEADERS_DIR}" \
  -library "${FAT_SIM_LIB}" -headers "${HEADERS_DIR}" \
  -output "${XCFRAMEWORK}"

[[ -d "${XCFRAMEWORK}" ]] || { echo "ERROR: xcframework not produced: ${XCFRAMEWORK}" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
log "OK"
echo "Swift bindings : ${GEN_DIR}"
echo "XCFramework    : ${XCFRAMEWORK}"
