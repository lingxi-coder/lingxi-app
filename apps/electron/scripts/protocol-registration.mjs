import { chmodSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export const WINDOWS_PROTOCOL_SCRIPT = String.raw`param([ValidateSet('Register','Unregister')][string]$Action = 'Register')
$ErrorActionPreference = 'Stop'
$Executable = Join-Path $PSScriptRoot 'LingXi Code.exe'
if (!(Test-Path -LiteralPath $Executable -PathType Leaf)) { throw 'Keep this script beside LingXi Code.exe.' }
$Key = 'HKCU:\Software\Classes\lingxi'
$Command = '"' + $Executable + '" "%1"'
if (Test-Path -LiteralPath $Key) {
  $Current = (Get-ItemProperty -LiteralPath ($Key + '\shell\open\command') -ErrorAction SilentlyContinue).'(default)'
  if ($Current -ne $Command) { throw 'Another installation owns lingxi:. Unregister it first.' }
}
if ($Action -eq 'Unregister') {
  if (Test-Path -LiteralPath $Key) { Remove-Item -LiteralPath $Key -Recurse -Force }
  Write-Output 'Removed this user protocol registration. The application files remain.'
  exit 0
}
New-Item -Path ($Key + '\shell\open\command') -Force | Out-Null
Set-Item -LiteralPath $Key -Value 'URL:LingXi Protocol'
New-ItemProperty -LiteralPath $Key -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
Set-Item -LiteralPath ($Key + '\shell\open\command') -Value $Command
Write-Output 'Registered lingxi: for the current user. Keep this application at its current path.'
`;

export const LINUX_PROTOCOL_SCRIPT = String.raw`#!/bin/sh
set -eu
action="${'${'}1:-register}"
case "$action" in register|unregister) ;; *) echo 'Usage: ./register-protocol.sh [register|unregister]' >&2; exit 2;; esac
app_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
executable="$app_dir/lingxi-code"
[ -x "$executable" ] || { echo 'Keep this script beside executable lingxi-code.' >&2; exit 1; }
data_home="${'${'}XDG_DATA_HOME:-$HOME/.local/share}"
config_home="${'${'}XDG_CONFIG_HOME:-$HOME/.config}"
for user_path in "$data_home" "$config_home"; do
  case "$user_path" in /*) ;; *) echo 'XDG paths must be absolute.' >&2; exit 1;; esac
done
# Desktop Entry Exec supports field codes and quoting; literal percent signs
# must be doubled and shell metacharacters escaped inside quoted paths.
escaped=$(printf '%s' "$executable" | sed 's/\\/\\\\/g; s/"/\\"/g; s/${'`'}/\\${'`'}/g; s/\$/\\$/g; s/%/%%/g')
case "$executable" in *'
'*) echo 'Application paths containing newlines are unsupported.' >&2; exit 1;; esac
applications="$data_home/applications"
desktop="$applications/lingxi-code.desktop"
backup="$applications/.lingxi-code.previous-handler"
if [ -f "$desktop" ] && ! grep -Fxq "X-LingXi-Executable=$executable" "$desktop"; then
  echo 'Another installation owns lingxi-code.desktop. Unregister it first.' >&2; exit 1
fi
if [ "$action" = unregister ]; then
  if [ -f "$desktop" ]; then
    current=$(xdg-mime query default x-scheme-handler/lingxi || true)
    if [ "$current" = lingxi-code.desktop ]; then
      previous=$(cat "$backup" 2>/dev/null || true)
      if [ -n "$previous" ] && [ "$previous" != lingxi-code.desktop ]; then
        xdg-mime default "$previous" x-scheme-handler/lingxi
      fi
    fi
    rm -f -- "$desktop" "$backup"
    # Remove only our desktop ID; keep every unrelated MIME association.
    for mimeapps in "$config_home/mimeapps.list" "$applications/mimeapps.list"; do
      if [ -f "$mimeapps" ]; then
        awk '/^x-scheme-handler\/lingxi=/ { count = split(substr($0, index($0, "=") + 1), handlers, ";"); value = ""; for (i = 1; i <= count; i++) if (handlers[i] != "" && handlers[i] != "lingxi-code.desktop") value = value handlers[i] ";"; if (value != "") print "x-scheme-handler/lingxi=" value; next } { print }' "$mimeapps" > "$mimeapps.lingxi-tmp"
        mv -- "$mimeapps.lingxi-tmp" "$mimeapps"
      fi
    done
  fi
else
  command -v xdg-mime >/dev/null 2>&1 || { echo 'xdg-mime is required.' >&2; exit 1; }
  mkdir -p -- "$applications"
  if [ ! -f "$desktop" ]; then xdg-mime query default x-scheme-handler/lingxi > "$backup" || :; fi
  printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=LingXi Code' "Exec=\"$escaped\" %u" 'Terminal=false' 'NoDisplay=true' 'MimeType=x-scheme-handler/lingxi;' "X-LingXi-Executable=$executable" > "$desktop"
  xdg-mime default lingxi-code.desktop x-scheme-handler/lingxi
fi
if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$applications"; fi
printf '%s\n' "LingXi protocol $action completed for the current user."
`;

export function writeProtocolRegistration(payloadRoot, platform) {
  if (platform === 'win32') writeFileSync(join(payloadRoot, 'register-protocol.ps1'), WINDOWS_PROTOCOL_SCRIPT, 'utf8');
  else if (platform === 'linux') {
    const script = join(payloadRoot, 'register-protocol.sh');
    writeFileSync(script, LINUX_PROTOCOL_SCRIPT, 'utf8');
    chmodSync(script, 0o755);
  }
}
export function verifyProtocolRegistration(payloadRoot, platform) {
  const name = platform === 'win32' ? 'register-protocol.ps1' : 'register-protocol.sh';
  const expected = platform === 'win32' ? WINDOWS_PROTOCOL_SCRIPT : LINUX_PROTOCOL_SCRIPT;
  if (readFileSync(join(payloadRoot, name), 'utf8') !== expected) throw new Error('Packaged protocol registration helper is missing or stale');
}
