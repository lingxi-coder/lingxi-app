import { useState } from 'react';
import { useT } from '../../theme/ThemeContext';
import { Icon } from '../Icon';
import { SectionTitle, SettingsRow, Toggle } from './primitives';

export function SettingsPrivacyPage() {
  const t = useT();
  const [locMeta, setLocMeta] = useState(true);
  const [improve, setImprove] = useState(true);
  const link = t.link || t.accent2 || t.accent;
  const linkStyle = {
    color: link, textDecoration: 'underline' as const,
    textDecorationColor: `color-mix(in oklab, ${link} 40%, transparent)`,
    textUnderlineOffset: 3, cursor: 'pointer', fontWeight: 500,
  };

  const DataRow = ({ label, last }: { label: string; last?: boolean }) => (
    <button
      style={{
        width: '100%', display: 'flex', alignItems: 'center', gap: 8,
        padding: '14px 16px', background: 'transparent',
        border: 'none', borderTop: `0.5px solid ${t.border}`,
        cursor: 'pointer', textAlign: 'left', fontFamily: 'inherit',
        borderBottomLeftRadius: last ? 11 : 0, borderBottomRightRadius: last ? 11 : 0,
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
    >
      <span style={{ flex: 1, fontSize: 13.5, color: t.text }}>{label}</span>
      <Icon name="chevron" size={13} color={t.text3} stroke={2} style={{ transform: 'rotate(-90deg)' }} />
    </button>
  );

  const btnStyle = {
    padding: '8px 14px', borderRadius: 8,
    background: t.surface, color: t.text,
    border: `0.5px solid ${t.border}`, cursor: 'pointer',
    fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
  };

  return (
    <div>
      <div style={{ height: 24 }} />

      <div style={{ background: t.surface, border: `0.5px solid ${t.border}`, borderRadius: 12, overflow: 'hidden' }}>
        <div style={{ padding: '16px 18px', fontSize: 13, color: t.text2, lineHeight: 1.6 }}>
          Lingxi is built on transparent data practices. Learn how your workspaces, chats and code are protected when using Lingxi, and visit our{' '}
          <a style={linkStyle}>Trust Center</a> and <a style={linkStyle}>Privacy Policy</a> for details.
        </div>
        <DataRow label="How we protect your data" />
        <DataRow label="How we use your data" last />
      </div>

      <SectionTitle>Preferences</SectionTitle>
      <SettingsRow
        title="Location metadata"
        desc={
          <>
            Allow Lingxi to use coarse location metadata (city/region) to improve product experiences. <a style={linkStyle}>Learn more</a>.
          </>
        }
      >
        <Toggle value={locMeta} onChange={setLocMeta} />
      </SettingsRow>
      <SettingsRow
        title="Help improve Lingxi"
        desc={
          <>
            Allow the use of your chats and coding sessions to train and improve Lingxi models. <a style={linkStyle}>Learn more</a>.
          </>
        }
      >
        <Toggle value={improve} onChange={setImprove} />
      </SettingsRow>
      <SettingsRow
        title="Workspace telemetry"
        desc="Anonymous usage metrics about features and performance — no chat content, no code. Used only to fix bugs and improve UX."
      >
        <Toggle value={true} onChange={() => {}} />
      </SettingsRow>

      <SectionTitle>Your data</SectionTitle>
      <SettingsRow title="Export data" desc="Download a copy of your chats, projects, sessions and uploaded files as a .zip archive.">
        <button
          style={btnStyle}
          onMouseEnter={(e) => (e.currentTarget.style.borderColor = t.borderStrong)}
          onMouseLeave={(e) => (e.currentTarget.style.borderColor = t.border)}
        >
          Export data
        </button>
      </SettingsRow>
      <SettingsRow title="Shared chats" desc="View and revoke public links you've created to share conversations or artifacts.">
        <button
          style={btnStyle}
          onMouseEnter={(e) => (e.currentTarget.style.borderColor = t.borderStrong)}
          onMouseLeave={(e) => (e.currentTarget.style.borderColor = t.border)}
        >
          Manage
        </button>
      </SettingsRow>
      <SettingsRow title="Memory preferences" desc="What Lingxi remembers across sessions — project facts, style preferences, recent files.">
        <button
          style={{ ...btnStyle, display: 'inline-flex', alignItems: 'center', gap: 6 }}
          onMouseEnter={(e) => (e.currentTarget.style.borderColor = t.borderStrong)}
          onMouseLeave={(e) => (e.currentTarget.style.borderColor = t.border)}
        >
          Manage
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke={t.text3} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M7 17 17 7M9 7h8v8" />
          </svg>
        </button>
      </SettingsRow>
      <SettingsRow title="Delete account" desc="Permanently remove your account, all workspaces and stored data. This cannot be undone.">
        <button
          style={{
            padding: '8px 14px', borderRadius: 8,
            background: 'transparent', color: t.danger,
            border: `0.5px solid color-mix(in oklab, ${t.danger} 40%, transparent)`,
            cursor: 'pointer', fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
          }}
        >
          Delete account
        </button>
      </SettingsRow>

      <div style={{ height: 60 }} />
    </div>
  );
}
