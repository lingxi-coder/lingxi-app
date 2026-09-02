import { useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import type { SlashCommandDto } from '@lingxi/bridge-client';

import type { UseBridge } from '../bridge/useBridge';
import { filterSlashCommands, renderDesktopSlashHelp, slashCommandText, slashMenuLabel } from '../bridge/slashCommands';
import { useT } from '../theme/ThemeContext';
import type { ThemeMode } from '../theme/tokens';
import { Icon } from './Icon';

type LocalAction = {
  id: string;
  title: string;
  description: string;
  keywords: string[];
  disabled?: boolean;
  disabledReason?: string;
  kind: 'local';
  run(): Promise<void> | void;
};

type SlashAction = {
  id: string;
  title: string;
  description: string;
  keywords: string[];
  kind: 'slash';
  command: SlashCommandDto;
  disabled?: boolean;
  disabledReason?: string;
  run(): Promise<void>;
};

type PaletteAction = LocalAction | SlashAction;
type ScoredAction = PaletteAction & { score: number };

const PALETTE_LOCAL_OVERRIDE_NAMES = new Set([
  'add-dir', 'cd', 'clear', 'compact', 'config', 'copy', 'help', 'login',
  'logout', 'plugin', 'reload-plugins', 'tasks', 'theme',
]);

function normalize(value: string): string {
  return value.trim().toLowerCase();
}

function rankMatch(haystack: string, needle: string): number | null {
  if (!needle) return 0;
  const hay = haystack.toLowerCase();
  if (hay === needle) return 0;
  if (hay.startsWith(needle)) return 10;
  const index = hay.indexOf(needle);
  if (index >= 0) return 20 + index;
  return null;
}

function scoreAction(action: PaletteAction, needle: string): number | null {
  if (!needle) return 0;
  const fields = [action.title, action.description, ...action.keywords];
  const ranked = fields
    .map((field) => rankMatch(field, needle))
    .filter((score): score is number => score !== null);
  if (ranked.length === 0) return null;
  return Math.min(...ranked);
}

export function DesktopCommandPalette({
  open,
  bridge,
  theme,
  onClose,
  onOpenSettingsPage,
}: {
  open: boolean;
  bridge: UseBridge;
  theme: ThemeMode;
  onClose(): void;
  onOpenSettingsPage(pageId?: string): void;
}) {
  const t = useT();
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const deferredQuery = useDeferredValue(query);
  const sessionReady = Boolean(bridge.activeSession?.sessionId) && !bridge.sessionLoading;

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setSelectedIndex(0);
    setError(null);
    void bridge.refreshSlashCommands().catch(() => undefined);
    window.requestAnimationFrame(() => inputRef.current?.focus());
  }, [bridge.refreshSlashCommands, open]);

  const localActions = useMemo<LocalAction[]>(() => {
    const blockedReason = sessionReady ? undefined : '需要先打开可用会话。';
    const refreshBlockedReason = bridge.running ? '运行中的回合结束后再刷新。' : blockedReason;
    const sessionBlockedReason = bridge.running ? '当前回合进行中，先等待结束。' : blockedReason;
    return [
      {
        id: 'show-help',
        title: '/help · Desktop commands',
        description: 'Show only commands that have a real Desktop disposition.',
        keywords: ['help', 'commands', 'slash'],
        kind: 'local',
        disabled: !sessionReady,
        disabledReason: blockedReason,
        run: () => {
          bridge.beginLocalCommand('/help');
          bridge.emitCommandOutput(renderDesktopSlashHelp(bridge.desktop.slashCommands), false);
        },
      },
      {
        id: 'open-general-settings',
        title: '/config · 打开设置',
        description: '打开 Desktop 设置，而不是返回终端配置提示。',
        keywords: ['config', 'settings', 'theme'],
        kind: 'local',
        run: () => onOpenSettingsPage('general'),
      },
      {
        id: 'open-projects',
        title: '/cd · 切换项目',
        description: '打开项目与信任设置以选择工作目录。',
        keywords: ['cd', 'project', 'workspace'],
        kind: 'local',
        run: () => onOpenSettingsPage('projects'),
      },
      {
        id: 'open-add-directory',
        title: '/add-dir · 添加工作目录',
        description: '打开权限页管理 additionalDirectories。',
        keywords: ['add-dir', 'directory', 'permissions'],
        kind: 'local',
        run: () => onOpenSettingsPage('permissions'),
      },
      {
        id: 'open-plugins',
        title: '/plugin · 插件与市场',
        description: '打开插件管理页。',
        keywords: ['plugin', 'plugins', 'marketplace'],
        kind: 'local',
        run: () => onOpenSettingsPage('plugins'),
      },
      {
        id: 'reload-plugins',
        title: '/reload-plugins · 重新加载插件',
        description: '重启当前 engine，使插件和 slash catalog 重新加载。',
        keywords: ['reload-plugins', 'plugin', 'restart'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: sessionBlockedReason,
        run: () => bridge.restartBridge(),
      },
      {
        id: 'copy-last-response',
        title: '/copy · 复制最后回复',
        description: '复制最近一条 assistant 文本到系统剪贴板。',
        keywords: ['copy', 'clipboard', 'response'],
        kind: 'local',
        disabled: !sessionReady,
        disabledReason: blockedReason,
        run: async () => {
          const item = [...bridge.conversation.items].reverse().find((entry) => (
            entry.type === 'narration' && entry.role === 'assistant' && entry.text.trim().length > 0
          ));
          if (!item || item.type !== 'narration') throw new Error('没有可复制的 assistant 回复。');
          await bridge.copyText(item.text);
        },
      },
      {
        id: 'open-account',
        title: '打开账户',
        description: '查看登录态、切换到 Provider 凭据页。',
        keywords: ['account', 'auth', 'login', 'logout', 'credential'],
        kind: 'local',
        run: () => onOpenSettingsPage('account'),
      },
      {
        id: 'open-diagnostics',
        title: '打开诊断',
        description: '查看 status、doctor、重启与日志。',
        keywords: ['diagnostics', 'status', 'doctor', 'log'],
        kind: 'local',
        run: () => onOpenSettingsPage('diagnostics'),
      },
      {
        id: 'open-provider-credentials',
        title: '打开 Provider 凭据',
        description: '编辑安全存储、API base URL 与模型相关联的密钥。',
        keywords: ['provider', 'credential', 'keychain', 'apiBaseUrl'],
        kind: 'local',
        run: () => onOpenSettingsPage('provider-credentials'),
      },
      {
        id: 'refresh-auth',
        title: '刷新 Auth',
        description: '重新拉取 auth listing 和凭据摘要。',
        keywords: ['auth', 'refresh', 'credential'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: refreshBlockedReason,
        run: () => bridge.refreshAuth(),
      },
      {
        id: 'refresh-hooks',
        title: '刷新 Hooks',
        description: '重新拉取 hooks listing。',
        keywords: ['hooks', 'refresh', 'hook'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: refreshBlockedReason,
        run: () => bridge.refreshHooks(),
      },
      {
        id: 'refresh-agents',
        title: '刷新 Agents',
        description: '重新拉取 session agent catalog。',
        keywords: ['agents', 'refresh', 'agent'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: refreshBlockedReason,
        run: () => bridge.refreshAgents(),
      },
      {
        id: 'refresh-status',
        title: '刷新状态',
        description: '仅重新拉取 /status 快照。',
        keywords: ['status', 'refresh', 'runtime'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: refreshBlockedReason,
        run: () => bridge.refreshStatus(),
      },
      {
        id: 'refresh-doctor',
        title: '刷新 Doctor',
        description: '仅重新拉取 /doctor 报告。',
        keywords: ['doctor', 'refresh', 'diagnostics'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: refreshBlockedReason,
        run: () => bridge.refreshDoctor(),
      },
      {
        id: 'login',
        title: 'Login',
        description: '触发引擎登录流程。',
        keywords: ['login', 'auth'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: sessionBlockedReason,
        run: () => bridge.login(),
      },
      {
        id: 'logout',
        title: 'Logout',
        description: '触发引擎登出流程。',
        keywords: ['logout', 'auth'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: sessionBlockedReason,
        run: () => bridge.logout(),
      },
      {
        id: 'force-compact',
        title: '立即压缩',
        description: '发送 /compact。',
        keywords: ['compact', 'compression'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: sessionBlockedReason,
        run: () => bridge.forceCompact(),
      },
      {
        id: 'clear-session',
        title: '清空会话',
        description: '调用专用会话清空流程。',
        keywords: ['clear', 'session', 'reset', 'new'],
        kind: 'local',
        disabled: !sessionReady || bridge.running || bridge.sessionLoading,
        disabledReason: sessionBlockedReason,
        run: async () => {
          if (window.confirm('Clear the current session and start a new draft?')) await bridge.clearSession();
        },
      },
      {
        id: 'toggle-runtime-center',
        title: bridge.runtimeCenter.overviewOpen ? '关闭运行时中心' : '打开运行时中心',
        description: '查看 tasks、agents、resources 与 plan。',
        keywords: ['runtime', 'center', 'agents', 'tasks'],
        kind: 'local',
        run: () => bridge.setRuntimeCenterOverviewOpen(!bridge.runtimeCenter.overviewOpen),
      },
      {
        id: 'toggle-theme',
        title: theme === 'dark' ? '切到浅色主题' : '切到深色主题',
        description: '切换当前桌面配色。',
        keywords: ['theme', 'light', 'dark', 'appearance'],
        kind: 'local',
        run: () => bridge.setThemePreference(theme === 'dark' ? 'light' : 'dark'),
      },
    ];
  }, [bridge, onOpenSettingsPage, sessionReady, theme]);

  const slashActions = useMemo<SlashAction[]>(() => {
    const commands = filterSlashCommands(bridge.desktop.slashCommands, deferredQuery, 48)
      .filter((command) => (
        command.source !== 'builtin' || !PALETTE_LOCAL_OVERRIDE_NAMES.has(command.name)
      ));
    return commands.map((command) => ({
      id: `slash:${command.name}`,
      title: slashMenuLabel(command),
      description: command.description,
      keywords: [
        command.name,
        ...(command.aliases ?? []),
        command.source,
        command.argument_hint ?? '',
        command.menu_description ?? '',
      ],
      kind: 'slash' as const,
      command,
      disabled: !sessionReady || bridge.sessionLoading,
      disabledReason: sessionReady ? undefined : '需要先打开可用会话。',
      run: () => bridge.runSlashCommand(slashCommandText(command.name)),
    }));
  }, [bridge.desktop.slashCommands, bridge.runSlashCommand, deferredQuery, sessionReady, bridge.sessionLoading]);

  const results = useMemo<ScoredAction[]>(() => {
    const needle = normalize(deferredQuery);
    const local: ScoredAction[] = localActions
      .map((action) => ({ action, score: scoreAction(action, needle) }))
      .filter((entry) => entry.score !== null)
      .map(({ action, score }) => ({ ...action, score: score as number }));
    const slash: ScoredAction[] = slashActions
      .map((action) => ({
        ...action,
        score: scoreAction(action, needle) ?? 0,
      }));
    return [...local, ...slash]
      .sort((left, right) => left.score - right.score || (left.kind === right.kind ? left.title.localeCompare(right.title) : left.kind === 'local' ? -1 : 1));
  }, [deferredQuery, localActions, slashActions]);

  useEffect(() => {
    setSelectedIndex((index) => Math.max(0, Math.min(index, Math.max(results.length - 1, 0))));
  }, [results.length]);

  useEffect(() => {
    if (!open) return;
    const selected = listRef.current?.querySelector<HTMLElement>(`[data-palette-index="${selectedIndex}"]`);
    selected?.scrollIntoView({ block: 'nearest' });
  }, [open, selectedIndex]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      const panel = listRef.current?.closest('[data-command-palette-panel]');
      if (panel instanceof HTMLElement && panel.contains(target)) return;
      onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, onClose]);

  const invoke = async (action: PaletteAction) => {
    if (action.disabled) {
      setError(action.disabledReason ?? '当前无法执行该命令。');
      return;
    }
    setError(null);
    try {
      await action.run();
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '命令执行失败。');
    }
  };

  if (!open) return null;

  const selected = results[selectedIndex];
  const sessionSummary = bridge.desktop.status
    ? `${bridge.desktop.status.model} · ${bridge.desktop.status.n_mcp_connected}/${bridge.desktop.status.n_mcp_total} MCP · ${bridge.desktop.status.n_agents} agents`
    : bridge.desktop.currentModel ?? 'No status yet';

  return (
    <div
      className="desktop-command-palette-backdrop"
      role="presentation"
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 90,
        display: 'grid',
        placeItems: 'start center',
        padding: '74px 20px 20px',
        background: 'color-mix(in oklab, #000 22%, transparent)',
        backdropFilter: 'blur(12px)',
      }}
    >
      <div
        data-command-palette-panel
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
        style={{
          width: 'min(760px, calc(100vw - 32px))',
          borderRadius: 24,
          border: `1px solid ${t.border}`,
          background: t.windowBg,
          boxShadow: t.dark ? '0 24px 70px rgba(0,0,0,.42)' : '0 26px 70px rgba(43,35,72,.18)',
          overflow: 'hidden',
        }}
      >
        <div style={{ padding: 16, borderBottom: `0.5px solid ${t.border}`, display: 'grid', gap: 10 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <div style={{ width: 34, height: 34, borderRadius: 11, display: 'grid', placeItems: 'center', background: t.accentBg, color: t.accent, boxShadow: `0 0 0 1px ${t.accentBorder}` }}>
              <Icon name="search" size={16} />
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ color: t.text, fontSize: 15, fontWeight: 650, letterSpacing: '-.02em' }}>命令面板</div>
              <div className="mono" style={{ color: t.text4, fontSize: 10.5, marginTop: 2 }}>{sessionSummary}</div>
            </div>
            <button type="button" onClick={onClose} style={{ border: 0, background: 'transparent', color: t.text4, cursor: 'pointer', padding: 6, borderRadius: 8 }}>
              <Icon name="x" size={16} />
            </button>
          </div>
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setSelectedIndex(0);
              setError(null);
            }}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown') {
                event.preventDefault();
                setSelectedIndex((index) => Math.min(index + 1, Math.max(results.length - 1, 0)));
              } else if (event.key === 'ArrowUp') {
                event.preventDefault();
                setSelectedIndex((index) => Math.max(0, index - 1));
              } else if (event.key === 'Enter') {
                event.preventDefault();
                if (selected) void invoke(selected);
              }
            }}
            placeholder="搜索命令、设置页、会话动作…"
            aria-label="搜索命令"
            style={{
              width: '100%',
              minHeight: 42,
              padding: '0 14px',
              borderRadius: 12,
              border: `1px solid ${t.border}`,
              background: t.surface,
              color: t.text,
              font: 'inherit',
              outline: 'none',
            }}
          />
        </div>

        <div ref={listRef} role="listbox" aria-label="命令结果" style={{ maxHeight: 'min(58vh, 520px)', overflowY: 'auto', padding: 8 }}>
          {results.length === 0 && (
            <div role="status" style={{ padding: '22px 16px', color: t.text4, fontSize: 12.5 }}>
              没有匹配结果。
            </div>
          )}
          {results.map((entry, index) => {
            const selectedRow = index === selectedIndex;
            const selectedSlash = entry.kind === 'slash';
            return (
              <button
                key={entry.id}
                type="button"
                data-palette-index={index}
                role="option"
                aria-selected={selectedRow}
                disabled={Boolean(entry.disabled)}
                onClick={() => { void invoke(entry); }}
                onMouseEnter={() => setSelectedIndex(index)}
                style={{
                  width: '100%',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  padding: '11px 12px',
                  border: 0,
                  borderRadius: 14,
                  background: selectedRow ? t.surfaceHover : 'transparent',
                  color: entry.disabled ? t.text4 : t.text,
                  cursor: entry.disabled ? 'not-allowed' : 'pointer',
                  textAlign: 'left',
                  marginBottom: 4,
                }}
              >
                <div style={{ width: 28, height: 28, flexShrink: 0, borderRadius: 10, display: 'grid', placeItems: 'center', background: selectedSlash ? t.accentBg : t.surfaceActive, color: selectedSlash ? t.accent : t.text3 }}>
                  <Icon name={selectedSlash ? 'terminal' : 'spark'} size={14} />
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
                    <span style={{ fontSize: 13.5, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{entry.title}</span>
                    {entry.kind === 'slash' && (
                      <span className="mono" style={{ color: t.text4, fontSize: 10.5 }}>{entry.command.source}</span>
                    )}
                  </div>
                  <div style={{ marginTop: 2, color: t.text3, fontSize: 12, lineHeight: 1.45, textWrap: 'pretty' }}>{entry.description}</div>
                </div>
                <div style={{ flexShrink: 0, display: 'grid', justifyItems: 'end', gap: 4 }}>
                  {entry.kind === 'slash' ? (
                    <span className="mono" style={{ color: t.text4, fontSize: 10.5 }}>{entry.command.argument_hint ?? '无参数'}</span>
                  ) : (
                    <span className="mono" style={{ color: t.text4, fontSize: 10.5 }}>Desktop</span>
                  )}
                  {entry.disabled && entry.disabledReason && (
                    <span style={{ color: t.warn, fontSize: 10.5 }}>{entry.disabledReason}</span>
                  )}
                </div>
              </button>
            );
          })}
        </div>

        <div style={{ padding: '12px 16px 15px', borderTop: `0.5px solid ${t.border}`, display: 'flex', alignItems: 'center', gap: 10, color: t.text4, fontSize: 10.5 }}>
          <span className="mono">↑↓</span>
          <span>浏览结果</span>
          <span className="mono">Enter</span>
          <span>执行</span>
          <span className="mono">Esc</span>
          <span>关闭</span>
          {error && <span role="alert" style={{ marginLeft: 'auto', color: t.danger }}>{error}</span>}
        </div>
      </div>
    </div>
  );
}
