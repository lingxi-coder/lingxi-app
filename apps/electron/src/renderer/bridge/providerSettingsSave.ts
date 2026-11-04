export function equalProviderSettings(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  const a = Object.keys(left), b = Object.keys(right);
  return a.length === b.length && a.every((key) => Object.prototype.hasOwnProperty.call(right, key)
    && equalProviderSettings((left as Record<string, unknown>)[key], (right as Record<string, unknown>)[key]));
}
