#!/bin/bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
ELECTRON_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly ELECTRON_DIR
REPO_ROOT="$(cd "${ELECTRON_DIR}/../.." && pwd)"
readonly REPO_ROOT
readonly ENGINE_DIR="${REPO_ROOT}"
readonly PROVISIONING_PROJECT="${ELECTRON_DIR}/macos-signing/ProvisioningBootstrap.xcodeproj"

readonly FLARE_TEAM_ID="AZ4AX7J833"
readonly FLARE_CERTIFICATE_NAME="Apple Development: lingfeng luo (KQ7KX8LCYL)"

channel="${LINGXI_CREDENTIAL_BROKER_CHANNEL:-development}"
launch_after_build=false
preflight_only=false
print_config=false
auto_register=true

usage() {
  cat <<'EOF'
Usage: package-macos-flare.sh [--channel development|production] [--check] [--launch] [--no-register] [--print-config]

Build, sign, and verify the LingXi macOS Electron app with the Flare App, Inc.
Apple Development identity. The script discovers matching installed macOS
provisioning profiles and asks Xcode Automatic Signing to create missing ones
unless explicit paths are supplied through:

  LINGXI_MAC_PROVISIONING_PROFILE
  LINGXI_MAC_BROKER_PROVISIONING_PROFILE
  LINGXI_MAC_AUDIO_PROVISIONING_PROFILE

Options:
  --channel CHANNEL  Credential namespace to package (default: development)
  --check            Validate signing assets without building
  --launch           Launch the verified packaged app
  --no-register      Do not let Xcode create or download missing profiles
  --print-config     Print the required identifiers and exit
  -h, --help         Show this help
EOF
}

fail() {
  printf '[package:mac:flare] ERROR: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '[package:mac:flare] %s\n' "$*"
}

while (($# > 0)); do
  case "$1" in
    --channel)
      (($# >= 2)) || fail '--channel requires development or production'
      channel="$2"
      shift 2
      ;;
    --check)
      preflight_only=true
      shift
      ;;
    --launch)
      launch_after_build=true
      shift
      ;;
    --no-register)
      auto_register=false
      shift
      ;;
    --print-config)
      print_config=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

case "$channel" in
  development)
    readonly desktop_bundle_id="com.lingxi.code.development"
    readonly broker_bundle_id="com.lingxi.code.credential-broker.development"
    readonly audio_bundle_id="com.lingxi.code.audio-helper.development"
    ;;
  production)
    readonly desktop_bundle_id="com.lingxi.code"
    readonly broker_bundle_id="com.lingxi.code.credential-broker"
    readonly audio_bundle_id="com.lingxi.code.audio-helper"
    ;;
  *)
    fail "unsupported channel: ${channel}"
    ;;
esac

if [[ "$print_config" == true ]]; then
  printf 'team_id=%s\n' "$FLARE_TEAM_ID"
  printf 'certificate=%s\n' "$FLARE_CERTIFICATE_NAME"
  printf 'channel=%s\n' "$channel"
  printf 'desktop_bundle_id=%s\n' "$desktop_bundle_id"
  printf 'broker_bundle_id=%s\n' "$broker_bundle_id"
  printf 'audio_bundle_id=%s\n' "$audio_bundle_id"
  exit 0
fi

[[ "$(uname -s)" == "Darwin" ]] || fail 'macOS is required'
[[ "$(uname -m)" == "arm64" ]] || fail 'an Apple silicon Mac is required'

for required_command in /usr/bin/codesign /usr/bin/security /usr/bin/xcrun cargo node npm; do
  command -v "$required_command" >/dev/null 2>&1 || fail "required command is unavailable: ${required_command}"
done

configured_team_id="${LINGXI_MAC_TEAM_ID:-$FLARE_TEAM_ID}"
[[ "$configured_team_id" == "$FLARE_TEAM_ID" ]] || {
  fail "LINGXI_MAC_TEAM_ID must remain ${FLARE_TEAM_ID} to match the iOS Flare App, Inc. team"
}

identity_hashes="$({
  /usr/bin/security find-identity -v -p codesigning 2>/dev/null || true
} | awk -v name="$FLARE_CERTIFICATE_NAME" 'index($0, "\"" name "\"") { print $2 }')"

identity_count="$(printf '%s\n' "$identity_hashes" | awk 'NF { count += 1 } END { print count + 0 }')"
[[ "$identity_count" -eq 1 ]] || {
  fail "expected exactly one valid '${FLARE_CERTIFICATE_NAME}' identity, found ${identity_count}; open Xcode > Settings > Accounts > Flare App, Inc. > Manage Certificates"
}
identity_hash="$(printf '%s\n' "$identity_hashes" | awk 'NF { print; exit }')"

