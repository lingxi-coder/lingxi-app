/**
 * Where the engine keeps the sessions and the memory of a working directory:
 * `<config home>/projects/<projectDirName(cwd)>/`. A port of the engine's
 * `project_dir_name` (harness-runtime `crates/session/src/jsonl/path.rs`): every
 * UTF-16 unit that is not an ASCII letter or digit becomes `-`, and a result
 * longer than 200 characters is cut there and followed by `-` and the base 36
 * form of the absolute value of the djb2 hash of the whole path.
 */

const MAX_SANITIZED_LENGTH = 200;

function djb2(value: string): number {
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) hash = ((hash << 5) - hash + value.charCodeAt(index)) | 0;
  return hash;
}

export function projectDirName(cwd: string): string {
  let sanitized = '';
  for (let index = 0; index < cwd.length; index += 1) {
    const unit = cwd.charCodeAt(index);
    const alphanumeric = (unit >= 0x30 && unit <= 0x39) || (unit >= 0x41 && unit <= 0x5a) || (unit >= 0x61 && unit <= 0x7a);
    sanitized += alphanumeric ? String.fromCharCode(unit) : '-';
  }
  if (sanitized.length <= MAX_SANITIZED_LENGTH) return sanitized;
  return `${sanitized.slice(0, MAX_SANITIZED_LENGTH)}-${Math.abs(djb2(cwd)).toString(36)}`;
}
