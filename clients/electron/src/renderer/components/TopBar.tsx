import { useT } from '../theme/ThemeContext';
import type { Project } from '../data';
import type { ThemeMode } from '../theme/tokens';
import type { UsageSnapshot } from '../bridge/conversation';
import { Icon } from './Icon';
import { iconBtn } from './primitives';

interface TopBarProps {
  repo: Project;
  onTogglePanel: () => void;
  panelOpen: boolean;
  theme: ThemeMode;
  setTheme: (m: ThemeMode) => void;
  sidebarCollapsed: boolean;
  /** Live token-usage snapshot from the bridge (`usage_update`), or `null`. */
  usage?: UsageSnapshot | null;
}

/** Compact token count (e.g. `1.2k`, `980`) for the usage chip. */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

export function TopBar({ repo, onTogglePanel, panelOpen, theme, setTheme, sidebarCollapsed, usage }: TopBarProps) {
  const t = useT();
  const cached = usage ? usage.cacheReadTokens + usage.cacheCreationTokens : 0;
  const usageTitle = usage
    ? `tokens — in ${usage.inputTokens.toLocaleString()} · out ${usage.outputTokens.toLocaleString()}` +
      ` · cache read ${usage.cacheReadTokens.toLocaleString()} · cache write ${usage.cacheCreationTokens.toLocaleString()}`
    : undefined;
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

      {/* Token counter — live `usage_update` snapshot from the engine. */}
      {usage && (
        <div
          title={usageTitle}
          style={{
            display: 'flex', alignItems: 'center', gap: 7,
            padding: '4px 9px', borderRadius: 7,
            background: t.surface, border: `0.5px solid ${t.border}`,
          }}
        >
          <Icon name="spark" size={12} color={t.text3} stroke={1.8} />
          <span style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            <span style={{ fontSize: 10.5, color: t.text4, fontWeight: 600, letterSpacing: 0.3 }}>↑</span>
            <span className="mono" style={{ fontSize: 11.5, color: t.text2, fontWeight: 500 }}>
              {fmtTokens(usage.inputTokens)}
            </span>
            <span style={{ fontSize: 10.5, color: t.text4, fontWeight: 600, letterSpacing: 0.3, marginLeft: 2 }}>↓</span>
            <span className="mono" style={{ fontSize: 11.5, color: t.text2, fontWeight: 500 }}>
              {fmtTokens(usage.outputTokens)}
            </span>
          </span>
          {cached > 0 && (
            <span
              className="mono"
              style={{ fontSize: 10.5, color: t.accent, fontWeight: 500 }}
              title={`cache read ${usage.cacheReadTokens.toLocaleString()} · cache write ${usage.cacheCreationTokens.toLocaleString()}`}
            >
              ⚡{fmtTokens(cached)}
            </span>
          )}
        </div>
      )}

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
