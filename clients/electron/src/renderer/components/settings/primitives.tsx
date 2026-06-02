import { useState, type ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';
import { Icon } from '../Icon';

export interface SelectOption {
  id: string;
  label: string;
  dot?: string;
  disabled?: boolean;
}

export function Segmented({
  value, onChange, options,
}: {
  value: string;
  onChange: (id: string) => void;
  options: SelectOption[];
}) {
  const t = useT();
  return (
    <div style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
      {options.map((o) => {
        const active = value === o.id;
        return (
          <button
            key={o.id}
            onClick={() => onChange(o.id)}
            disabled={o.disabled}
            style={{
              padding: '5px 14px', borderRadius: 7, border: 'none', cursor: o.disabled ? 'not-allowed' : 'pointer',
              background: active ? t.surface : 'transparent',
              color: active ? t.text : o.disabled ? t.text4 : t.text3,
              fontSize: 12.5, fontWeight: active ? 600 : 500, fontFamily: 'inherit',
              boxShadow: active ? '0 1px 2px rgba(0,0,0,0.06), 0 0 0 0.5px ' + t.border : 'none',
              transition: 'all 0.12s',
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export function Toggle({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  const t = useT();
  return (
    <button
      onClick={() => onChange(!value)}
      style={{
        width: 38, height: 22, borderRadius: 99, border: 'none', cursor: 'pointer', padding: 0,
        background: value ? t.accent : t.surfaceActive,
        position: 'relative', transition: 'background 0.15s', flexShrink: 0,
      }}
    >
      <span
        style={{
          position: 'absolute', top: 2, left: value ? 18 : 2,
          width: 18, height: 18, borderRadius: '50%', background: '#fff',
          transition: 'left 0.15s', boxShadow: '0 1px 3px rgba(0,0,0,0.2)',
        }}
      />
    </button>
  );
}

export function SettingsSelect({
  value, options, onChange, placeholder,
}: {
  value: string;
  options: SelectOption[];
  onChange: (id: string) => void;
  placeholder?: string;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const selected = options.find((o) => o.id === value);
  return (
    <div style={{ position: 'relative', minWidth: 260 }}>
      <button
        onClick={() => setOpen(!open)}
        style={{
          width: '100%', display: 'flex', alignItems: 'center', gap: 8,
          padding: '8px 12px', borderRadius: 8, cursor: 'pointer',
          background: t.surface, border: `0.5px solid ${t.border}`,
          color: t.text, fontSize: 13, fontFamily: 'inherit', textAlign: 'left',
        }}
        onMouseEnter={(e) => (e.currentTarget.style.borderColor = t.borderStrong)}
        onMouseLeave={(e) => (e.currentTarget.style.borderColor = t.border)}
      >
        <span style={{ flex: 1 }}>{selected ? selected.label : placeholder}</span>
        <Icon name="chevron" size={13} color={t.text3} stroke={2} />
      </button>
      {open && (
        <>
          <div onClick={() => setOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
          <div
            style={{
              position: 'absolute', top: 'calc(100% + 4px)', left: 0, right: 0, zIndex: 50,
              background: t.surface, border: `0.5px solid ${t.borderStrong}`,
              borderRadius: 10, padding: 4,
              boxShadow: '0 12px 32px rgba(0,0,0,0.25)',
              animation: 'fade-in 0.15s ease',
            }}
          >
            {options.map((o) => (
              <div
                key={o.id}
                onClick={() => {
                  onChange(o.id);
                  setOpen(false);
                }}
                style={{
                  display: 'flex', alignItems: 'center', gap: 8,
                  padding: '7px 10px', borderRadius: 6, cursor: 'pointer',
                  background: o.id === value ? t.surfaceHover : 'transparent',
                  color: t.text, fontSize: 13,
                }}
                onMouseEnter={(e) => {
                  if (o.id !== value) e.currentTarget.style.background = t.surfaceHover;
                }}
                onMouseLeave={(e) => {
                  if (o.id !== value) e.currentTarget.style.background = 'transparent';
                }}
              >
                {o.dot && <span style={{ width: 12, height: 12, borderRadius: 3, background: o.dot, flexShrink: 0, border: `0.5px solid ${t.border}` }} />}
                <span style={{ flex: 1 }}>{o.label}</span>
                {o.id === value && <Icon name="check" size={12} color={t.accent} stroke={2.5} />}
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

export function SettingsRow({ title, desc, children }: { title: ReactNode; desc?: ReactNode; children: ReactNode }) {
  const t = useT();
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 24, padding: '18px 0', borderBottom: `0.5px solid ${t.border}` }}>
      <div style={{ flex: 1 }}>
        <div style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{title}</div>
        {desc && <div style={{ fontSize: 12.5, color: t.text3, marginTop: 4, lineHeight: 1.5, maxWidth: 620 }}>{desc}</div>}
      </div>
      <div style={{ flexShrink: 0, paddingTop: 1 }}>{children}</div>
    </div>
  );
}

export function SectionTitle({ children }: { children: ReactNode }) {
  const t = useT();
  return <div style={{ fontSize: 16, fontWeight: 600, color: t.text, paddingTop: 32, paddingBottom: 4 }}>{children}</div>;
}

export const CODE_THEMES: { light: SelectOption[]; dark: SelectOption[] } = {
  light: [
    { id: 'lingxi-light', label: 'Lingxi Light', dot: '#fbf9f5' },
    { id: 'gh-light', label: 'GitHub Light', dot: '#ffffff' },
    { id: 'solarized', label: 'Solarized Light', dot: '#fdf6e3' },
    { id: 'paper', label: 'Paper', dot: '#f5f1e8' },
  ],
  dark: [
    { id: 'lingxi-dark', label: 'Lingxi Dark', dot: '#15131a' },
    { id: 'midnight', label: 'Midnight', dot: '#0a0a0e' },
    { id: 'tokyo', label: 'Tokyo Night', dot: '#1a1b26' },
    { id: 'monokai', label: 'Monokai', dot: '#272822' },
  ],
};

export function CodePreview({ theme }: { theme: 'dark' | 'light' }) {
  const c =
    theme === 'dark'
      ? {
          bg: '#15131a', text: '#dfd9eb', mute: '#7a7286',
          kw: '#c084e8', fn: '#7dd3a3', str: '#e8a872', name: '#8db8e8',
          addBg: '#0e2818', delBg: '#2a1418', addText: '#a5e8b8', delText: '#e8a5a5',
        }
      : {
          bg: '#fbf9f5', text: '#2c2a32', mute: '#8a8294',
          kw: '#7a3fb8', fn: '#1f7a4d', str: '#a85a1e', name: '#3460a8',
          addBg: '#e1f5e7', delBg: '#fde2e2', addText: '#1a6a36', delText: '#9a2828',
        };
  const lines: { n: number; t: 'ctx' | 'add' | 'del'; text: ReactNode }[] = [
    {
      n: 1,
      t: 'ctx',
      text: (
        <>
          <span style={{ color: c.kw }}>function</span> <span style={{ color: c.fn }}>greet</span>(<span style={{ color: c.name }}>name</span>:{' '}
          <span style={{ color: c.kw }}>string</span>) {'{'}
        </>
      ),
    },
    {
      n: 2,
      t: 'del',
      text: (
        <>
          {'  '}
          <span style={{ color: c.kw }}>return</span> <span style={{ color: c.str }}>&quot;Hello, &quot;</span> + <span style={{ color: c.name }}>name</span>;
        </>
      ),
    },
    {
      n: 2,
      t: 'add',
      text: (
        <>
          {'  '}
          <span style={{ color: c.kw }}>return</span> <span style={{ color: c.str }}>{'`Hello, ${name}!`'}</span>;
        </>
      ),
    },
    { n: 3, t: 'ctx', text: '}' },
  ];
  return (
    <div
      className="mono"
      style={{
        background: c.bg, color: c.text,
        borderRadius: 10, overflow: 'hidden',
        fontSize: 12.5, lineHeight: 1.75, flex: 1, minWidth: 0,
        border: theme === 'light' ? '0.5px solid rgba(0,0,0,0.08)' : 'none',
      }}
    >
      {lines.map((l, i) => {
        const bg = l.t === 'add' ? c.addBg : l.t === 'del' ? c.delBg : 'transparent';
        const sigil = l.t === 'add' ? '+' : l.t === 'del' ? '−' : ' ';
        const sigilColor = l.t === 'add' ? c.addText : l.t === 'del' ? c.delText : c.mute;
        return (
          <div key={i} style={{ display: 'flex', background: bg }}>
            <span style={{ width: 38, textAlign: 'right', color: c.mute, padding: '0 10px', flexShrink: 0 }}>{l.n}</span>
            <span style={{ width: 16, color: sigilColor, flexShrink: 0, fontWeight: 600 }}>{sigil}</span>
            <span style={{ paddingRight: 14, whiteSpace: 'pre' }}>{l.text}</span>
          </div>
        );
      })}
    </div>
  );
}