scratch_dir="$(mktemp -d "${TMPDIR:-/tmp}/lingxi-macos-signing.XXXXXX")"
cleanup() {
  rm -rf -- "$scratch_dir"
}
trap cleanup EXIT

if [[ -n "${LINGXI_MAC_PROFILE_DIRS:-}" ]]; then
  IFS=':' read -r -a profile_dirs <<<"$LINGXI_MAC_PROFILE_DIRS"
else
  profile_dirs=(
    "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"
    "$HOME/Library/MobileDevice/Provisioning Profiles"
  )
fi

profile_matches_bundle_id() {
  local profile_path="$1"
  local expected_bundle_id="$2"
  local decoded_profile="${scratch_dir}/profile.plist"

  /usr/bin/security cms -D -i "$profile_path" >"$decoded_profile" 2>/dev/null || return 1

  local profile_team
  local profile_platform
  local profile_application_id
  local profile_expiration
  profile_team="$(/usr/libexec/PlistBuddy -c 'Print :TeamIdentifier:0' "$decoded_profile" 2>/dev/null || true)"
  profile_platform="$(/usr/bin/plutil -extract Platform.0 raw -o - "$decoded_profile" 2>/dev/null || true)"
  profile_application_id="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$decoded_profile" 2>/dev/null || true)"
  profile_expiration="$(/usr/bin/plutil -extract ExpirationDate raw -o - "$decoded_profile" 2>/dev/null || true)"

  [[ "$profile_team" == "$FLARE_TEAM_ID" ]] || return 1
  [[ "$profile_platform" == "OSX" ]] || return 1
  [[ "$profile_expiration" > "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" ]] || return 1
  [[ "$profile_application_id" == "${FLARE_TEAM_ID}.${expected_bundle_id}" || \
     "$profile_application_id" == "${FLARE_TEAM_ID}.*" ]]
}

find_profile() {
  local expected_bundle_id="$1"
  local profile_dir
  local profile_path

  for profile_dir in "${profile_dirs[@]}"; do
    [[ -d "$profile_dir" ]] || continue
    while IFS= read -r -d '' profile_path; do
      if profile_matches_bundle_id "$profile_path" "$expected_bundle_id"; then
        printf '%s\n' "$profile_path"
        return 0
      fi
    done < <(find "$profile_dir" -maxdepth 1 -type f \( -name '*.provisionprofile' -o -name '*.mobileprovision' \) -print0)
  done
  return 1
}

register_profile_with_xcode() {
  local expected_bundle_id="$1"
  local output_variable="$2"
  local output_slug="$3"
  local build_root="${scratch_dir}/xcode/${output_slug}"
  local generated_profile="${build_root}/ProvisioningBootstrap.app/Contents/embedded.provisionprofile"

  [[ -d "$PROVISIONING_PROJECT" ]] || fail "provisioning bootstrap project is missing: ${PROVISIONING_PROJECT}"
  log "asking Xcode Automatic Signing to register ${expected_bundle_id}"
  /usr/bin/xcodebuild \
    -project "$PROVISIONING_PROJECT" \
    -target ProvisioningBootstrap \
    -configuration Debug \
    -sdk macosx \
    -quiet \
    -allowProvisioningUpdates \
    -allowProvisioningDeviceRegistration \
    ARCHS=arm64 \
    CODE_SIGN_STYLE=Automatic \
    CODE_SIGN_IDENTITY='Apple Development' \
    DEVELOPMENT_TEAM="$FLARE_TEAM_ID" \
    PRODUCT_BUNDLE_IDENTIFIER="$expected_bundle_id" \
    CONFIGURATION_BUILD_DIR="$build_root" \
    OBJROOT="${build_root}/Intermediates" \
    ONLY_ACTIVE_ARCH=YES \
    SYMROOT="$build_root"

  [[ -f "$generated_profile" ]] || {
    fail "Xcode signed ${expected_bundle_id} without an embedded development profile"
  }
  profile_matches_bundle_id "$generated_profile" "$expected_bundle_id" || {
    fail "Xcode generated an invalid profile for ${expected_bundle_id}"
  }
  printf -v "$output_variable" '%s' "$generated_profile"
}

desktop_profile="${LINGXI_MAC_PROVISIONING_PROFILE:-}"
if [[ -z "$desktop_profile" ]]; then
  desktop_profile="$(find_profile "$desktop_bundle_id" || true)"
fi

broker_profile="${LINGXI_MAC_BROKER_PROVISIONING_PROFILE:-}"
if [[ -z "$broker_profile" ]]; then
  broker_profile="$(find_profile "$broker_bundle_id" || true)"
fi

audio_profile="${LINGXI_MAC_AUDIO_PROVISIONING_PROFILE:-}"
if [[ -z "$audio_profile" ]]; then
  audio_profile="$(find_profile "$audio_bundle_id" || true)"
