#!/usr/bin/env bash
#
# clients/setup.sh — one-command client bootstrap for a fresh clone.
#
# Reproduces the fresh-clone setup the native shells need, end to end:
#
#   1. Engine for Electron : cargo build -p bridge-server
#        → <repo>/target/debug/bridge-server, which the renderer's
#          repo-relative resolveServerBin() discovers automatically.
#   2. Shared SDK          : (cd clients/shared && npm install && npm run build)
#        → dist/  (REQUIRED before electron — electron depends on file:../shared).
#   3. Electron shell      : (cd clients/electron && npm install)
#   4. Android bindings    : if cargo-ndk + an Android NDK are present →
#                            clients/android/scripts/build-mobile-linux-native.sh --variant play ; else SKIP.
#   5. iOS framework       : if on macOS with xcodebuild + xcodegen →
#                            clients/ios/scripts/build-xcframework.sh +
#                            xcodegen generate ; else SKIP.
#
# Idempotent: safe to re-run; npm install / cargo build / the per-client build
# scripts all no-op or rebuild incrementally on a second run.
#
# Optional toolchains (Android NDK, Xcode) gracefully SKIP with guidance instead
# of hard-failing the whole script. The required core (cargo + node/npm) is
# enforced up front.
#
# NO secrets: the LLM API key (ANTHROPIC_API_KEY) is read by the engine from the
# runtime environment — it is never baked in by this script.

set -euo pipefail

# ---------------------------------------------------------------------------
# Paths — resolve the repo root from THIS script's location (clients/setup.sh).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # clients/
CLIENTS_DIR="${SCRIPT_DIR}"
REPO_ROOT="$(cd "${CLIENTS_DIR}/.." && pwd)"                 # worktree / clone root
CARGO_DIR="${REPO_ROOT}"                         # Rust workspace
BRIDGE_BIN="${CARGO_DIR}/target/debug/bridge-server"

SHARED_DIR="${CLIENTS_DIR}/shared"
ELECTRON_DIR="${CLIENTS_DIR}/electron"
ANDROID_DIR="${CLIENTS_DIR}/android"
IOS_DIR="${CLIENTS_DIR}/ios"

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  C_BOLD="$(printf '\033[1m')"; C_DIM="$(printf '\033[2m')"
  C_GREEN="$(printf '\033[32m')"; C_YELLOW="$(printf '\033[33m')"
  C_BLUE="$(printf '\033[34m')"; C_RED="$(printf '\033[31m')"
  C_RESET="$(printf '\033[0m')"
else
  C_BOLD=""; C_DIM=""; C_GREEN=""; C_YELLOW=""; C_BLUE=""; C_RED=""; C_RESET=""
fi

STEP_NO=0
header() {
  STEP_NO=$((STEP_NO + 1))
  printf '\n%s========================================================================%s\n' "${C_BLUE}${C_BOLD}" "${C_RESET}"
  printf '%s[%d/%d] %s%s\n' "${C_BLUE}${C_BOLD}" "${STEP_NO}" "${TOTAL_STEPS}" "$1" "${C_RESET}"
  printf '%s========================================================================%s\n' "${C_BLUE}${C_BOLD}" "${C_RESET}"
}
info() { printf '%s   • %s%s\n' "${C_DIM}" "$1" "${C_RESET}"; }
ok()   { printf '%s   ✔ %s%s\n' "${C_GREEN}" "$1" "${C_RESET}"; }
skip() { printf '%s   ⏭  SKIP: %s%s\n' "${C_YELLOW}" "$1" "${C_RESET}"; }
warn() { printf '%s   ! %s%s\n' "${C_YELLOW}" "$1" "${C_RESET}"; }
die()  { printf '%s   ✘ ERROR: %s%s\n' "${C_RED}${C_BOLD}" "$1" "${C_RESET}" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

TOTAL_STEPS=5

# Track what happened for the final summary.
SUMMARY=()
record() { SUMMARY+=("$1"); }

printf '%sLingXi clients — one-command setup%s\n' "${C_BOLD}" "${C_RESET}"
info "repo root : ${REPO_ROOT}"
info "clients   : ${CLIENTS_DIR}"
info "platform  : $(uname -s) $(uname -m)"

# ---------------------------------------------------------------------------
# Preflight — the REQUIRED core toolchain (engine + the two npm packages).
# Optional mobile toolchains are checked inline in their own steps.
# ---------------------------------------------------------------------------
have cargo || die "cargo not found. Install the Rust toolchain (https://rustup.rs) and re-run."
have node  || die "node not found. Install Node.js >= 18 (https://nodejs.org) and re-run."
have npm   || die "npm not found. Install Node.js (bundles npm) and re-run."

# ===========================================================================
# Step 1 — Engine for Electron: build the bridge-server binary.
# ===========================================================================
header "Engine: cargo build -p bridge-server"
info "workspace: ${CARGO_DIR}"
info "(electron's resolveServerBin finds it at target/debug/bridge-server)"
( cd "${CARGO_DIR}" && cargo build -p bridge-server --bin bridge-server )
if [ -x "${BRIDGE_BIN}" ]; then
  ok "bridge-server built → ${BRIDGE_BIN}"
  record "engine     : bridge-server built (debug)"
else
  die "cargo reported success but ${BRIDGE_BIN} is missing."
fi

# ===========================================================================
# Step 2 — Shared SDK: npm install + BUILD (must happen before electron).
# ===========================================================================
header "Shared SDK: npm install && npm run build  (REQUIRED before electron)"
info "dir: ${SHARED_DIR}"
( cd "${SHARED_DIR}" && npm install )
( cd "${SHARED_DIR}" && npm run build )
if [ -f "${SHARED_DIR}/dist/index.js" ]; then
  ok "@lingxi/bridge-client built → ${SHARED_DIR}/dist"
  record "shared SDK : installed + built (dist/ present)"
else
  die "shared build finished but ${SHARED_DIR}/dist/index.js is missing — electron's file:../shared dep would be unresolved."
fi

# ===========================================================================
# Step 3 — Electron shell: npm install (consumes the freshly built shared dist).
# ===========================================================================
header "Electron: npm install"
info "dir: ${ELECTRON_DIR}"
( cd "${ELECTRON_DIR}" && npm install )
ok "electron deps installed (links @lingxi/bridge-client from clients/shared)"
record "electron   : npm install complete"

# ===========================================================================
# Step 4 — Android bindings (OPTIONAL): cargo-ndk + Android NDK required.
# ===========================================================================
header "Android runtime and bindings: build-mobile-linux-native.sh  (optional)"
ANDROID_NDK_DETECTED=""
for candidate in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_ROOT:-}" "${NDK_HOME:-}"; do
  if [ -n "${candidate}" ] && [ -d "${candidate}" ]; then ANDROID_NDK_DETECTED="${candidate}"; break; fi
