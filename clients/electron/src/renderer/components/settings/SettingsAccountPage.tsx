import { useT } from '../../theme/ThemeContext';
import { SectionTitle, SettingsRow } from './primitives';

export function SettingsAccountPage() {
  const t = useT();
  return (
    <div>
      <SectionTitle>Profile</SectionTitle>
      <div style={{ display: 'flex', alignItems: 'center', gap: 18, padding: '18px 0', borderBottom: `0.5px solid ${t.border}` }}>
        <div
          style={{
            width: 56, height: 56, borderRadius: 12,
            background: `linear-gradient(135deg, ${t.accent}, ${t.accent2})`,
            color: '#fff', display: 'flex', alignItems: 'center', justifyContent: 'center',
            fontSize: 22, fontWeight: 600,
          }}
        >
          LL
        </div>
        <div style={{ flex: 1 }}>
          <div style={{ fontSize: 15, fontWeight: 600, color: t.text }}>lingfeng</div>
          <div style={{ fontSize: 12.5, color: t.text3, marginTop: 3 }}>luolingfeng.flare@gmail.com · Max plan</div>
        </div>
        <button
          style={{
            padding: '8px 14px', borderRadius: 8,
            background: t.surface, color: t.text,
            border: `0.5px solid ${t.border}`, cursor: 'pointer',
            fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
          }}
        >
          Edit profile
        </button>
      </div>
      <SettingsRow title="Display name" desc="Shown next to your messages and on shared conversations.">
        <input
          defaultValue="lingfeng"
          style={{
            width: 220, padding: '8px 12px', borderRadius: 8,
            background: t.surface, border: `0.5px solid ${t.border}`,
            color: t.text, fontSize: 13, fontFamily: 'inherit', outline: 'none',
          }}
        />
      </SettingsRow>
      <SettingsRow title="Email" desc="Used for sign-in and account recovery.">
        <span style={{ fontSize: 13, color: t.text2 }}>luolingfeng.flare@gmail.com</span>
      </SettingsRow>
      <SettingsRow title="Sign out everywhere" desc="Revoke all active sessions on other devices.">
        <button
          style={{
            padding: '8px 14px', borderRadius: 8,
            background: 'transparent', color: t.danger,
            border: `0.5px solid color-mix(in oklab, ${t.danger} 40%, transparent)`, cursor: 'pointer',
            fontSize: 12.5, fontWeight: 500, fontFamily: 'inherit',
          }}
        >
          Sign out
        </button>
      </SettingsRow>
      <div style={{ height: 60 }} />
    </div>
  );
}
