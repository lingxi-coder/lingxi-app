import { useState } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';
import type { PinnedSessionRecord } from '../../../bridge/lingxi';
import { formatSessionMetadata } from '../../../bridge/sessionPresentation';
import { ghostButtonStyle } from './ghostButton';

export interface ProjectRow {
  path: string;
  active: boolean;
}

/**
 * Which persisted projects to list, and which one is active. Pure so it can
 * be tested without mounting anything.
 *
 * Lifted from `BetaSidebar`'s project loop in `BetaDesktop.tsx` — NOT
 * `BetaSettings` as the task brief said. `BetaSettings` (the settings
 * dialog) has no project list or trust UI at all; that behaviour lives in
 * `BetaSidebar`, a separate exported component in the same file. See this
 * task's report for the full correction.
 */
export function projectRows(settings: { projects: string[]; activeProject?: string } | undefined): ProjectRow[] {
  const projects = settings?.projects ?? [];
  return projects.map((path) => ({ path, active: path === settings?.activeProject }));
}

export interface PinnedSessionRow {
  projectPath: string;
  sessionId: string;
  title: string;
}

/**
 * Which sessions are pinned, in persisted order. Pure so it can be tested
 * without mounting anything — same shape as `projectRows` above. Lifted
 * from `BetaSidebar`'s "Pinned" section (`BetaDesktop.tsx`, ~lines 239-301):
 * this is the third deliverable the brief named for this page (置顶会话)
 * alongside the project list and trust handling, dropped in the first pass.
 */
