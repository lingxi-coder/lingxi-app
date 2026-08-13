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

# Keep Rust's linker target and every vendored C/C++ dependency on the same
# minimum OS as the XcodeGen project. Without this, current Xcode compiles C
# objects for the SDK default (for example iOS 26.x) while rustc links for its
# historical iOS 10 default, which can introduce unavailable symbols such as
# ___chkstk_darwin.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

GEN_DIR="${IOS_DIR}/Generated"
FRAMEWORKS_DIR="${IOS_DIR}/Frameworks"
XCFRAMEWORK="${FRAMEWORKS_DIR}/LingxiCodeFFI.xcframework"

# Scratch area for the lipo'd fat-sim archive + assembled header dirs.
BUILD_DIR="${IOS_DIR}/build/xcframework"

DEVICE_TARGET="aarch64-apple-ios"
SIM_TARGETS=("aarch64-apple-ios-sim" "x86_64-apple-ios")
BUILD_TARGETS=("${DEVICE_TARGET}" "${SIM_TARGETS[@]}")
if [[ "${LINGXI_DEVICE_ONLY:-0}" == "1" ]]; then
  BUILD_TARGETS=("${DEVICE_TARGET}")
fi

# Use the Xcode-toolchain lipo (xcrun resolves it) — avoids a stray `lipo` on
# PATH (e.g. anaconda) that does not understand Mach-O fat archives correctly.
LIPO=(xcrun lipo)
LIBTOOL=(xcrun libtool)

log() { printf '\033[1;34m[build-xcframework]\033[0m %s\n' "$*"; }

# ---------------------------------------------------------------------------
# 0. Preflight — tools + Rust targets
# ---------------------------------------------------------------------------
for tool in cargo rustc xcrun xcodebuild; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: required tool not found: ${tool}" >&2; exit 1; }
done

# Build the device-only iSH archives and bundled Alpine fakefs before the Rust
# device slice. The helper is idempotent and sources everything from the pinned
# OpenMinis submodule; no generated binary or rootfs is committed.
LINUX_RUNTIME_BUILD="${SCRIPT_DIR}/build-linux-runtime.sh"
if [[ -x "${LINUX_RUNTIME_BUILD}" ]]; then
  if [[ "${LINGXI_REUSE_STAGED_LINUX_RUNTIME:-0}" == "1" ]]; then
    STAGED_LINUX_RUNTIME="${IOS_DIR}/build/linux-runtime/openminis"
    for required in \
      "${STAGED_LINUX_RUNTIME}/manifest.json" \
      "${STAGED_LINUX_RUNTIME}/libs/libfakefs.a" \
      "${STAGED_LINUX_RUNTIME}/libs/libish.a" \
      "${STAGED_LINUX_RUNTIME}/libs/libish_emu.a" \
      "${STAGED_LINUX_RUNTIME}/resources/alpine-rootfs.zip"; do
      [[ -f "${required}" ]] || {
        echo "ERROR: staged Linux runtime is incomplete: ${required}" >&2
        exit 1
      }
    done
    log "Reusing complete staged iSH ARM64 + Alpine Linux runtime…"
  else
    log "Building iSH ARM64 + Alpine Linux runtime…"
    # Propagate the runtime mode. Calling the helper bare rebuilds the LEGACY
    # bare minirootfs and overwrites whatever is staged, so an xcframework build
    # run after a local-app rootfs build silently reverted the app to a rootfs
    # with no Node in it.
    if [[ "${LINGXI_LOCAL_APP_RUNTIME:-0}" == "1" ]]; then
      "${LINUX_RUNTIME_BUILD}" --local-app-runtime
    else
      "${LINUX_RUNTIME_BUILD}"
    fi
  fi
fi
SIMULATOR_SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"

