import { useState, type ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';
import { Toggle } from './primitives';

export function SettingsUsagePage() {
  const t = useT();
  const [credits, setCredits] = useState(false);
  const link = t.link || t.accent2 || t.accent;

  const Bar = ({ pct }: { pct: number }) => (
    <div style={{ flex: 1, height: 6, borderRadius: 99, background: t.surfaceActive, overflow: 'hidden' }}>
      <div style={{ width: `${pct}%`, height: '100%', background: t.accent, borderRadius: 99 }} />
    </div>
  );

  const Row = ({ name, sub, pct, right, info }: { name: string; sub?: string; pct: number; right: string; info?: string }) => (
    <div style={{ display: 'grid', gridTemplateColumns: '1fr 1.6fr auto', gap: 24, alignItems: 'center', padding: '14px 0' }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <span style={{ fontSize: 13.5, color: t.text, fontWeight: 500 }}>{name}</span>
          {info && (
            <span
              style={{
                width: 14, height: 14, borderRadius: 99,
                border: `1px solid ${t.border}`, color: t.text4,
                fontSize: 9, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                fontStyle: 'italic', fontFamily: 'Georgia, serif', fontWeight: 600, cursor: 'help',
              }}
              title={info}
            >
              i
            </span>
          )}
        </div>
        {sub && <div style={{ fontSize: 12, color: t.text4, marginTop: 3 }}>{sub}</div>}
      </div>
      <Bar pct={pct} />
      <span style={{ fontSize: 12.5, color: t.text2, minWidth: 56, textAlign: 'right' }}>{right}</span>
    </div>
  );

  const linkAnchor = (children: ReactNode) => (
    <a
      style={{
        display: 'inline-block', fontSize: 13, color: link,
        textDecoration: 'underline',
        textDecorationColor: `color-mix(in oklab, ${link} 40%, transparent)`,
        textUnderlineOffset: 3, cursor: 'pointer', fontWeight: 500,
      }}
    >
      {children}
    </a>
  );

  return (
    <div>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, paddingTop: 32, paddingBottom: 4 }}>
        <span style={{ fontSize: 16, fontWeight: 600, color: t.text }}>Plan usage limits</span>
        <span style={{ fontSize: 13, color: t.text3 }}>Max (20×)</span>
      </div>

      <div style={{ paddingTop: 10, borderBottom: `0.5px solid ${t.border}`, paddingBottom: 10 }}>
        <Row name="Current session" sub="Resets in 1 hr 3 min" pct={60} right="60% used" />
      </div>

      <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, paddingTop: 32, paddingBottom: 4 }}>
        <span style={{ fontSize: 16, fontWeight: 600, color: t.text }}>Weekly limits</span>
      </div>
      <div style={{ marginTop: 14, marginBottom: 6 }}>{linkAnchor('Learn more about usage limits')}</div>

      <div style={{ paddingTop: 4 }}>
        <Row name="All models" sub="Resets Thu 11:59 AM" pct={51} right="51% used" />
        <Row name="Lingxi 4.7" sub="Resets Thu 12:00 PM" pct={9} right="9% used" info="Per-model weekly quota for Lingxi 4.7." />
        <Row name="Lingxi Design" sub="Resets Thu 12:00 PM" pct={14} right="14% used" info="Quota for design-mode generations." />
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, paddingTop: 12, paddingBottom: 12 }}>
        <span style={{ fontSize: 12, color: t.text4 }}>Last updated: 3 minutes ago</span>
        <button
          style={{
            width: 20, height: 20, borderRadius: 99, border: 'none', background: 'transparent', cursor: 'pointer',
            display: 'inline-flex', alignItems: 'center', justifyContent: 'center', color: t.text3,
          }}
          title="Refresh"
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <path d="M3 12a9 9 0 1 0 3-6.7" />
            <path d="M3 4v5h5" />
          </svg>
        </button>
      </div>

      <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, paddingTop: 24, paddingBottom: 4 }}>
        <span style={{ fontSize: 16, fontWeight: 600, color: t.text }}>Additional features</span>
      </div>
      <div style={{ paddingTop: 10 }}>
        <Row
          name="Daily included routine runs"
          sub="You haven't run any routines yet"
          pct={0.5}
          right="0 / 15"
          info="Scheduled background routines included with your Max plan."
        />
      </div>

      <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, paddingTop: 28, paddingBottom: 4 }}>
        <span style={{ fontSize: 16, fontWeight: 600, color: t.text }}>Usage credits</span>
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 24, padding: '18px 0', borderBottom: `0.5px solid ${t.border}` }}>
        <div style={{ flex: 1, fontSize: 13, color: t.text, lineHeight: 1.55 }}>
          Turn on usage credits to keep using Lingxi if you hit a limit. {linkAnchor('Learn more')}
        </div>
        <Toggle value={credits} onChange={setCredits} />
      </div>
      <div style={{ height: 60 }} />
    </div>
  );
}
