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
