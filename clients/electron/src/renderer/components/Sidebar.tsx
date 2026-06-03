import { useState, useLayoutEffect } from 'react';
import { useT } from '../theme/ThemeContext';
import { PROJECTS } from '../data';
import { Icon } from './Icon';
import {
  Kbd, iconBtn, ModeTabs, ProjectMenuIcon, AccountMenuItem, type Mode,
} from './primitives';

interface SidebarProps {
  mode: Mode;
  setMode: (m: Mode) => void;
  setActiveRepo: (id: string) => void;
  activeSession: string;
  setActiveSession: (id: string) => void;
  openSettings: () => void;
  collapsed: boolean;
  setCollapsed: (v: boolean) => void;
}

interface MenuPos {
  left: number;
  top: number;
}

export function Sidebar({
  mode, setMode, setActiveRepo, activeSession, setActiveSession,
  openSettings, collapsed, setCollapsed,
}: SidebarProps) {
  const t = useT();
  const [moreOpen, setMoreOpen] = useState(false);
  const [accountOpen, setAccountOpen] = useState(false);
  const [hoverProject, setHoverProject] = useState<string | null>(null);
  const [menuProject, setMenuProject] = useState<string | null>(null);
  const [menuPos, setMenuPos] = useState<MenuPos | null>(null);

  // Position the project context menu — flip above when there isn't room below.
  useLayoutEffect(() => {
    if (!menuProject) {
      setMenuPos(null);
      return;
    }
    const trigger = document.querySelector(`[data-project-row="${menuProject}"]`);
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const MENU_H = 268;
    const spaceBelow = window.innerHeight - rect.bottom;
    const flipUp = spaceBelow < MENU_H + 12;
    setMenuPos({
      left: rect.left + 16,
      top: flipUp ? Math.max(8, rect.top - MENU_H - 4) : rect.bottom + 2,
    });
  }, [menuProject]);

  // expand state per project — default: open the project that owns the active session, plus a couple by default
  const [expanded, setExpanded] = useState<Record<string, boolean>>({
    mlplatform: true, agai: true, visionx: true, 'lingxi-next': true, lingxi: true, telegram: true,
  });
  const [showMore, setShowMore] = useState<Record<string, boolean>>({}); // per-project show-hidden flag
  const toggle = (id: string) => setExpanded((o) => ({ ...o, [id]: !o[id] }));

  if (collapsed) {
    return (
      <div
        style={{
          width: 52, flexShrink: 0, background: t.sidebarBg,
          borderRight: `0.5px solid ${t.border}`, paddingTop: 40,
          display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 4,
        }}
      >
        <button onClick={() => setCollapsed(false)} title="展开" style={iconBtn(t)}>
          <Icon name="sidebar" size={15} color={t.text2} stroke={1.7} />
        </button>
        <button title="搜索" style={iconBtn(t)}><Icon name="search" size={15} color={t.text2} stroke={1.7} /></button>
        <div style={{ height: 1, width: 28, background: t.border, margin: '6px 0' }} />
        <button title="新建会话" style={iconBtn(t)}><Icon name="plus" size={15} color={t.text2} stroke={1.7} /></button>
        <button title="Routines" style={iconBtn(t)}><Icon name="bolt" size={15} color={t.text2} stroke={1.7} /></button>
        <button title="Customize" style={iconBtn(t)}><Icon name="box" size={15} color={t.text2} stroke={1.7} /></button>
        <div style={{ flex: 1 }} />
        <button onClick={openSettings} title="设置" style={iconBtn(t)}><Icon name="cog" size={15} color={t.text2} stroke={1.7} /></button>
        <div
          style={{
            width: 28, height: 28, borderRadius: 7, marginBottom: 10,
            background: `linear-gradient(135deg, ${t.accent}, ${t.accent2})`,
            color: '#fff', display: 'flex', alignItems: 'center', justifyContent: 'center',
            fontSize: 11, fontWeight: 600,
          }}
        >
          LL
        </div>
      </div>
    );
  }

  const primaryActions: { icon: string; label: string; kbd?: string; bold?: boolean; desc?: string }[] = [
    { icon: 'plus', label: '新建会话', kbd: '⌘ N', bold: true },
    { icon: 'bolt', label: 'Routines', desc: '自动化工作流' },
    { icon: 'box', label: 'Customize', desc: '指令与角色' },
  ];

  const menuItems: { icon?: string; label?: string; danger?: boolean; divider?: boolean }[] = [
    { icon: 'pin', label: 'Pin project' },
    { icon: 'finder', label: 'Open in Finder' },
    { icon: 'tree', label: 'Create permanent worktree' },
    { icon: 'rename', label: 'Rename project' },
    { icon: 'archive', label: 'Archive chats' },
    { divider: true },
    { icon: 'x', label: 'Remove', danger: true },
  ];

  return (
    <div
      style={{
        width: 260, flexShrink: 0, background: t.sidebarBg,
        borderRight: `0.5px solid ${t.border}`,
        display: 'flex', flexDirection: 'column', paddingTop: 38,
      }}
    >
      {/* Top icon row */}
      <div style={{ padding: '4px 10px 6px', display: 'flex', alignItems: 'center', gap: 2 }}>
        <button
          onClick={() => setCollapsed(true)}
          title="收起边栏"
          style={iconBtn(t)}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="sidebar" size={15} color={t.text2} stroke={1.7} />
        </button>
        <button
          title="搜索 ⌘K"
          style={iconBtn(t)}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="search" size={15} color={t.text2} stroke={1.7} />
        </button>
        <div style={{ flex: 1 }} />
      </div>

      {/* Mode tabs — full width below icons */}
      <div style={{ padding: '0 10px 10px' }}>
        <ModeTabs mode={mode} setMode={setMode} />
      </div>

      {/* Primary actions */}
      <div style={{ padding: '0 10px 6px' }}>
        {primaryActions.map((item) => (
          <button
            key={item.label}
            style={{
              display: 'flex', alignItems: 'center', gap: 10, width: '100%',
              padding: '8px 10px', borderRadius: 8, border: 'none', cursor: 'pointer',
              background: 'transparent', color: t.text,
              fontSize: 13, fontWeight: item.bold ? 600 : 500,
              fontFamily: 'inherit', textAlign: 'left',
              transition: 'background 0.12s',
            }}
            onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            <Icon name={item.icon} size={item.icon === 'bolt' ? 14 : 15} color={item.bold ? t.text : t.text2} stroke={1.8} />
            <span style={{ flex: 1 }}>{item.label}</span>
            {item.kbd && <Kbd>{item.kbd}</Kbd>}
          </button>
        ))}
        <button
          onClick={() => setMoreOpen(!moreOpen)}
          style={{
            display: 'flex', alignItems: 'center', gap: 10, width: '100%',
            padding: '8px 10px', borderRadius: 8, border: 'none', cursor: 'pointer',
            background: 'transparent', color: t.text2,
            fontSize: 13, fontWeight: 500, fontFamily: 'inherit', textAlign: 'left',
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="chevron" size={14} stroke={2} color={t.text2} style={{ transform: moreOpen ? 'rotate(180deg)' : undefined }} />
          <span style={{ flex: 1 }}>More</span>
        </button>
        {moreOpen && (
          <div style={{ paddingLeft: 26, paddingBottom: 4, animation: 'fade-in 0.15s ease' }}>
            {['Routines · 历史', '快捷指令', '导入 / 导出', '帮助'].map((label) => (
              <div
                key={label}
                style={{ padding: '6px 10px', borderRadius: 6, fontSize: 12.5, color: t.text3, cursor: 'pointer' }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.background = t.surfaceHover;
                  e.currentTarget.style.color = t.text2;
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = 'transparent';
                  e.currentTarget.style.color = t.text3;
                }}
              >
                {label}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Projects header */}
      <div style={{ padding: '14px 8px 4px 14px', display: 'flex', alignItems: 'center', gap: 4 }}>
        <span
          style={{
            flex: 1, fontSize: 12.5, color: t.text2, fontWeight: 600,
            display: 'inline-flex', alignItems: 'center', gap: 4, cursor: 'pointer',
          }}
        >
          Projects <Icon name="chevron" size={11} color={t.text3} stroke={2} />
        </span>
        <button
          title="全部折叠"
          onClick={() => setExpanded({})}
          style={{ ...iconBtn(t), width: 22, height: 22 }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke={t.text3} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <path d="M4 4l6 6M20 4l-6 6M4 20l6-6M20 20l-6-6" />
          </svg>
        </button>
        <button
          title="更多"
          style={{ ...iconBtn(t), width: 22, height: 22 }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="more" size={13} color={t.text3} stroke={2} />
        </button>
        <button
          title="新建项目"
          style={{ ...iconBtn(t), width: 22, height: 22 }}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke={t.text3} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            <path d="M12 12v4M10 14h4" />
          </svg>
        </button>
      </div>

      {/* Project tree */}
      <div style={{ flex: 1, overflowY: 'auto', padding: '2px 8px 8px' }}>
        {PROJECTS.map((p) => {
          const open = !!expanded[p.id];
          const visibleSessions = open
            ? showMore[p.id]
              ? p.sessions
              : p.sessions.filter((s) => !s.hidden)
            : [];
          const hiddenCount = p.sessions.filter((s) => s.hidden).length;
          return (
            <div key={p.id} style={{ marginBottom: 2, position: 'relative' }}>
              {/* Project header */}
              <div
                data-project-row={p.id}
                onClick={() => {
                  toggle(p.id);
                  setActiveRepo(p.id);
                }}
                onMouseEnter={() => setHoverProject(p.id)}
                onMouseLeave={() => setHoverProject(null)}
                style={{
                  display: 'flex', alignItems: 'center', gap: 8,
                  height: 30, padding: '0 6px 0 8px', borderRadius: 7, cursor: 'pointer',
                  color: t.text2,
                  background: hoverProject === p.id || menuProject === p.id ? t.surfaceHover : 'transparent',
                  transition: 'background 0.12s',
                }}
              >
                <Icon name="folder" size={14} color={t.text3} stroke={1.7} />
                <span
                  style={{
                    flex: 1, fontSize: 13, fontWeight: 500, color: t.text,
                    overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                  }}
                >
                  {p.name}
                </span>

                {hoverProject === p.id || menuProject === p.id ? (
                  <div style={{ display: 'flex', gap: 1, alignItems: 'center' }}>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        setMenuProject(menuProject === p.id ? null : p.id);
                      }}
                      title="更多"
                      style={{
                        width: 20, height: 20, borderRadius: 5, border: 'none', cursor: 'pointer',
                        background: menuProject === p.id ? t.surfaceActive : 'transparent',
                        color: t.text2, display: 'flex', alignItems: 'center', justifyContent: 'center',
                      }}
                      onMouseEnter={(e) => {
                        if (menuProject !== p.id) e.currentTarget.style.background = t.surfaceActive;
                      }}
                      onMouseLeave={(e) => {
                        if (menuProject !== p.id) e.currentTarget.style.background = 'transparent';
                      }}
                    >
                      <Icon name="more" size={13} stroke={2} />
                    </button>
                    <button
                      onClick={(e) => e.stopPropagation()}
                      title="新会话"
                      style={{
                        width: 20, height: 20, borderRadius: 5, border: 'none', cursor: 'pointer',
                        background: 'transparent', color: t.text2,
                        display: 'flex', alignItems: 'center', justifyContent: 'center',
                      }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceActive)}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                    >
                      <Icon name="pencil" size={12} stroke={1.8} />
                    </button>
                  </div>
                ) : p.dirty ? (
                  <span
                    style={{
                      width: 6, height: 6, borderRadius: 99,
                      background: t.warn, flexShrink: 0,
                      boxShadow: `0 0 0 2px color-mix(in oklab, ${t.warn} 25%, transparent)`,
                    }}
                  />
                ) : null}
              </div>

              {/* Context menu */}
              {menuProject === p.id && menuPos && (
                <>
                  <div onClick={() => setMenuProject(null)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
                  <div
                    style={{
                      position: 'fixed', left: menuPos.left, top: menuPos.top, zIndex: 50,
                      background: t.surface, border: `0.5px solid ${t.borderStrong}`,
                      borderRadius: 11, padding: 6, minWidth: 230,
                      boxShadow: '0 16px 40px rgba(0,0,0,0.28)',
                      animation: 'fade-in 0.12s ease',
                    }}
                  >
                    {menuItems.map((mi, i) =>
                      mi.divider ? (
                        <div key={`divider-${i}`} style={{ height: 0.5, background: t.border, margin: '4px 6px' }} />
                      ) : (
                        <div
                          key={mi.label}
                          onClick={() => setMenuProject(null)}
                          style={{
                            display: 'flex', alignItems: 'center', gap: 10,
                            padding: '6px 10px', borderRadius: 6, cursor: 'pointer',
                            color: mi.danger ? t.danger : t.text,
                          }}
                          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                        >
                          <ProjectMenuIcon name={mi.icon!} color={mi.danger ? t.danger : t.text2} />
                          <span style={{ fontSize: 13, fontWeight: 500 }}>{mi.label}</span>
                        </div>
                      ),
                    )}
                  </div>
                </>
              )}

              {/* Sessions */}
              {open && (
                <div style={{ paddingLeft: 6, paddingBottom: 4, animation: 'fade-in 0.15s ease' }}>
                  {p.sessions.length === 0 ? (
                    <div style={{ padding: '6px 10px 6px 30px', fontSize: 12.5, color: t.text4 }}>No chats</div>
                  ) : (
                    <>
                      {visibleSessions.map((s) => {
                        const active = s.id === activeSession;
                        return (
                          <div
                            key={s.id}
                            onClick={() => {
                              setActiveSession(s.id);
                              setActiveRepo(p.id);
                            }}
                            style={{
                              display: 'flex', alignItems: 'center', gap: 6,
                              padding: '5px 10px 5px 30px', borderRadius: 6, cursor: 'pointer',
                              background: active ? t.surfaceActive : 'transparent',
                              color: active ? t.text : t.text2,
                              transition: 'background 0.12s', position: 'relative',
                            }}
                            onMouseEnter={(e) => {
                              if (!active) e.currentTarget.style.background = t.surfaceHover;
                            }}
                            onMouseLeave={(e) => {
                              if (!active) e.currentTarget.style.background = 'transparent';
                            }}
                          >
                            {active && (
                              <div style={{ position: 'absolute', left: 20, top: 6, bottom: 6, width: 2, background: t.accent, borderRadius: 99 }} />
                            )}
                            <span
                              style={{
                                flex: 1, fontSize: 12.5, fontWeight: active ? 600 : 500,
                                overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                              }}
                            >
                              {s.title}
                            </span>
                            {s.kbd && <Kbd>{s.kbd}</Kbd>}
                            {s.activity && <span style={{ fontSize: 11, color: t.text4, fontFamily: 'inherit' }}>{s.activity}</span>}
                          </div>
                        );
                      })}
                      {hiddenCount > 0 && (
                        <div
                          onClick={() => setShowMore((o) => ({ ...o, [p.id]: !o[p.id] }))}
                          style={{ padding: '4px 10px 6px 30px', fontSize: 11.5, color: t.text3, cursor: 'pointer' }}
                          onMouseEnter={(e) => (e.currentTarget.style.color = t.text2)}
                          onMouseLeave={(e) => (e.currentTarget.style.color = t.text3)}
                        >
                          {showMore[p.id] ? 'Show less' : 'Show more'}
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Account row */}
      <div style={{ borderTop: `0.5px solid ${t.border}`, padding: '8px 10px 12px', position: 'relative' }}>
        <div
          onClick={() => setAccountOpen(!accountOpen)}
          style={{
            display: 'flex', alignItems: 'center', gap: 10,
            padding: '6px 8px', borderRadius: 8, cursor: 'pointer',
            background: accountOpen ? t.surfaceHover : 'transparent',
            transition: 'background 0.12s',
          }}
          onMouseEnter={(e) => {
            if (!accountOpen) e.currentTarget.style.background = t.surfaceHover;
          }}
          onMouseLeave={(e) => {
            if (!accountOpen) e.currentTarget.style.background = 'transparent';
          }}
        >
          <div
            style={{
              width: 28, height: 28, borderRadius: 7,
              background: `linear-gradient(135deg, ${t.accent}, ${t.accent2})`,
              color: '#fff', display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 11, fontWeight: 600,
            }}
          >
            LL
          </div>
          <div style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'baseline', gap: 6 }}>
            <span style={{ fontSize: 13, fontWeight: 500, color: t.text }}>lingfeng</span>
            <span style={{ color: t.text4 }}>·</span>
            <span style={{ fontSize: 12, color: t.text3 }}>Max</span>
          </div>
          <Icon name="chevron" size={13} color={t.text3} stroke={2} style={{ transform: accountOpen ? undefined : 'rotate(180deg)' }} />
        </div>

        {accountOpen && (
          <>
            <div onClick={() => setAccountOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
            <div
              style={{
                position: 'absolute', bottom: 'calc(100% - 4px)', left: 10, right: 10, zIndex: 50,
                background: t.surface, border: `0.5px solid ${t.borderStrong}`,
                borderRadius: 12, padding: 6,
                boxShadow: '0 16px 40px rgba(0,0,0,0.32)',
                animation: 'fade-in 0.15s ease',
              }}
            >
              <div style={{ padding: '8px 10px 10px', fontSize: 12.5, color: t.text3 }}>luolingfeng.flare@gmail.com</div>
              <AccountMenuItem icon="cog" label="Settings" kbd="⌘," onClick={() => { setAccountOpen(false); openSettings(); }} />
              <AccountMenuItem icon="globe" label="Language" arrow />
              <AccountMenuItem icon="help" label="Get help" />
              <div style={{ height: 0.5, background: t.border, margin: '6px 6px' }} />
              <AccountMenuItem icon="plans" label="View all plans" />
              <AccountMenuItem icon="download" label="Get apps and extensions" />
              <AccountMenuItem icon="gift" label="Gift Lingxi" />
              <AccountMenuItem icon="news" label="View changelog" />
              <AccountMenuItem icon="info" label="Learn more" arrow />
              <div style={{ height: 0.5, background: t.border, margin: '6px 6px' }} />
              <AccountMenuItem icon="logout" label="Log out" />
            </div>
          </>
        )}
      </div>
    </div>
  );
}