# Resolve the toolchain cargo/rustc actually use here (the workspace pins one via
# rust-toolchain.toml), and check the iOS std targets are installed FOR THAT
# toolchain — `rustup target list --installed` without `--toolchain` reports the
# default toolchain's targets, which can differ from the pinned one.
ACTIVE_TOOLCHAIN="$(cd "${CARGO_DIR}" && rustup show active-toolchain 2>/dev/null | awk '{print $1}')"
if [[ -n "${ACTIVE_TOOLCHAIN}" ]]; then
  INSTALLED_TARGETS="$(rustup target list --toolchain "${ACTIVE_TOOLCHAIN}" --installed 2>/dev/null || true)"
  MISSING=()
  for t in "${BUILD_TARGETS[@]}"; do
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
# A constrained local build can preserve bindings generated by the exact same
# Rust sources while rebuilding only the target static libraries. This avoids a
# second host graph solely for bindgen. The normal/fresh-checkout path remains
# the default and always regenerates.
if [[ "${LINGXI_REUSE_GENERATED_BINDINGS:-0}" == "1" ]]; then
  for required in \
    "${GEN_DIR}/client_protocol.swift" \
    "${GEN_DIR}/client_protocolFFI.h" \
    "${GEN_DIR}/engine_mobile.swift" \
    "${GEN_DIR}/ios_framework.swift"; do
    [[ -f "${required}" ]] || {
      echo "ERROR: generated-binding reuse requested but file is missing: ${required}" >&2
      exit 1
    }
  done
  log "Reusing validated Swift bindings in ${GEN_DIR} …"
else
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
fi

