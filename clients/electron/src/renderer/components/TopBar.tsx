import { useT } from '../theme/ThemeContext';
import type { Project } from '../data';
import type { ThemeMode } from '../theme/tokens';
import { Icon } from './Icon';
import { iconBtn } from './primitives';

interface TopBarProps {
  repo: Project;
  onTogglePanel: () => void;
  panelOpen: boolean;
  theme: ThemeMode;
  setTheme: (m: ThemeMode) => void;
  sidebarCollapsed: boolean;
}

export function TopBar({ repo, onTogglePanel, panelOpen, theme, setTheme, sidebarCollapsed }: TopBarProps) {
  const t = useT();
  return (
    <div
      style={{
        height: 44, flexShrink: 0, padding: '0 14px 0 16px',
        display: 'flex', alignItems: 'center', gap: 6,
        borderBottom: `0.5px solid ${t.border}`, position: 'relative', zIndex: 5,
        // when sidebar is collapsed, traffic lights are on the main area side so we add left padding for them
        paddingLeft: sidebarCollapsed ? 92 : 16,
      }}
    >
      {/* Breadcrumb */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <Icon name="folder" size={13} color={t.text3} stroke={1.7} />
        <span style={{ fontSize: 13, color: t.text2, fontWeight: 500 }}>{repo.scope}</span>
        <span style={{ color: t.text4 }}>/</span>
        <span style={{ fontSize: 13, color: t.text, fontWeight: 600 }}>{repo.name}</span>
        <button
          style={{
            display: 'flex', alignItems: 'center', gap: 4, marginLeft: 4,
            padding: '3px 6px', borderRadius: 6, border: 'none', cursor: 'pointer',
            background: 'transparent', color: t.text3,
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="chevron" size={13} stroke={2} />
        </button>
      </div>

      <div style={{ flex: 1 }} />

      {/* Branch chip */}
      <div
        style={{
          display: 'flex', alignItems: 'center', gap: 6,
          padding: '4px 9px', borderRadius: 7,
          background: t.surface, border: `0.5px solid ${t.border}`,
        }}
      >
        <Icon name="branch" size={12} color={t.text3} stroke={1.8} />
        <span className="mono" style={{ fontSize: 11.5, color: t.text2, fontWeight: 500 }}>{repo.branch}</span>
      </div>

      {/* Diff pill */}
      {repo.diff && (
        <div
          style={{
            display: 'flex', alignItems: 'center', gap: 0, padding: '4px 4px',
            borderRadius: 7, background: t.accentBg, border: `0.5px solid ${t.accentBorder}`,
          }}
        >
          <span style={{ display: 'flex', alignItems: 'center', gap: 4, padding: '0 6px', fontSize: 11.5, color: t.accent, fontWeight: 600 }}>
            <span
              style={{
                width: 14, height: 14, borderRadius: 4,
                background: `color-mix(in oklab, ${t.accent} 25%, transparent)`,
                display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              }}
            >
              <Icon name="pencil" size={9} color={t.accent} stroke={2.2} />
            </span>
            <span className="mono">{repo.diff.add}</span>
          </span>
        </div>
      )}

      <div style={{ width: 1, height: 18, background: t.border, margin: '0 4px' }} />

      {/* Theme */}
      <button
        onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
        title="主题"
        style={iconBtn(t)}
        onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
        onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
      >
        <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={15} color={t.text2} stroke={1.7} />
      </button>

      {/* Right panel toggle */}
      <button
        onClick={onTogglePanel}
        title="右侧面板"
        style={{
          ...iconBtn(t),
          background: panelOpen ? t.surfaceActive : 'transparent',
          color: panelOpen ? t.accent : t.text2,
        }}
        onMouseEnter={(e) => {
          if (!panelOpen) e.currentTarget.style.background = t.surfaceHover;
        }}
        onMouseLeave={(e) => {
          if (!panelOpen) e.currentTarget.style.background = 'transparent';
        }}
      >
        <Icon name="sidebarR" size={15} stroke={1.7} />
      </button>
    </div>
  );
}