fi

if [[ -z "$desktop_profile" || -z "$broker_profile" || -z "$audio_profile" ]]; then
  if [[ "$auto_register" == true ]]; then
    [[ -n "$desktop_profile" ]] || register_profile_with_xcode "$desktop_bundle_id" desktop_profile desktop
    [[ -n "$broker_profile" ]] || register_profile_with_xcode "$broker_bundle_id" broker_profile broker
    [[ -n "$audio_profile" ]] || register_profile_with_xcode "$audio_bundle_id" audio_profile audio
  fi
fi

if [[ -z "$desktop_profile" || -z "$broker_profile" || -z "$audio_profile" ]]; then
  cat >&2 <<EOF
[package:mac:flare] ERROR: matching Mac App Development provisioning profiles are not installed.

Create or download profiles for the Flare App, Inc. team (${FLARE_TEAM_ID}):
  Desktop: ${desktop_bundle_id}
  Broker:  ${broker_bundle_id}
  Audio:   ${audio_bundle_id}

Install them with Xcode, or provide their paths through
LINGXI_MAC_PROVISIONING_PROFILE, LINGXI_MAC_BROKER_PROVISIONING_PROFILE,
and LINGXI_MAC_AUDIO_PROVISIONING_PROFILE.
The iOS Xcode Managed Profiles cannot sign these macOS bundles.
Remove --no-register to let Xcode Automatic Signing create them.
EOF
  exit 1
fi

[[ -f "$desktop_profile" ]] || fail "Desktop provisioning profile is missing: ${desktop_profile}"
[[ -f "$broker_profile" ]] || fail "Broker provisioning profile is missing: ${broker_profile}"
[[ -f "$audio_profile" ]] || fail "Audio Helper provisioning profile is missing: ${audio_profile}"

export LINGXI_CODESIGN_IDENTITY="$identity_hash"
export LINGXI_MAC_TEAM_ID="$FLARE_TEAM_ID"
export LINGXI_MAC_PROVISIONING_PROFILE="$desktop_profile"
export LINGXI_MAC_BROKER_PROVISIONING_PROFILE="$broker_profile"
export LINGXI_MAC_AUDIO_PROVISIONING_PROFILE="$audio_profile"
export LINGXI_CREDENTIAL_BROKER_CHANNEL="$channel"

(
  cd "$ELECTRON_DIR"
  node --input-type=module - "$desktop_profile" "$broker_profile" "$audio_profile" "$FLARE_TEAM_ID" "$channel" <<'NODE'
import { audioHelperIdentifiers } from './scripts/audio-helper.mjs';
import { brokerIdentifiers, validateProvisioningProfile } from './scripts/credential-broker.mjs';

const [desktopProfile, brokerProfile, audioProfile, teamId, channel] = process.argv.slice(2);
const identifiers = brokerIdentifiers(channel);
const audioIdentifiers = audioHelperIdentifiers(channel);
validateProvisioningProfile(desktopProfile, teamId, identifiers.desktopBundleId);
validateProvisioningProfile(brokerProfile, teamId, identifiers.brokerBundleId);
validateProvisioningProfile(audioProfile, teamId, audioIdentifiers.bundleId);
NODE
)

log "signing team: Flare App, Inc. (${FLARE_TEAM_ID})"
log "signing identity: ${FLARE_CERTIFICATE_NAME} (${identity_hash})"
log "Desktop profile: ${desktop_profile}"
log "Broker profile: ${broker_profile}"
log "Audio Helper profile: ${audio_profile}"
log "credential channel: ${channel}"

if [[ "$preflight_only" == true ]]; then
  log 'signing preflight passed'
  exit 0
fi

repo_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
remap_flags="--remap-path-prefix=${REPO_ROOT}=. --remap-path-prefix=${repo_cargo_home}=/cargo-home --remap-path-prefix=${HOME}/.rustup=/rustup"
export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }${remap_flags}"

log 'building portable release bridge-server'
(
  cd "$ENGINE_DIR"
  cargo build --locked --release -p bridge-server --bin bridge-server
)

log 'packaging and signing the Electron app'
(
  cd "$ELECTRON_DIR"
  npm run package:mac
  npm run verify:package
)

package_version="$(node -p "require('${ELECTRON_DIR}/package.json').version")"
app_path="${ELECTRON_DIR}/dist/LingXi-Code-${package_version}-mac-arm64/LingXi Code.app"
[[ -d "$app_path" ]] || fail "verified app is missing: ${app_path}"

log "verified app: ${app_path}"
if [[ "$launch_after_build" == true ]]; then
  /usr/bin/open "$app_path"
  log 'launched packaged Desktop app'
fi
