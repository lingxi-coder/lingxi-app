import type { CSSProperties, ReactNode } from 'react';
import type { Tokens } from '../theme/tokens';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export const Kbd = ({ children }: { children: ReactNode }) => {
  const t = useT();
  return (
    <span
      style={{
        fontSize: 10.5, color: t.text3, padding: '2px 6px', borderRadius: 5,
        background: t.surface, border: `0.5px solid ${t.border}`,
        fontFamily: '-apple-system, BlinkMacSystemFont, sans-serif', fontWeight: 500,
        lineHeight: 1,
      }}
    >
      {children}
    </span>
  );
};

export const iconBtn = (_t: Tokens): CSSProperties => ({
  width: 30, height: 30, borderRadius: 7, border: 'none', cursor: 'pointer',
  background: 'transparent', display: 'flex', alignItems: 'center', justifyContent: 'center',
  transition: 'background 0.12s',
});

export type Mode = 'chat' | 'cowork' | 'code';

export const ModeTabs = ({ mode, setMode }: { mode: Mode; setMode: (m: Mode) => void }) => {
  const t = useT();
  const tabs: { id: Mode; icon: string; label: string }[] = [
    { id: 'chat', icon: 'chat', label: 'Chat' },
    { id: 'cowork', icon: 'cowork', label: 'Cowork' },
    { id: 'code', icon: 'code', label: 'Code' },
  ];
  return (
    <div
      style={{
        display: 'flex', gap: 2, padding: 3, borderRadius: 9,
        background: t.surface, border: `0.5px solid ${t.border}`,
      }}
    >
      {tabs.map((x) => {
        const active = mode === x.id;
        return (
          <button
            key={x.id}
            onClick={() => setMode(x.id)}
            style={{
              flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 5,
              padding: '5px 8px', borderRadius: 7, border: 'none', cursor: 'pointer',
              background: active ? (x.id === 'code' ? t.windowBg : t.surfaceActive) : 'transparent',
              color: active ? t.text : t.text3,
              fontSize: 12, fontWeight: active ? 600 : 500, fontFamily: 'inherit',
              boxShadow: active ? '0 1px 2px rgba(0,0,0,0.04), 0 0 0 0.5px ' + t.border : 'none',
              transition: 'all 0.12s',
            }}
          >
            <Icon name={x.icon} size={12.5} stroke={1.9} />
            {x.label}
          </button>
        );
      })}
    </div>
  );
};

// ─── PROJECT CONTEXT MENU ICONS ────────────────────────
export const ProjectMenuIcon = ({ name, color }: { name: string; color: string }) => {
  const p = {
    width: 15, height: 15, viewBox: '0 0 24 24', fill: 'none', stroke: color,
    strokeWidth: 1.7, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const,
  };
  switch (name) {
    case 'pin': return <svg {...p}><path d="m9 4 9 9-2 2-1-1-3 3v4l-2-2-2 2v-4l3-3-1-1z" /></svg>;
    case 'finder': return <svg {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /><path d="M14 13l3-3M17 13l-3-3" /></svg>;
    case 'tree': return <svg {...p}><circle cx="6" cy="6" r="2" /><circle cx="18" cy="6" r="2" /><circle cx="12" cy="18" r="2" /><path d="M6 8v3a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V8M12 13v3" /></svg>;
    case 'rename': return <svg {...p}><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" /></svg>;
    case 'archive': return <svg {...p}><rect x="3" y="4" width="18" height="4" rx="1" /><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4" /></svg>;
    case 'x': return <svg {...p}><path d="M18 6 6 18M6 6l12 12" /></svg>;
    default: return null;
  }
};

// ─── ACCOUNT MENU ICONS ────────────────────────────
export const AccountIcon = ({ name, color, size = 16 }: { name: string; color: string; size?: number }) => {
  const p = {
    width: size, height: size, viewBox: '0 0 24 24', fill: 'none', stroke: color,
    strokeWidth: 1.7, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const,
  };
  switch (name) {
    case 'cog': return <svg {...p}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.9 2.9l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.9-2.9l.1-.1A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1A1.7 1.7 0 0 0 4.6 9a1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.9-2.9l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.9 2.9l-.1.1A1.7 1.7 0 0 0 19.4 9 1.7 1.7 0 0 0 21 10H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" /></svg>;
    case 'globe': return <svg {...p}><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>;
    case 'help': return <svg {...p}><circle cx="12" cy="12" r="9" /><path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3" /><circle cx="12" cy="17" r="0.6" fill={color} stroke="none" /></svg>;
    case 'plans': return <svg {...p}><path d="m9 11 3 3L22 4" /><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" /></svg>;
    case 'download': return <svg {...p}><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3" /></svg>;
    case 'gift': return <svg {...p}><rect x="3" y="8" width="18" height="4" rx="1" /><path d="M12 8v13M19 12v7a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-7M7.5 8a2.5 2.5 0 0 1 0-5C11 3 12 8 12 8s1-5 4.5-5a2.5 2.5 0 0 1 0 5" /></svg>;
    case 'news': return <svg {...p}><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 8h18M7 13h4M7 17h10" /></svg>;
    case 'info': return <svg {...p}><circle cx="12" cy="12" r="9" /><path d="M12 8h.01M11 12h1v5h1" /></svg>;
    case 'logout': return <svg {...p}><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9" /></svg>;
    default: return null;
  }
};

export function AccountMenuItem({
  icon, label, kbd, arrow, onClick,
}: {
  icon: string;
  label: string;
  kbd?: string;
  arrow?: boolean;
  onClick?: () => void;
}) {
  const t = useT();
  return (
    <div
      onClick={onClick}
      style={{
        display: 'flex', alignItems: 'center', gap: 12,
        padding: '7px 10px', borderRadius: 7, cursor: 'pointer',
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
    >
      <AccountIcon name={icon} color={t.text2} size={15} />
      <span style={{ flex: 1, fontSize: 13, color: t.text, fontWeight: 500 }}>{label}</span>
      {kbd && <span style={{ fontSize: 11.5, color: t.text4, fontFamily: 'inherit' }}>{kbd}</span>}
      {arrow && <Icon name="chevronR" size={13} color={t.text3} stroke={2} />}
    </div>
  );
}
