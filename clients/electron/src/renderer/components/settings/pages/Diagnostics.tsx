import { useEffect, useState } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { restartDisabledReason, type PageContentProps } from '../SettingsScreen';
import type { DiagnosticEntry } from '../../../bridge/lingxi';
import { ghostButtonStyle } from './ghostButton';

function levelColor(t: ReturnType<typeof useT>, level: DiagnosticEntry['level']): string {
  if (level === 'error') return t.danger;
  if (level === 'warn') return t.warn;
  return t.text4;
}

/**
 * Copy report / export JSON / refresh, lifted from the old settings modal's
 * Diagnostics section in `BetaDesktop.tsx` (`bridge.copyDiagnostics` /
 * `exportDiagnostics` / `refreshDiagnostics`, and the same sanitized-log
 * rendering). The restart action is NOT part of that lifted section —
 * that Diagnostics block had no restart button — it is added
 * here per this task's brief, reusing `bridge.restartBridge` and the
 * already-tested `restartDisabledReason` the settings shell itself uses for
 * its own pending-settings restart action.
 */
export function Diagnostics({ bridge }: PageContentProps) {
  const t = useT();
  const [restartError, setRestartError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState(false);
  const status = bridge.desktop.status;
  const doctor = bridge.desktop.doctor;
  const compaction = bridge.desktop.lastCompaction;
  const retry = bridge.desktop.lastApiRetry;
  const auth = bridge.desktop.auth;

  useEffect(() => {
    void bridge.refreshDiagnostics().catch(() => undefined);
  }, [bridge.refreshDiagnostics]);

  const entries = bridge.bootstrap?.diagnostics ?? [];
  const hasSession = Boolean(bridge.activeSession?.sessionId);
  const restartReason = restartDisabledReason(bridge.running, hasSession);

  const handleRestart = () => {
    setRestartError(null);
    setRestarting(true);
    void bridge.restartBridge()
      .catch((cause) => setRestartError(cause instanceof Error ? cause.message : '引擎无法重启。'))
      .finally(() => setRestarting(false));
  };

  return (
    <>
      <Card title="引擎状态">
        <Row title="状态摘要" desc={status?.status_line ?? '尚未收到 /status 快照。'} align="center">
          <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <button type="button" onClick={() => void bridge.refreshAuth().catch(() => undefined)} style={ghostButtonStyle(t)}>刷新 Auth</button>
            <button type="button" onClick={() => void bridge.refreshStatus().catch(() => undefined)} style={ghostButtonStyle(t)}>刷新状态</button>
            <button type="button" onClick={() => void bridge.refreshDoctor().catch(() => undefined)} style={ghostButtonStyle(t)}>刷新 Doctor</button>
          </div>
        </Row>
        <Row title="模型" desc={status?.model ?? bridge.desktop.currentModel ?? '—'} align="center">{null}</Row>
        <Row title="工作目录" desc={status?.cwd ?? bridge.activeSession?.projectPath ?? '—'} align="center">{null}</Row>
        <Row
          title="运行概况"
          desc={status
            ? `${status.n_messages} 条消息 · ${status.input_tokens + status.output_tokens} tok · $${status.total_cost_usd.toFixed(4)}`
            : '等待状态数据。'}
          align="center"
        >
          {null}
        </Row>
        <Row
          title="MCP / Hooks / Agents"
          desc={status
            ? `${status.n_mcp_connected}/${status.n_mcp_total} MCP，${status.n_hooks} hooks，${status.n_agents} agents`
            : '等待状态数据。'}
          align="center"
        >
          {null}
        </Row>
      </Card>

      <Card title="Doctor">
        <Row
          title="检查结果"
          desc={doctor
            ? `${doctor.summary.passed} 通过 · ${doctor.summary.warnings} 警告 · ${doctor.summary.failed} 失败`
            : '尚未收到 /doctor 报告。'}
          align="center"
        >
          <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
            <button type="button" onClick={() => void bridge.refresh().catch(() => undefined)} style={ghostButtonStyle(t)}>刷新全部</button>
            <button type="button" onClick={() => void bridge.refreshDiagnostics().catch(() => undefined)} style={ghostButtonStyle(t)}>刷新日志</button>
          </div>
        </Row>
        {doctor?.checks.map((check) => (
          <Row key={check.name} title={check.name} desc={check.detail ?? '无附加说明。'} align="center">
            <span style={{ color: check.status.type === 'fail' ? t.danger : check.status.type === 'warn' ? t.warn : t.ok, fontSize: 12.5, fontWeight: 600 }}>
              {check.status.type}
            </span>
          </Row>
        ))}
      </Card>

      <Card title="生命周期信号">
        <Row title="Auth" desc={auth?.type === 'signed_in' ? `${auth.email} · ${auth.org_id}` : '未登录'} align="center">
          <span className="mono" style={{ color: t.text3, fontSize: 11.5 }}>{auth?.type ?? 'unknown'}</span>
        </Row>
        <Row title="最近压缩" desc={compaction ? `${compaction.messages_before} → ${compaction.messages_after} 消息 · 节省 ${compaction.bytes_saved.toLocaleString()} bytes` : '暂无压缩完成事件。'} align="center">
          <button type="button" onClick={() => void bridge.forceCompact().catch(() => undefined)} disabled={!hasSession || bridge.running || bridge.sessionLoading} style={ghostButtonStyle(t, !hasSession || bridge.running || bridge.sessionLoading)}>
            立即压缩
          </button>
        </Row>
        <Row title="API 重试" desc={retry ? `${retry.message} · 第 ${retry.attempt}/${retry.max_retries} 次 · ${retry.delay_ms}ms` : '暂无 API 重试事件。'} align="center">
          <button type="button" onClick={() => { if (window.confirm('Clear the current session and start a new draft?')) void bridge.clearSession().catch(() => undefined); }} disabled={!hasSession || bridge.running || bridge.sessionLoading} style={ghostButtonStyle(t, !hasSession || bridge.running || bridge.sessionLoading)}>
            清空会话
          </button>
        </Row>
      </Card>

      <Card title="诊断">
        <Row title="日志" desc="仅包含经过脱敏的生命周期消息；提示词、工具负载与凭据值均已排除。" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <button type="button" onClick={() => void bridge.copyDiagnostics()} style={ghostButtonStyle(t)}>复制报告</button>
            <button type="button" onClick={() => void bridge.exportDiagnostics()} style={ghostButtonStyle(t)}>导出 JSON…</button>
            <button type="button" onClick={() => void bridge.refreshDiagnostics()} style={ghostButtonStyle(t)}>刷新</button>
          </div>
        </Row>
        <Row title="重启引擎" desc={restartReason ?? '如遇到异常状态，可在此手动重启引擎。'} align="center">
          <button
            type="button"
            onClick={handleRestart}
            disabled={restartReason !== null || restarting}
            style={ghostButtonStyle(t, restartReason !== null || restarting)}
          >
            {restarting ? '重启中…' : '重启引擎'}
          </button>
        </Row>
      </Card>

      {restartError && (
        <div role="alert" data-testid="diagnostics-restart-error" style={{ fontSize: 12.5, color: t.danger, marginTop: -18, marginBottom: 18 }}>
          重启失败：{restartError}
        </div>
      )}

      <Card title="日志条目">
        <div
          className="mono"
          data-testid="diagnostics-log"
          style={{ maxHeight: 260, overflow: 'auto', padding: '12px 18px', color: t.text3, fontSize: 11, lineHeight: 1.6 }}
        >
          {entries.length
            ? entries.map((entry, index) => (
              <div key={`${entry.timestamp}-${index}`}>
                <span style={{ color: levelColor(t, entry.level) }}>{entry.timestamp} [{entry.source}/{entry.level}]</span> {entry.message}
              </div>
            ))
            : '暂无诊断条目。'}
        </div>
      </Card>
    </>
  );
}
