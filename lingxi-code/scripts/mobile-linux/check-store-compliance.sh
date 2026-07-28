#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../.." && pwd)"
declare -a scan_paths=(
  "${repo_root}/clients/android"
  "${repo_root}/clients/ios"
  "${repo_root}/lingxi-code/apps/android-aar"
  "${repo_root}/lingxi-code/apps/ios-framework"
  "${repo_root}/lingxi-code/platforms/android"
  "${repo_root}/lingxi-code/platforms/ios"
)

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
check_pattern "Android privilege escalation" 'Shizuku|android\\.permission\\.BIND_ACCESSIBILITY_SERVICE|AccessibilityService|ACTION_MANAGE_OVERLAY_PERMISSION|SYSTEM_ALERT_WINDOW'
check_pattern "Android battery bypass" 'ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS|REQUEST_IGNORE_BATTERY_OPTIMIZATIONS'
check_pattern "Android fake media foreground service" 'foregroundServiceType\\s*=\\s*"mediaPlayback"|foregroundServiceType\\s*=\\s*".*mediaPlayback.*"|mediaPlayback'
check_pattern "iOS background audio mode" 'UIBackgroundModes|<string>audio</string>|setCategory\\(\\.playback|setCategory\\([^\\n]*\\.playback|category\\s*=\\s*\\.playback'

echo "store-compliance scan passed"
