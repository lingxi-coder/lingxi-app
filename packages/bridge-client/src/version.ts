/**
 * Semver compatibility check — the Node-side mirror of the Rust
 * `bridge::wire::version_compatible` (`bridge/src/wire.rs`).
 *
 * The rule (governing decision §0.10): two peers are compatible IFF they share
 * the same MAJOR version. A version string whose major component does not parse
 * as a base-10 integer is treated as INCOMPATIBLE (fail-closed), matching the
 * Rust guard exactly.
 */

function majorOf(version: string): number | null {
  const head = version.split('.')[0]?.trim() ?? '';
  if (head.length === 0 || !/^\d+$/.test(head)) {
    return null;
  }
  return Number.parseInt(head, 10);
}

/** `true` iff `local` and `remote` share the same major version (fail-closed). */
export function versionCompatible(local: string, remote: string): boolean {
  const a = majorOf(local);
  const b = majorOf(remote);
  if (a === null || b === null) {
    return false;
  }
  return a === b;
}
