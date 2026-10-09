#!/usr/bin/env bash
set -euo pipefail

mode="${1:-play}"
native_library="${2:-}"
case "${mode}" in
  play|direct) ;;
  *) echo "usage: $0 [play|direct] [libandroid_aar.so]" >&2; exit 2 ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
runtime_root="$(python3 "${script_dir}/../lib/runtime_source.py" --root)"
declare -a scan_paths=(
  "${repo_root}/apps/android/native"
  "${repo_root}/apps/ios/native"
  "${repo_root}/apps/android/ffi"
  "${repo_root}/apps/ios/ffi"
  "${runtime_root}/crates/platforms/android"
  "${runtime_root}/crates/platforms/ios"
)

for scan_path in "${scan_paths[@]}"; do
  [[ -d "${scan_path}" ]] || { echo "required compliance source missing: ${scan_path}" >&2; exit 1; }
done

declare -a base_rg=(
  rg
  -n
  --hidden
  --glob=!docs/superpowers/references/**
  --glob=!**/build/**
  --glob=!**/.build/**
  --glob=!**/DerivedData/**
  --glob=!**/.git/**
)
if [[ "${mode}" == "play" ]]; then
  base_rg+=(--glob=!**/src/direct/**)
fi

check_pattern() {
  local label="$1"
  local pattern="$2"
  shift 2
  local output
  if output="$("${base_rg[@]}" -e "${pattern}" "$@" "${scan_paths[@]}" 2>&1)"; then
    echo "forbidden ${label} pattern detected:" >&2
    echo "${output}" >&2
    exit 1
  else
    local status=$?
    if [[ "${status}" != 1 ]]; then
      echo "${label} compliance scan failed (rg exit ${status}): ${output}" >&2
      exit 1
    fi
  fi
}

check_pattern "Android self-update" 'REQUEST_INSTALL_PACKAGES|PackageInstaller|installPackage\(|DexClassLoader|PathClassLoader|InMemoryDexClassLoader|pm install'
check_pattern "Android broad package visibility" 'QUERY_ALL_PACKAGES'
if [[ "${mode}" == "play" ]]; then
  check_pattern "Android privilege escalation" 'Shizuku|android\.permission\.BIND_ACCESSIBILITY_SERVICE|AccessibilityService|ACTION_MANAGE_OVERLAY_PERMISSION|SYSTEM_ALERT_WINDOW|FOREGROUND_SERVICE_MEDIA_PROJECTION'
else
  check_pattern "Android forbidden privilege escalation" 'Shizuku|ACTION_MANAGE_OVERLAY_PERMISSION|SYSTEM_ALERT_WINDOW'
fi
# specialUse is a normal FGS permission. Only reviewed service declarations and
# their exact subtype/constant locations are allowed; all privilege scans remain.
python3 "${script_dir}/check-android-special-use.py" \
  --mode "${mode}" --repo-root "${repo_root}" "${scan_paths[@]}"
check_pattern "Android battery bypass" 'ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS|REQUEST_IGNORE_BATTERY_OPTIMIZATIONS'
# Reject media FGS permissions/constants and declarations, not audio-player variable names.
check_pattern "Android fake media foreground service" 'FOREGROUND_SERVICE(_TYPE)?_MEDIA_PLAYBACK|foregroundServiceType\s*=\s*"[^"]*mediaPlayback[^"]*"'
# Validate actual background declarations; foreground playback/TTS is allowed.
python3 "${script_dir}/check-ios-background-policy.py" \
  --mode "${mode}" --repo-root "${repo_root}" "${scan_paths[@]}"

if [[ -n "${native_library}" ]]; then
  [[ -f "${native_library}" ]] \
    || { echo "native library not found: ${native_library}" >&2; exit 1; }
  if strings "${native_library}" | rg -F 'android_use' >/dev/null; then
    [[ "${mode}" == "direct" ]] \
      || { echo "Play native library unexpectedly links android_use" >&2; exit 1; }
  else
    [[ "${mode}" == "play" ]] \
      || { echo "Direct native library is missing android_use" >&2; exit 1; }
  fi
fi

echo "${mode} store-compliance scan passed"
