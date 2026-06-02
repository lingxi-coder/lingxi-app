import { useState, type ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';
import type { ThemeMode } from '../../theme/tokens';
import { Icon } from '../Icon';
import { SettingsCodePage } from './SettingsCodePage';
import { SettingsAccountPage } from './SettingsAccountPage';
import { SettingsGeneralPage } from './SettingsGeneralPage';
import { SettingsPrivacyPage } from './SettingsPrivacyPage';
import { SettingsBillingPage } from './SettingsBillingPage';
import { SettingsUsagePage } from './SettingsUsagePage';
import { SettingsGenericPage } from './SettingsGenericPage';

interface NavItem {
  id: string;
  label: string;
  tag?: string;
}

const SETTINGS_NAV: { group: string | null; items: NavItem[] }[] = [
  {
    group: null,
    items: [
      { id: 'general', label: 'General' },
      { id: 'account', label: 'Account' },
      { id: 'privacy', label: 'Privacy' },
      { id: 'billing', label: 'Billing' },
      { id: 'usage', label: 'Usage' },
      { id: 'capabilities', label: 'Capabilities' },
      { id: 'connectors', label: 'Connectors' },
      { id: 'code', label: 'Lingxi Code' },
      { id: 'cowork', label: 'Cowork' },
      { id: 'chrome', label: 'Lingxi in Chrome', tag: 'Beta' },
    ],
  },
  {
    group: 'Desktop app',
    items: [
      { id: 'dt-extensions', label: 'Extensions' },
      { id: 'dt-developer', label: 'Developer' },
    ],
  },
];

export function SettingsPage({
  open, onClose, theme, setTheme,
}: {
  open: boolean;
  onClose: () => void;
  theme: ThemeMode;
  setTheme: (m: ThemeMode) => void;
}) {
  const t = useT();
  const [tab, setTab] = useState('general');
  if (!open) return null;

  let content: ReactNode;
  if (tab === 'code') content = <SettingsCodePage />;
  else if (tab === 'account') content = <SettingsAccountPage />;
  else if (tab === 'general') content = <SettingsGeneralPage theme={theme} setTheme={setTheme} />;
  else if (tab === 'privacy') content = <SettingsPrivacyPage />;
  else if (tab === 'billing') content = <SettingsBillingPage />;
  else if (tab === 'usage') content = <SettingsUsagePage />;
  else if (tab === 'capabilities')
    content = <SettingsGenericPage title="Capabilities" blurb="Enable or disable tools: web, code execution, file editing, MCP servers." />;
  else if (tab === 'connectors')
    content = <SettingsGenericPage title="Connectors" blurb="GitHub, Linear, Notion, Slack, Drive, and custom MCP servers." />;
  else if (tab === 'cowork')
    content = <SettingsGenericPage title="Cowork" blurb="Real-time collaboration: presence, cursors, shared canvases." />;
  else if (tab === 'chrome')
    content = <SettingsGenericPage title="Lingxi in Chrome" blurb="Browser extension (Beta) — page-level context, screenshots, form-fill." />;
  else if (tab === 'dt-extensions') content = <SettingsGenericPage title="Extensions" blurb="Local plugins and CLI integrations." />;
  else if (tab === 'dt-developer') content = <SettingsGenericPage title="Developer" blurb="Verbose logs, devtools, API key management." />;

  return (
    <div
      style={{
        position: 'absolute', inset: 0, zIndex: 90,
        background: t.windowBg, display: 'flex', flexDirection: 'column',
        animation: 'fade-in 0.18s ease',
      }}
    >
      {/* Title bar */}
      <div style={{ height: 56, flexShrink: 0, padding: '0 22px 0 88px', display: 'flex', alignItems: 'center', gap: 14, borderBottom: `0.5px solid ${t.border}` }}>
        <button
          onClick={onClose}
          style={{
            width: 28, height: 28, borderRadius: 7, border: 'none', cursor: 'pointer',
            background: 'transparent', color: t.text2,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="chevronL" size={18} stroke={1.8} />
        </button>
        <span style={{ fontSize: 16, fontWeight: 600, color: t.text }}>Settings</span>
        <div style={{ flex: 1 }} />
      </div>

      {/* Body */}
      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        {/* Left nav */}
        <div style={{ width: 240, flexShrink: 0, padding: '20px 14px', borderRight: `0.5px solid ${t.border}`, overflowY: 'auto' }}>
          {SETTINGS_NAV.map((g, gi) => (
            <div key={gi} style={{ marginBottom: 6 }}>
              {g.group && (
                <div style={{ fontSize: 11, color: t.text4, fontWeight: 600, letterSpacing: 0.5, textTransform: 'uppercase', padding: '14px 10px 6px' }}>
                  {g.group}
                </div>
              )}
              {g.items.map((it) => {
                const active = tab === it.id;
                return (
                  <div
                    key={it.id}
                    onClick={() => setTab(it.id)}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 8,
                      padding: '7px 10px', borderRadius: 7, cursor: 'pointer',
                      background: active ? t.surfaceActive : 'transparent',
                      color: active ? t.text : t.text2,
                      fontSize: 13, fontWeight: active ? 600 : 500,
                      transition: 'background 0.12s',
                    }}
                    onMouseEnter={(e) => {
                      if (!active) e.currentTarget.style.background = t.surfaceHover;
                    }}
                    onMouseLeave={(e) => {
                      if (!active) e.currentTarget.style.background = 'transparent';
                    }}
                  >
                    <span style={{ flex: 1 }}>{it.label}</span>
                    {it.tag && (
                      <span style={{ fontSize: 10, padding: '1px 6px', borderRadius: 4, background: t.surfaceHover, color: t.text3, fontWeight: 600 }}>
                        {it.tag}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>

        {/* Content */}
        <div style={{ flex: 1, overflowY: 'auto', padding: '12px 56px 60px', minWidth: 0 }}>
          <div style={{ maxWidth: 920, margin: '0 auto' }}>{content}</div>
        </div>
      </div>
    </div>
  );
}
