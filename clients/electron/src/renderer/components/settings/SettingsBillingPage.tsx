import { useT } from '../../theme/ThemeContext';
import { SectionTitle, SettingsRow } from './primitives';

export function SettingsBillingPage() {
  const t = useT();
  const link = t.link || t.accent2 || t.accent;
  const invoices = [
    { date: 'May 11, 2026', total: '$109.48', status: 'Paid' },
    { date: 'May 8, 2026', total: '$90.69', status: 'Paid' },
    { date: 'Apr 22, 2026', total: '$20.00', status: 'Paid' },
  ];
  return (
    <div>
      <SectionTitle>Plan</SectionTitle>
      <div style={{ display: 'flex', alignItems: 'center', gap: 18, padding: '18px 0', borderBottom: `0.5px solid ${t.border}` }}>
        <div
          style={{
            width: 56, height: 56, borderRadius: 12,
            background: `linear-gradient(135deg, ${t.accent}, ${t.accent2})`,
            color: '#fff', display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0,
          }}
        >
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="5" r="1.8" fill="currentColor" />
            <circle cx="5" cy="12" r="1.5" fill="currentColor" />
            <circle cx="19" cy="12" r="1.5" fill="currentColor" />
            <circle cx="8" cy="19" r="1.3" fill="currentColor" />
            <circle cx="16" cy="19" r="1.3" fill="currentColor" />
            <path d="M12 6.5v5M6.4 12.6 11 13M17.6 12.6 13 13M9 18l2.5-4M15 18l-2.5-4" />
          </svg>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 15, fontWeight: 600, color: t.text }}>Lingxi Max</div>
          <div style={{ fontSize: 12.5, color: t.text3, marginTop: 3 }}>20× more usage than Pro</div>
          <div style={{ fontSize: 12.5, color: t.text4, marginTop: 2 }}>Your subscription will auto renew on Jun 11, 2026.</div>
        </div>
        <button
          style={{
            padding: '8px 14px', borderRadius: 8,
            background: t.surface, color: t.text,
            border: `0.5px solid ${t.border}`, cursor: 'pointer',
            fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = t.surface)}
        >
          Adjust plan
        </button>
      </div>

      <SectionTitle>Payment</SectionTitle>
      <div style={{ display: 'flex', alignItems: 'center', gap: 24, padding: '18px 0', borderBottom: `0.5px solid ${t.border}` }}>
        <div style={{ flex: 1, display: 'flex', alignItems: 'center', gap: 12 }}>
          <div style={{ width: 36, height: 26, borderRadius: 5, background: '#1a1f36', position: 'relative', overflow: 'hidden', flexShrink: 0 }}>
            <span style={{ position: 'absolute', top: 6, left: 6, width: 14, height: 14, borderRadius: '50%', background: '#eb001b' }} />
            <span style={{ position: 'absolute', top: 6, right: 6, width: 14, height: 14, borderRadius: '50%', background: '#f79e1b', opacity: 0.95 }} />
            <span style={{ position: 'absolute', top: 6, left: 13, width: 10, height: 14, borderRadius: '50%', background: '#ff5f00' }} />
          </div>
          <span style={{ fontSize: 13.5, color: t.text }}>Mastercard •••• 2526</span>
        </div>
        <button
          style={{
            padding: '8px 14px', borderRadius: 8,
            background: t.surface, color: t.text,
            border: `0.5px solid ${t.border}`, cursor: 'pointer',
            fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = t.surface)}
        >
          Update
        </button>
      </div>

      <SectionTitle>Invoices</SectionTitle>
      <div style={{ paddingTop: 14 }}>
        <div
          style={{
            display: 'grid', gridTemplateColumns: '1.4fr 1fr 1fr 0.6fr', gap: 12,
            padding: '10px 4px', fontSize: 12, color: t.text3, fontWeight: 500,
            borderBottom: `0.5px solid ${t.border}`,
          }}
        >
          <span>Date</span>
          <span>Total</span>
          <span>Status</span>
          <span style={{ textAlign: 'right' }}>Actions</span>
        </div>
        {invoices.map((inv, i) => (
          <div
            key={i}
            style={{
              display: 'grid', gridTemplateColumns: '1.4fr 1fr 1fr 0.6fr', gap: 12,
              padding: '14px 4px', fontSize: 13, color: t.text,
              borderBottom: i < invoices.length - 1 ? `0.5px solid ${t.border}` : 'none',
              alignItems: 'center',
            }}
          >
            <span>{inv.date}</span>
            <span style={{ color: t.text }}>{inv.total}</span>
            <span style={{ color: t.text2 }}>{inv.status}</span>
            <a
              style={{
                color: link, textAlign: 'right', textDecoration: 'underline',
                textDecorationColor: `color-mix(in oklab, ${link} 40%, transparent)`,
                textUnderlineOffset: 3, cursor: 'pointer', fontWeight: 500,
              }}
            >
              View
            </a>
          </div>
        ))}
      </div>

      <SectionTitle>Cancellation</SectionTitle>
      <SettingsRow title="Cancel plan" desc="You'll keep your Max plan features until Jun 11, 2026, then return to the free tier.">
        <button
          style={{
            padding: '8px 16px', borderRadius: 8,
            background: t.danger, color: '#fff',
            border: 'none', cursor: 'pointer',
            fontSize: 12.5, fontWeight: 600, fontFamily: 'inherit',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(0.92)')}
          onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
        >
          Cancel
        </button>
      </SettingsRow>
      <div style={{ height: 60 }} />
    </div>
  );
}