# UniFFI 0.28 emits, per FFI namespace: <name>.swift, <name>FFI.h, <name>FFI.modulemap.
# Merge the per-namespace modulemaps into a single `module.modulemap` so a single
# Clang module wraps every generated header for the xcframework + Swift importer.
MODULEMAP="${GEN_DIR}/module.modulemap"
shopt -s nullglob
NAMESPACE_MODULEMAPS=("${GEN_DIR}"/*FFI.modulemap)
if [[ ${#NAMESPACE_MODULEMAPS[@]} -gt 0 ]]; then
  : > "${MODULEMAP}"
  for mm in "${NAMESPACE_MODULEMAPS[@]}"; do
    cat "${mm}" >> "${MODULEMAP}"
    printf '\n' >> "${MODULEMAP}"
    rm -f "${mm}"
  done
elif [[ ! -s "${MODULEMAP}" ]]; then
  # A previous reuse invocation may have already consumed the per-namespace
  # files. Reconstruct the same flat module map from the generated headers
  # instead of silently packaging an empty map (which makes every FFI symbol
  # disappear from Swift at module-emission time).
  : > "${MODULEMAP}"
  for header in "${GEN_DIR}"/*FFI.h; do
    module="$(basename "${header}" .h)"
    printf 'module %s {\n  header "%s"\n  export *\n}\n\n' \
      "${module}" "$(basename "${header}")" >> "${MODULEMAP}"
  done
fi
shopt -u nullglob
[[ -s "${MODULEMAP}" ]] || {
  echo "ERROR: generated module.modulemap is empty" >&2
  exit 1
}

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

# ---------------------------------------------------------------------------
# 1c. Async-foreign-future scaffolding dedup (BELOW-marker)
# ---------------------------------------------------------------------------
# UniFFI emits a small async-foreign-future support block — including the
# MODULE-INTERNAL `protocol UniffiForeignFutureTask { func cancel() }` and its
# `extension Task: UniffiForeignFutureTask {}` conformance — ONCE PER NAMESPACE
# that exports an async callback interface. With >1 such namespace in the single
# Swift module (M10-P3a added `IosEventListener` in `ios_framework` alongside the
# pre-existing `ClientEventListener` in `client_adapter`), those two
# non-`private` declarations COLLIDE: "invalid redeclaration of
# 'UniffiForeignFutureTask'" / "ambiguous for type lookup". They sit BELOW the
# `// Public interface members begin here.` marker, so the §1b pass (which only
# touches the above-marker runtime block) does not catch them.
#
# Fix (deterministic, no bindgen-semantics change): keep this protocol + its
# `Task` conformance in exactly ONE keeper file and delete BOTH lines from every
# other file. The two declarations are identical across namespaces and
# module-internal, so module-internal references resolve to the single kept copy.
# Prefer client_adapter.swift as the keeper when it contains the exact shared
# declaration; otherwise keep the first file that actually emits it. The
# per-namespace `private UNIFFI_FOREIGN_FUTURE_HANDLE_MAP`, the `private`
# `uniffiTraitInterfaceCallAsync*` helpers, and the namespaced
# `uniffiForeignFutureHandleCount<Ns>()` are file-local / uniquely named and are
# left untouched.
log "Deduplicating async-foreign-future task protocol into a single module copy…"
# Newer UniFFI output may namespace the client_adapter copy
# (`ClientAdapterUniffiForeignFutureTask`) while leaving the ios_framework copy
# unnamespaced. Only files containing the exact shared declaration participate
# in this pass; if there is a single copy, keep it where bindgen emitted it.
FUTURE_PROTOCOL_FILES=()
for sw in "${GEN_DIR}"/*.swift; do
  if grep -q '^protocol UniffiForeignFutureTask ' "${sw}"; then
    FUTURE_PROTOCOL_FILES+=("${sw}")
  fi
done

if [[ ${#FUTURE_PROTOCOL_FILES[@]} -gt 1 ]]; then
  FUTURE_KEEPER="${FUTURE_PROTOCOL_FILES[0]}"
  for sw in "${FUTURE_PROTOCOL_FILES[@]}"; do
    if [[ "$(basename "${sw}" .swift)" == "client_adapter" ]]; then
      FUTURE_KEEPER="${sw}"
      break
    fi
  done

  for sw in "${FUTURE_PROTOCOL_FILES[@]}"; do
    [[ "${sw}" == "${FUTURE_KEEPER}" ]] && continue
    tmp="${sw}.fdedup"
    # Strip the `protocol UniffiForeignFutureTask { … }` block (column-0
    # `protocol` line through its closing column-0 `}`) and the
    # immediately-following `extension Task: UniffiForeignFutureTask {}`
    # one-liner. Other lines are emitted verbatim.
    awk '
      /^protocol UniffiForeignFutureTask / { in_proto = 1; next }
      in_proto && /^}/                     { in_proto = 0; next }
      in_proto                             { next }
      /^extension Task: UniffiForeignFutureTask \{\}/ { next }
      { print }
    ' "${sw}" > "${tmp}"
    mv "${tmp}" "${sw}"
  done
fi

# ---------------------------------------------------------------------------
# 1d. Force callback-vtable registration in `buildIosEngine`
# ---------------------------------------------------------------------------
# UniFFI registers a `callback_interface`'s foreign vtable inside its namespace's
# `private` lazy `initializationResult`, which is forced ONLY by that namespace's
# `uniffiEnsureInitialized()` — called from async FFI calls or the SYNCHRONOUS
# `makeRustCall`/`rustCallWithError` paths' callers, but NOT by the generated
# `buildIosEngine` itself (it is a bare `rustCallWithError` that lowers the
# listener WITHOUT first forcing `ios_framework`'s init). The engine then streams
# events into the registered `IosEventListener` foreign vtable — but if nothing
# forced `ios_framework`'s `initializationResult`, that vtable is never set, and
# the first inbound event panics in Rust with `uniffi_core … "Foreign pointer not
# set"`. (The pre-P4 EngineModule link smoke only called a C contract-version
# function, never built a handle, so this latent gap went unobserved.)
#
# Fix (deterministic, no bindgen/engine-semantics change): make `buildIosEngine`
# call `ios_framework`'s `uniffiEnsureInitialized()` as its FIRST statement, so
# the `uniffiCallbackInitIosEventListener()` inside the lazy init runs before the
# foreign listener is lowered and handed to the engine. The function/types are
# unchanged; this only forces the same one-time init UniFFI already runs for
# async entrypoints. Idempotent: skipped if the call is already present.
log "Forcing IosEventListener callback-vtable init in engine constructors…"
IOSF="${GEN_DIR}/ios_framework.swift"
[[ -f "${IOSF}" ]] || { echo "ERROR: ${IOSF} missing after bindgen" >&2; exit 1; }
for constructor in buildIosEngine buildIosEngineWithConfig; do
  if ! grep -qF "func ${constructor}(" "${IOSF}"; then
    echo "ERROR: ${constructor} not found in ${IOSF}; bindgen output shape changed" >&2
    exit 1
  fi
  marker="M10-P4:${constructor}"
  if ! grep -qF "${marker}" "${IOSF}"; then
    tmp="${IOSF}.initpatch"
    awk -v signature="public func ${constructor}(" -v marker="${marker}" '
      index($0, signature) == 1 {
        print
        print "    uniffiEnsureInitialized() // " marker " register IosEventListener vtable before lowering"
        next
      }
      { print }
    ' "${IOSF}" > "${tmp}"
    grep -qF "${marker}" "${tmp}" || {
      echo "ERROR: failed to inject uniffiEnsureInitialized() into ${constructor}" >&2
      exit 1
    }
    mv "${tmp}" "${IOSF}"
  fi
done

SWIFT_COUNT="$(find "${GEN_DIR}" -maxdepth 1 -type f -name '*.swift' -print | wc -l | tr -d ' ')"
[[ "${SWIFT_COUNT}" -gt 0 ]] || { echo "ERROR: no Swift bindings generated in ${GEN_DIR}" >&2; exit 1; }
log "Swift bindings: ${SWIFT_COUNT} .swift file(s) + headers + module.modulemap (single-module deduped)"

# ---------------------------------------------------------------------------
# 2. Per-arch staticlib builds
# ---------------------------------------------------------------------------
for t in "${BUILD_TARGETS[@]}"; do
  log "Building staticlib (${PROFILE}) for ${t} …"
  if [[ "${t}" == "aarch64-apple-ios-sim" ]]; then
    # rquickjs-sys 0.6 passes Rust's `*-ios-sim` triple directly to libclang,
    # but Apple clang spells the same target `*-ios<version>-simulator`.
    # A later --target argument wins, and the explicit SDK supplies libc
    # headers to bindgen without changing Rust's target or the resulting slice.
    simulator_bindgen_args="--target=arm64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}-simulator -isysroot ${SIMULATOR_SDK}"
    BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} ${simulator_bindgen_args}" \
      cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features uniffi \
        --target "${t}" --"${PROFILE}"
  elif [[ "${t}" == "x86_64-apple-ios" ]]; then
    simulator_bindgen_args="--target=x86_64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}-simulator -isysroot ${SIMULATOR_SDK}"
    BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} ${simulator_bindgen_args}" \
      cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features uniffi \
        --target "${t}" --"${PROFILE}"
  else
    # The package also declares a cdylib for host-side UniFFI introspection.
    # On device that incidental dylib sees the Objective-C iSH bridge only at
    # the final Xcode app link, so permit those symbols to remain unresolved;
    # the staticlib packaged below is unaffected and Xcode resolves them.
    RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-undefined,dynamic_lookup" \
      cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" --features uniffi \
        --target "${t}" --"${PROFILE}"
  fi
  arch_lib="${CARGO_TARGET_DIR}/${t}/${PROFILE_DIR}/${STATICLIB}"
  [[ -f "${arch_lib}" ]] || { echo "ERROR: staticlib not produced: ${arch_lib}" >&2; exit 1; }
done

# ---------------------------------------------------------------------------
# 3. Merge native static dependencies, then lipo the simulator slices
# ---------------------------------------------------------------------------
rm -rf "${BUILD_DIR}"
mkdir -p "${BUILD_DIR}"

# Rust `staticlib` archives do not absorb native archives linked with
# `static:-bundle`. mbedtls-sys deliberately uses that mode for normal Rust
# binaries, so package its three target-specific archives beside the Rust
# objects before handing the result to Xcode.
resolve_mbedtls_out() {
  local target="$1"
  local candidates=()
  local selected candidate archive candidate_mtime selected_mtime representative minos
  shopt -s nullglob
  candidates=("${CARGO_TARGET_DIR}/${target}/${PROFILE_DIR}/build"/mbedtls-sys-*/out)
  shopt -u nullglob
  [[ ${#candidates[@]} -gt 0 ]] || {
    echo "ERROR: mbedTLS archives not found for ${target}" >&2
    return 1
  }
  selected="${candidates[0]}"
  selected_mtime="$(stat -f '%m' "${selected}")"
  for candidate in "${candidates[@]:1}"; do
    candidate_mtime="$(stat -f '%m' "${candidate}")"
    if (( candidate_mtime > selected_mtime )); then
      selected="${candidate}"
      selected_mtime="${candidate_mtime}"
    fi
  done
  for archive in libmbedtls.a libmbedx509.a libmbedcrypto.a; do
    [[ -f "${selected}/${archive}" ]] || {
      echo "ERROR: missing ${selected}/${archive}" >&2
      return 1
    }
  done
  # Cargo intentionally retains build-hash directories when an environment
  # input changes. The newest OUT_DIR is the one produced (or selected from
  # cache) by the immediately preceding target build; older directories may be
  # byte-different precisely because their deployment target was different.
  # Verify the selected C objects instead of either rejecting valid cache
  # history or accidentally merging a stale archive.
  representative="$(find "${selected}" -maxdepth 1 -type f -name '*.o' -print -quit)"
  [[ -n "${representative}" ]] || {
    echo "ERROR: no mbedTLS object available for deployment-target validation: ${selected}" >&2
    return 1
  }
  minos="$(xcrun vtool -show-build "${representative}" 2>/dev/null | awk '$1 == "minos" { print $2; exit }')"
  [[ "${minos}" == "${IPHONEOS_DEPLOYMENT_TARGET}" ]] || {
    echo "ERROR: mbedTLS ${target} object targets iOS ${minos:-unknown}; expected ${IPHONEOS_DEPLOYMENT_TARGET}" >&2
    return 1
  }
  printf '%s\n' "${selected}"
}

for t in "${BUILD_TARGETS[@]}"; do
  arch_lib="${CARGO_TARGET_DIR}/${t}/${PROFILE_DIR}/${STATICLIB}"
  mbedtls_out="$(resolve_mbedtls_out "${t}")"
  combined_dir="${BUILD_DIR}/${t}"
  mkdir -p "${combined_dir}"
  "${LIBTOOL[@]}" -static -no_warning_for_no_symbols -o "${combined_dir}/${STATICLIB}" \
    "${arch_lib}" \
    "${mbedtls_out}/libmbedtls.a" \
    "${mbedtls_out}/libmbedx509.a" \
    "${mbedtls_out}/libmbedcrypto.a"
done

DEVICE_LIB="${BUILD_DIR}/${DEVICE_TARGET}/${STATICLIB}"
FAT_SIM_LIB="${BUILD_DIR}/${STATICLIB}"

if [[ "${LINGXI_DEVICE_ONLY:-0}" != "1" ]]; then
  sim_inputs=()
  for t in "${SIM_TARGETS[@]}"; do
    sim_inputs+=("${BUILD_DIR}/${t}/${STATICLIB}")
  done
  log "lipo-ing simulator slices → ${FAT_SIM_LIB}"
  "${LIPO[@]}" -create "${sim_inputs[@]}" -output "${FAT_SIM_LIB}"
fi

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
if [[ "${LINGXI_DEVICE_ONLY:-0}" == "1" ]]; then
  xcodebuild -create-xcframework \
    -library "${DEVICE_LIB}" -headers "${HEADERS_DIR}" \
    -output "${XCFRAMEWORK}"
else
  xcodebuild -create-xcframework \
    -library "${DEVICE_LIB}" -headers "${HEADERS_DIR}" \
    -library "${FAT_SIM_LIB}" -headers "${HEADERS_DIR}" \
    -output "${XCFRAMEWORK}"
fi

[[ -d "${XCFRAMEWORK}" ]] || { echo "ERROR: xcframework not produced: ${XCFRAMEWORK}" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
log "OK"
echo "Swift bindings : ${GEN_DIR}"
echo "XCFramework    : ${XCFRAMEWORK}"
