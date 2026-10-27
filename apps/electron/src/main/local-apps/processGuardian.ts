/** A member of the app's own process group watches a parent-owned pipe. EOF
 * kills that group even after Electron is terminated without running cleanup.
 * No PID from a previous launch is loaded or signalled by the next host. */
export function guardedProcessArguments(command: string): string[] {
  return ['-c', `
group=$$
exec 4< <(
  trap '' TERM INT HUP
  trap '' USR1
  printf 'ready\\n'
  exec >/dev/null 2>&1
  while IFS= read -r line <&3; do :; done
  trap '' USR1
  kill -TERM -- "-$group" 2>/dev/null || exit 0
  sleep 0.25
  kill -KILL -- "-$group" 2>/dev/null || :
)
exec 3<&-
if ! IFS= read -r guardian_ready <&4 || [ "$guardian_ready" != ready ]; then
  exec 4<&-
  printf 'Local App process guardian failed to start\\n' >&2
  exit 1
fi
exec 4<&-
status=0
(
  exec 3<&-
  eval "$1"
) || status=$?
exit "$status"
`, 'lingxi-local-app', command];
}
