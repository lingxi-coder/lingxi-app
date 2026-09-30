/** The existing LingXi Möbius geometry, simplified for a clear web wordmark. */
export function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <span
      className={`product-brand${compact ? ' product-brand-compact' : ''}`}
      style={{ display: 'inline-flex', alignItems: 'center', gap: `var(--brand-gap, ${compact ? 9 : 18}px)`, color: 'var(--brand-color, #4267ed)' }}
    >
      <svg
        className="product-brand-mark"
        viewBox="140 300 744 424"
        width={compact ? 41 : 101}
        height={compact ? 25 : 62}
        fill="none"
        aria-hidden="true"
      >
        <path
          d="M226 512C304 346 440 348 512 512C584 676 720 678 798 512C720 346 584 348 512 512C440 676 304 678 226 512Z"
          stroke="currentColor"
          strokeWidth="100"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
      <span style={{ display: 'inline-flex', alignItems: 'baseline', gap: `var(--brand-text-gap, ${compact ? 9 : 15}px)` }}>
        <span className="product-brand-name" style={{ fontSize: `var(--brand-name-size, ${compact ? 24 : 68}px)`, fontWeight: 640, letterSpacing: '-0.055em', lineHeight: 1.1 }}>LingXi</span>
        <span className="product-brand-chinese" style={{ fontSize: `var(--brand-chinese-size, ${compact ? 11 : 17}px)`, fontWeight: 550, letterSpacing: '.04em', opacity: .65 }}>灵犀</span>
      </span>
    </span>
  );
}
