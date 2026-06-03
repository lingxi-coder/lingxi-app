import { useState, type ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';
import type { ThemeMode } from '../../theme/tokens';
import { Icon } from '../Icon';
import { SectionTitle, SettingsSelect, Toggle } from './primitives';

type Appearance = ThemeMode | 'system';

export function SettingsGeneralPage({ theme, setTheme }: { theme: ThemeMode; setTheme: (m: ThemeMode) => void }) {
  const t = useT();
  const [fullName, setFullName] = useState('lingfeng luo');
  const [nickname, setNickname] = useState('lingfeng');
  const [role, setRole] = useState('');
  const [instructions, setInstructions] = useState('');
  const [chatFont, setChatFont] = useState('lingxi-sans');
  const [voice, setVoice] = useState('mellow');
  const [voiceSpeed, setVoiceSpeed] = useState('fast');
  const [notif, setNotif] = useState({
    responseDone: true, codeUpdates: true, permission: true, email: false, dispatch: true,
  });
  const setN = (k: keyof typeof notif, v: boolean) => setNotif((n) => ({ ...n, [k]: v }));

  const [runOnStartup, setRunOnStartup] = useState(false);
  const [quickShortcut, setQuickShortcut] = useState('opt2');
  const [voiceShortcut, setVoiceShortcut] = useState('caps');
  const [menuBar, setMenuBar] = useState(true);
  const [keepAwake, setKeepAwake] = useState(true);
  const [allowBrowser, setAllowBrowser] = useState(true);
  const [computerUse, setComputerUse] = useState(true);
  const [unhide, setUnhide] = useState(true);

  const [appearance, setAppearance] = useState<Appearance>(theme);
  const applyAppearance = (mode: Appearance) => {
    setAppearance(mode);
    if (mode === 'light' || mode === 'dark') setTheme(mode);
    else if (typeof window !== 'undefined' && window.matchMedia) {
      const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
      setTheme(prefersDark ? 'dark' : 'light');
    }
  };

  const link = t.link || t.accent2 || t.accent;
  const linkStyle = {
    color: link, textDecoration: 'underline' as const,
    textDecorationColor: `color-mix(in oklab, ${link} 40%, transparent)`,
    textUnderlineOffset: 3, cursor: 'pointer', fontWeight: 500,
  };
  const inputStyle = {
    width: 260, padding: '7px 12px', borderRadius: 8,
    background: t.surface, border: `0.5px solid ${t.border}`,
    color: t.text, fontSize: 13, fontFamily: 'inherit', outline: 'none',
  };
  const pillStyle = { fontSize: 11, padding: '3px 9px', borderRadius: 5, background: t.surfaceHover, color: t.text3, fontWeight: 600 };
  const ROW_PAD = '16px 0';
  const titleStyle = { fontSize: 14, fontWeight: 500, color: t.text, lineHeight: 1.35 };
  const descStyle = { fontSize: 12.5, color: t.text3, marginTop: 4, lineHeight: 1.5, maxWidth: 620 };

  const Row = ({
    title, desc, badge, children, align = 'start',
  }: {
    title: ReactNode;
    desc?: ReactNode;
    badge?: string;
    children: ReactNode;
    align?: 'start' | 'center';
  }) => (
    <div
      style={{
        display: 'flex', alignItems: align === 'center' ? 'center' : 'flex-start',
        gap: 24, padding: ROW_PAD, borderBottom: `0.5px solid ${t.border}`,
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={titleStyle}>{title}</span>
          {badge && <span style={pillStyle}>{badge}</span>}
        </div>
        {desc && <div style={descStyle}>{desc}</div>}
      </div>
      <div style={{ flexShrink: 0, paddingTop: align === 'center' ? 0 : 1 }}>{children}</div>
    </div>
  );

  const BlockRow = ({
    title, desc, action, children,
  }: {
    title: ReactNode;
    desc?: ReactNode;
    action?: ReactNode;
    children?: ReactNode;
  }) => (
    <div style={{ padding: ROW_PAD, borderBottom: `0.5px solid ${t.border}` }}>
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 24 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={titleStyle}>{title}</div>
          {desc && <div style={descStyle}>{desc}</div>}
        </div>
        {action && <div style={{ flexShrink: 0 }}>{action}</div>}
      </div>
      {children && <div style={{ marginTop: 12 }}>{children}</div>}
    </div>
  );

  const AppearanceBtn = ({ mode, children }: { mode: Appearance; children: ReactNode }) => {
    const active = appearance === mode;
    return (
      <button
        onClick={() => applyAppearance(mode)}
        title={mode}
        style={{
          width: 32, height: 26, padding: 0,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
          border: active ? `0.5px solid ${t.border}` : '0.5px solid transparent',
          background: active ? t.surface : 'transparent',
          color: active ? t.text : t.text3,
          borderRadius: 7, cursor: 'pointer',
          boxShadow: active ? '0 1px 2px rgba(0,0,0,0.06)' : 'none',
          transition: 'all 0.12s',
        }}
        onMouseEnter={(e) => {
          if (!active) e.currentTarget.style.color = t.text2;
        }}
        onMouseLeave={(e) => {
          if (!active) e.currentTarget.style.color = t.text3;
        }}
      >
        {children}
      </button>
    );
  };

  const GhostBtn = ({ children, leading }: { children: ReactNode; leading?: ReactNode }) => (
    <button
      style={{
        display: 'inline-flex', alignItems: 'center', gap: 6,
        padding: '6px 12px', borderRadius: 7,
        background: t.surface, border: `0.5px solid ${t.border}`,
        color: t.text2, fontSize: 12, fontWeight: 500, fontFamily: 'inherit', cursor: 'pointer',
      }}
      onMouseEnter={(e) => (e.currentTarget.style.borderColor = t.borderStrong)}
      onMouseLeave={(e) => (e.currentTarget.style.borderColor = t.border)}
    >
      {leading}
      {children}
    </button>
  );

  return (
    <div>
      <SectionTitle>Profile</SectionTitle>

      <Row title="Avatar" align="center">
        <div
          style={{
            width: 32, height: 32, borderRadius: 99,
            background: t.surfaceActive, color: t.text,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            fontSize: 12, fontWeight: 600, letterSpacing: 0.5,
          }}
        >
          LL
        </div>
      </Row>

      <Row title="Full name" align="center">
        <input value={fullName} onChange={(e) => setFullName(e.target.value)} style={inputStyle} />
      </Row>

      <Row title="What should Lingxi call you?" align="center">
        <input value={nickname} onChange={(e) => setNickname(e.target.value)} style={inputStyle} />
      </Row>

      <Row title="What best describes your work?" align="center">
        <SettingsSelect
          value={role}
          placeholder="Select"
          onChange={setRole}
          options={[
            { id: 'eng', label: 'Engineering / Software' },
            { id: 'design', label: 'Design' },
            { id: 'pm', label: 'Product management' },
            { id: 'data', label: 'Data / Research' },
            { id: 'writing', label: 'Writing' },
            { id: 'ops', label: 'Operations' },
            { id: 'edu', label: 'Education' },
            { id: 'other', label: 'Other' },
          ]}
        />
      </Row>

      <BlockRow
        title="Instructions for Lingxi"
        desc={
          <>
            Lingxi will keep these in mind across chats and Cowork sessions within <a style={linkStyle}>Lingxi&apos;s guidelines</a>.{' '}
            <a style={linkStyle}>Learn more</a>
          </>
        }
      >
        <textarea
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
          placeholder="e.g. keep explanations brief and to the point"
          style={{
            width: '100%', padding: '12px 14px', borderRadius: 10,
            background: t.surface, border: `0.5px solid ${t.border}`,
            color: t.text, fontSize: 13, fontFamily: 'inherit', outline: 'none',
            minHeight: 88, resize: 'vertical', lineHeight: 1.55, boxSizing: 'border-box',
          }}
        />
      </BlockRow>

      <SectionTitle>Preferences</SectionTitle>

      <Row title="Appearance" align="center">
        <div style={{ display: 'inline-flex', padding: 2, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
          <AppearanceBtn mode="system">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <rect x="3" y="4" width="18" height="13" rx="2" />
              <path d="M8 21h8M12 17v4" />
            </svg>
          </AppearanceBtn>
          <AppearanceBtn mode="light">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
            </svg>
          </AppearanceBtn>
          <AppearanceBtn mode="dark">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
            </svg>
          </AppearanceBtn>
        </div>
      </Row>

      <Row title="Chat font" align="center">
        <SettingsSelect
          value={chatFont}
          onChange={setChatFont}
          options={[
            { id: 'lingxi-sans', label: 'Lingxi Sans' },
            { id: 'lingxi-serif', label: 'Lingxi Serif' },
            { id: 'system', label: 'System default' },
            { id: 'mono', label: 'Monospace' },
          ]}
        />
      </Row>

      <Row title="Voice" align="center">
        <SettingsSelect
          value={voice}
          onChange={setVoice}
          options={[
            { id: 'mellow', label: 'Mellow' },
            { id: 'bright', label: 'Bright' },
            { id: 'calm', label: 'Calm' },
            { id: 'crisp', label: 'Crisp' },
          ]}
        />
      </Row>

      <Row title="Voice speed" align="center">
        <SettingsSelect
          value={voiceSpeed}
          onChange={setVoiceSpeed}
          options={[
            { id: 'slow', label: 'Slow' },
            { id: 'normal', label: 'Normal' },
            { id: 'fast', label: 'Fast' },
            { id: 'turbo', label: 'Turbo' },
          ]}
        />
      </Row>

      <SectionTitle>Notifications</SectionTitle>

      <Row title="Response completions" desc="Get notified when Lingxi has finished a response. Useful for long-running tasks.">
        <Toggle value={notif.responseDone} onChange={(v) => setN('responseDone', v)} />
      </Row>
      <Row title="Code notifications" desc="Lingxi can choose to notify you about important updates from a Code session.">
        <Toggle value={notif.codeUpdates} onChange={(v) => setN('codeUpdates', v)} />
      </Row>
      <Row title="Code permission requests" desc="Get a push notification when Lingxi needs your approval to run a command in a Code session.">
        <Toggle value={notif.permission} onChange={(v) => setN('permission', v)} />
      </Row>
      <Row title="Emails from Lingxi Code on the web" desc="Get an email when Lingxi Code on the web has finished building or needs your response.">
        <Toggle value={notif.email} onChange={(v) => setN('email', v)} />
      </Row>
      <Row title="Dispatch messages" desc="Get a push notification on your phone when Lingxi messages you in Dispatch.">
        <Toggle value={notif.dispatch} onChange={(v) => setN('dispatch', v)} />
      </Row>

      <SectionTitle>General desktop settings</SectionTitle>

      <Row title="Run on startup" desc="Automatically start Lingxi when you log in to your computer.">
        <Toggle value={runOnStartup} onChange={setRunOnStartup} />
      </Row>

      <Row title="Quick access shortcut" desc="Message Lingxi from anywhere on your desktop.">
        <SettingsSelect
          value={quickShortcut}
          onChange={setQuickShortcut}
          options={[
            { id: 'opt2', label: 'Tap Option twice' },
            { id: 'cmd2', label: 'Tap Command twice' },
            { id: 'ctrl2', label: 'Tap Control twice' },
            { id: 'shift2', label: 'Tap Shift twice' },
            { id: 'off', label: 'Off' },
          ]}
        />
      </Row>

      <Row
        title="Voice shortcut"
        desc={<>Speak to Lingxi from anywhere on your desktop. Press once to start dictation, and press again when you&apos;re done speaking.</>}
      >
        <SettingsSelect
          value={voiceShortcut}
          onChange={setVoiceShortcut}
          options={[
            { id: 'caps', label: 'Caps Lock' },
            { id: 'fn', label: 'Fn' },
            { id: 'rcmd', label: 'Right ⌘' },
            { id: 'ropt', label: 'Right ⌥' },
            { id: 'off', label: 'Off' },
          ]}
        />
      </Row>

      <Row title="Menu bar" desc="Show Lingxi in the menu bar.">
        <Toggle value={menuBar} onChange={setMenuBar} />
      </Row>

      <Row
        title="Keep computer awake"
        desc={
          <>
            Prevent your computer from idle-sleeping while Lingxi is open so scheduled tasks can run. Your display can still turn off. Closing the laptop lid
            will still put it to sleep.
          </>
        }
      >
        <Toggle value={keepAwake} onChange={setKeepAwake} />
      </Row>

      <SectionTitle>Browser use</SectionTitle>

      <Row
        title="Allow all browser actions"
        desc={
          <>
            Lingxi will browse and interact with any website in Chrome without asking. Applies to new sessions. This setting can put your data at risk.{' '}
            <a style={linkStyle}>Learn more</a>
          </>
        }
      >
        <Toggle value={allowBrowser} onChange={setAllowBrowser} />
      </Row>

      <BlockRow title="Connected browsers" desc="Chrome instances signed in to your account that Lingxi can automate.">
        <div style={{ border: `0.5px solid ${t.border}`, borderRadius: 10, background: t.surface, overflow: 'hidden' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px' }}>
            <span style={{ flex: 1, fontSize: 13, color: t.text }}>No browsers connected</span>
            <GhostBtn
              leading={
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M3 12a9 9 0 1 0 3-6.7" />
                  <path d="M3 4v5h5" />
                </svg>
              }
            >
              Recheck
            </GhostBtn>
          </div>
          <div style={{ padding: '10px 14px', fontSize: 12, color: t.text3, borderTop: `0.5px solid ${t.border}`, background: t.sidebarBg }}>
            No Chrome instances are connected. Open Chrome with the Lingxi extension and sign in.
          </div>
        </div>
      </BlockRow>

      <Row
        title="Computer use"
        badge="Beta"
        desc={
          <>
            Let Lingxi take screenshots and control your keyboard and mouse in apps you allow. <a style={linkStyle}>Learn more</a>
          </>
        }
      >
        <Toggle value={computerUse} onChange={setComputerUse} />
      </Row>

      <Row title="Unhide apps when Lingxi finishes" desc="Apps hidden during a task are restored when Lingxi stops.">
        <Toggle value={unhide} onChange={setUnhide} />
      </Row>

      <BlockRow
        title="Denied apps"
        desc={
          <>
            Any request Lingxi makes to access these apps is automatically rejected. Lingxi may still affect them indirectly through actions in allowed apps.
          </>
        }
        action={
          <GhostBtn>
            Add app <Icon name="chevron" size={11} color={t.text3} stroke={2} />
          </GhostBtn>
        }
      >
        <div style={{ padding: '12px 14px', fontSize: 12, color: t.text3, border: `0.5px dashed ${t.border}`, borderRadius: 10, background: t.surface }}>
          No apps denied. Add an app to automatically reject Lingxi&apos;s requests for it.
        </div>
      </BlockRow>

      <Row title="Accessibility" align="center">
        <span style={pillStyle}>Granted</span>
      </Row>

      <Row title="Screen recording" align="center">
        <span style={pillStyle}>Granted</span>
      </Row>

      <div style={{ height: 60 }} />
    </div>
  );
}
