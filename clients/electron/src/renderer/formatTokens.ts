/** Decimal units (1M = 1,000,000), matching the catalog limits and the Android client. */
export function formatTokens(value: number): string {
  if (value < 1_000) return String(Math.round(value));
  const thousands = Number((value / 1_000).toFixed(1));
  return thousands < 1_000 ? `${thousands}k` : `${Number((value / 1_000_000).toFixed(1))}M`;
}
