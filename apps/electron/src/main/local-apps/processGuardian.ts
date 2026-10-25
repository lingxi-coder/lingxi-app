/** A member of the app's own process group watches a parent-owned pipe. EOF
 * kills that group even after Electron is terminated without running cleanup.
 * No PID from a previous launch is loaded or signalled by the next host. */
export function guardedProcessArguments(command: string): string[] {
  return ['-c', `
group=$$
(
  trap '' TERM INT HUP
  trap 'exit 0' USR1
  while IFS= read -r line <&3; do :; done
  trap '' USR1
  kill -TERM -- "-$group" 2>/dev/null || exit 0
  sleep 0.25
  kill -KILL -- "-$group" 2>/dev/null || :
) </dev/null >/dev/null 2>&1 &
guardian=$!
trap 'kill -USR1 "$guardian" 2>/dev/null || :; wait "$guardian" 2>/dev/null || :' EXIT
status=0
eval "$1" 3<&- || status=$?
exit "$status"
`, 'lingxi-local-app', command];
}
