#!/usr/bin/env bash
set -euo pipefail

mode="${1:-play}"
native_library="${2:-}"
case "${mode}" in
  play|direct) ;;
  *) echo "usage: $0 [play|direct] [libandroid_aar.so]" >&2; exit 2 ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../.." && pwd)"
runtime_root="$(python3 "${script_dir}/../runtime_source.py" --root)"
declare -a scan_paths=(
  "${repo_root}/clients/android"
  "${repo_root}/clients/ios"
  "${repo_root}/lingxi-code/apps/android-aar"
  "${repo_root}/lingxi-code/apps/ios-framework"
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
  if output="$("${base_rg[@]}" -e "${pattern}" "$@" "${scan_paths[@]}" 2>/dev/null)"; then
    echo "forbidden ${label} pattern detected:" >&2
    echo "${output}" >&2
    exit 1
  fi
}

check_pattern "Android self-update" 'REQUEST_INSTALL_PACKAGES|PackageInstaller|installPackage\\(|DexClassLoader|PathClassLoader|InMemoryDexClassLoader|pm install'
check_pattern "Android broad package visibility" 'QUERY_ALL_PACKAGES'
if [[ "${mode}" == "play" ]]; then
  check_pattern "Android privilege escalation" 'Shizuku|android\\.permission\\.BIND_ACCESSIBILITY_SERVICE|AccessibilityService|ACTION_MANAGE_OVERLAY_PERMISSION|SYSTEM_ALERT_WINDOW|FOREGROUND_SERVICE_MEDIA_PROJECTION|FOREGROUND_SERVICE_SPECIAL_USE'
else
  check_pattern "Android forbidden privilege escalation" 'Shizuku|ACTION_MANAGE_OVERLAY_PERMISSION|SYSTEM_ALERT_WINDOW'
  direct_manifest="${repo_root}/clients/android/app/src/direct/AndroidManifest.xml"
  [[ -f "${direct_manifest}" ]] || { echo "missing Direct manifest" >&2; exit 1; }
  rg -q 'BIND_ACCESSIBILITY_SERVICE' "${direct_manifest}" \
    || { echo "Direct manifest is missing AccessibilityService binding" >&2; exit 1; }
  rg -q 'foregroundServiceType="mediaProjection\\|specialUse"' "${direct_manifest}" \
    || { echo "Direct manifest is missing the expected foreground service types" >&2; exit 1; }
fi
check_pattern "Android battery bypass" 'ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS|REQUEST_IGNORE_BATTERY_OPTIMIZATIONS'
check_pattern "Android fake media foreground service" 'foregroundServiceType\\s*=\\s*"mediaPlayback"|foregroundServiceType\\s*=\\s*".*mediaPlayback.*"|mediaPlayback'
check_pattern "iOS background audio mode" 'UIBackgroundModes|<string>audio</string>|setCategory\\(\\.playback|setCategory\\([^\\n]*\\.playback|category\\s*=\\s*\\.playback'

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
