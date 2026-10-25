/** Shared outlined wordmark exported from assets/brand/identity-v1. */
export function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <span className={`product-brand${compact ? ' product-brand-compact' : ''}`} role="img" aria-label="灵犀 LingXi">
      <img className="product-brand-light" src="/brand/lockup-bilingual-ink.svg" alt="" width={compact ? 174 : 410} height={compact ? 40 : 94} />
      <img className="product-brand-dark" src="/brand/lockup-bilingual-white.svg" alt="" width={compact ? 174 : 410} height={compact ? 40 : 94} />
    </span>
  );
}