done
# Fall back to an NDK installed under $ANDROID_HOME/ndk/<version>.
if [ -z "${ANDROID_NDK_DETECTED}" ] && [ -n "${ANDROID_HOME:-}" ] && [ -d "${ANDROID_HOME}/ndk" ]; then
  ANDROID_NDK_DETECTED="$(ls -d "${ANDROID_HOME}/ndk"/*/ 2>/dev/null | sort -V | tail -1 || true)"
  ANDROID_NDK_DETECTED="${ANDROID_NDK_DETECTED%/}"
fi

if have cargo-ndk && [ -n "${ANDROID_NDK_DETECTED}" ]; then
  info "cargo-ndk : $(command -v cargo-ndk)"
  info "NDK       : ${ANDROID_NDK_DETECTED}"
  export ANDROID_NDK_HOME="${ANDROID_NDK_DETECTED}"
  ( cd "${ANDROID_DIR}" && bash scripts/build-mobile-linux-native.sh --variant play )
  ok "Android JNI libs, Kotlin bindings and native SDK support generated"
  record "android    : JNI libs + bindings + native SDK support built"
else
  if ! have cargo-ndk; then
    skip "cargo-ndk not found. Install it:  cargo install cargo-ndk  — then re-run."
  fi
  if [ -z "${ANDROID_NDK_DETECTED}" ]; then
    skip "Android NDK not found. Install it (Android Studio → SDK Manager → NDK), set ANDROID_NDK_HOME (or ANDROID_HOME with ndk/), then re-run."
  fi
  record "android    : SKIPPED (cargo-ndk + Android NDK required)"
fi

# ===========================================================================
# Step 5 — iOS framework (OPTIONAL): macOS + xcodebuild + xcodegen required.
# ===========================================================================
header "iOS framework: build-xcframework.sh + xcodegen generate  (optional)"
if [ "$(uname -s)" = "Darwin" ] && have xcodebuild && have xcodegen; then
  info "xcodebuild: $(xcodebuild -version 2>/dev/null | head -1)"
  info "xcodegen  : $(xcodegen --version 2>/dev/null | head -1)"
  ( cd "${IOS_DIR}" && bash scripts/build-xcframework.sh )
  ( cd "${IOS_DIR}" && xcodegen generate )
  ok "LingxiCodeFFI.xcframework built + Xcode project generated"
  record "ios        : xcframework + xcodeproj generated"
else
  if [ "$(uname -s)" != "Darwin" ]; then
    skip "Not macOS — iOS builds require macOS with Xcode. Skipping."
  else
    if ! have xcodebuild; then
      skip "xcodebuild not found. Install Xcode + Command Line Tools (xcode-select --install), then re-run."
    fi
    if ! have xcodegen; then
      skip "xcodegen not found. Install it:  brew install xcodegen  — then re-run."
    fi
  fi
  record "ios        : SKIPPED (macOS + xcodebuild + xcodegen required)"
fi

# ===========================================================================
# Summary + NEXT STEPS
# ===========================================================================
printf '\n%s========================================================================%s\n' "${C_GREEN}${C_BOLD}" "${C_RESET}"
printf '%sSETUP COMPLETE%s\n' "${C_GREEN}${C_BOLD}" "${C_RESET}"
printf '%s========================================================================%s\n' "${C_GREEN}${C_BOLD}" "${C_RESET}"
for line in "${SUMMARY[@]}"; do printf '   %s%s%s\n' "${C_DIM}" "${line}" "${C_RESET}"; done

cat <<EOF

${C_BOLD}NEXT STEPS${C_RESET}
  • Desktop (Electron):   ${C_BOLD}cd ${ELECTRON_DIR} && npm run dev${C_RESET}
                          then open Settings → enter your Anthropic API key → pick a model.
  • Android:              open ${ANDROID_DIR} in Android Studio, build & run.
                          (re-run this script first if the Android step was skipped)
  • iOS:                  open ${IOS_DIR}/LingxiCode.xcodeproj in Xcode, build & run.
                          (re-run this script first if the iOS step was skipped)

  The LLM API key is entered in-app (Settings) or via ANTHROPIC_API_KEY in the
  environment — it is never committed or baked into any artifact.
EOF