export function pinnedSessionRows(settings: { pinnedSessions: PinnedSessionRecord[] } | undefined): PinnedSessionRow[] {
  return (settings?.pinnedSessions ?? []).map((pinned) => ({
    projectPath: pinned.projectPath,
    sessionId: pinned.sessionId,
    title: pinned.title,
  }));
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function Projects({ bridge }: PageContentProps) {
  const t = useT();
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const settings = bridge.bootstrap?.settings;
  // Mirrors `BetaSidebar`'s own precedence for "which project is active"
  // (an in-flight session's project outranks the persisted `activeProject`,
  // which outranks whatever `workspace` currently resolves to) — `projectRows`
  // itself stays simple and pure, taking a single resolved path.
  const activePath = bridge.activeSession?.projectPath ?? settings?.activeProject ?? bridge.bootstrap?.workspace?.path;
  const rows = projectRows({ projects: settings?.projects ?? [], activeProject: activePath });
  const pinnedRows = pinnedSessionRows({ pinnedSessions: settings?.pinnedSessions ?? [] });
  const workspace = bridge.bootstrap?.workspace;

  const hasActiveWork = (path: string) =>
    (bridge.bootstrap?.runtimes ?? []).some((runtime) => {
      if (runtime.projectPath !== path) return false;
      const status = bridge.sessionRuntimeStatus(runtime.sessionId);
      return Boolean(status?.turnActive || status?.pendingInteractions);
    });

  // Same title/metadata fallback `BetaSidebar` uses for a pinned row: the
  // catalog for that project may not have loaded yet ("加载中…"), may have
  // loaded without that session in it ("不可用" — e.g. it was deleted from
  // disk), or may confirm it, in which case the catalog's title wins over
  // the pinned record's stale one.
  const pinnedSessionMeta = (row: PinnedSessionRow) => {
    const catalog = bridge.bootstrap?.projectCatalogs?.[row.projectPath];
    const current = catalog?.sessions.find((session) => session.uuid === row.sessionId);
    const title = current?.title || row.title || '未命名会话';
    const metadata = !catalog ? '加载中…' : current ? formatSessionMetadata(current.modified_rfc3339, current.message_count) : '不可用';
    return { title, metadata };
  };

  const handleAdd = () => {
    setError(null);
    void bridge.addProject().catch((cause) => setError(cause instanceof Error ? cause.message : '无法添加项目。'));
  };
  const handleActivate = (path: string) => {
    setError(null);
    setBusyPath(path);
    void bridge.activateProject(path)
      .catch((cause) => setError(cause instanceof Error ? cause.message : '无法切换项目。'))
      .finally(() => setBusyPath((current) => (current === path ? null : current)));
  };
  const handleRemove = (path: string) => {
    setError(null);
    setBusyPath(path);
    void bridge.removeProject(path)
      .catch((cause) => setError(cause instanceof Error ? cause.message : '无法移除项目。'))
      .finally(() => setBusyPath((current) => (current === path ? null : current)));
  };
  const handleUnpin = (row: PinnedSessionRow, title: string) => {
    setError(null);
    void bridge.setSessionPinned({ projectPath: row.projectPath, sessionId: row.sessionId, title }, false)
      .catch((cause) => setError(cause instanceof Error ? cause.message : '无法取消置顶。'));
  };

  return (
    <>
      <Card title="项目">
        <Row title="已添加的项目" desc="Lingxi 只能访问这里列出的项目文件夹。" align="center">
          <button type="button" onClick={handleAdd} style={ghostButtonStyle(t)}>
            <Icon name="plus" size={12} stroke={2} /> 添加项目
          </button>
        </Row>
        {rows.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>
            还没有添加项目文件夹。
          </div>
        )}
        {rows.map((row) => {
          const busy = busyPath === row.path;
          const activeWork = hasActiveWork(row.path);
          return (
            <Row
              key={row.path}
              align="center"
              title={
                <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Icon name="folder" size={14} color={row.active ? t.text2 : t.text3} stroke={1.7} />
                  {basename(row.path)}
                  {row.active && (
                    <span style={{ fontSize: 10.5, padding: '2px 7px', borderRadius: 5, background: t.surfaceHover, color: t.text3, fontWeight: 600 }}>
                      当前
                    </span>
                  )}
                </span>
              }
              desc={row.path}
            >
              <div style={{ display: 'flex', gap: 7 }}>
                {!row.active && (
                  <button type="button" disabled={busy} onClick={() => handleActivate(row.path)} style={ghostButtonStyle(t, busy)}>
                    切换
                  </button>
                )}
                <button
                  type="button"
                  disabled={busy || activeWork}
                  title={activeWork ? '该项目有正在运行的会话，无法移除' : undefined}
                  onClick={() => handleRemove(row.path)}
                  style={ghostButtonStyle(t, busy || activeWork, true)}
                >
                  移除
                </button>
              </div>
            </Row>
          );
        })}
      </Card>

      {error && (
        <div role="alert" data-testid="projects-error" style={{ fontSize: 12.5, color: t.danger, marginTop: -18, marginBottom: 18 }}>
          {error}
        </div>
      )}

      <Card title="置顶会话">
        {pinnedRows.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>
            还没有置顶的会话。
          </div>
        )}
        {pinnedRows.map((row) => {
          const { title, metadata } = pinnedSessionMeta(row);
          return (
            <Row
              key={`${row.projectPath}\0${row.sessionId}`}
              align="center"
              title={title}
              desc={`${basename(row.projectPath)} · ${metadata}`}
            >
              <button type="button" onClick={() => handleUnpin(row, title)} style={ghostButtonStyle(t)}>
                <Icon name="pin" size={12} stroke={2} /> 取消置顶
              </button>
            </Row>
          );
        })}
      </Card>

      {/* Trust and its fingerprint are only known for the CURRENTLY ACTIVE
          workspace (`bridge.bootstrap.workspace`) — there is no bridge call
          that returns trust for every listed project, so this can't be a
          per-row column above without inventing data the bridge doesn't have. */}
      {workspace?.path && (
        <Card title="工作区信任">
          <Row title="当前项目" desc={workspace.path} align="center">
            <span style={{
              fontSize: 10.5, padding: '2px 7px', borderRadius: 5, fontWeight: 600,
              background: t.surfaceHover, color: workspace.trusted ? t.ok : t.warn,
            }}>
              {workspace.trusted ? '已信任' : '未信任'}
            </span>
          </Row>
          {workspace.fingerprint && (
            <Row title="指纹" desc="用于检测项目的可执行配置自上次信任决定以来是否发生变化。" align="center">
              <span className="mono" style={{ fontSize: 11, color: t.text3 }}>{workspace.fingerprint.slice(0, 16)}…</span>
            </Row>
          )}
        </Card>
      )}
    </>
  );
}
