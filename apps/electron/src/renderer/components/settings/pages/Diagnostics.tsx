import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useState } from 'react';
import { Card, Row, ProvenanceBadge, type Provenance } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import type { DiagnosticEntry } from '../../../bridge/lingxi';
import { ghostButtonStyle } from './ghostButton';

function levelColor(t: ReturnType<typeof useT>, level: DiagnosticEntry['level']): string {
  if (level === 'error') return t.danger;
  if (level === 'warn') return t.warn;
  return t.text4;
}

function restartDisabledReason(running: boolean, hasSession: boolean): string | null {
  if (running) return tr('desktop_settings_b85c01ffdf');
  if (!hasSession) return tr('desktop_settings_7057ce7cd7');
  return null;
}

/**
 * Copy report / export JSON / refresh, lifted from the old settings modal's
 * Diagnostics section in `BetaDesktop.tsx` (`bridge.copyDiagnostics` /
 * `exportDiagnostics` / `refreshDiagnostics`, and the same sanitized-log
 * rendering). The restart action is NOT part of that lifted section —
 * that Diagnostics block had no restart button — it is added
 * here per this task's brief, reusing `bridge.restartBridge` while keeping the
 * manual recovery action inside Diagnostics instead of the settings shell.
 */
export function Diagnostics({ bridge, snapshot }: PageContentProps) {
  useI18n();
  const t = useT();
  const [restartError, setRestartError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
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
      .catch((cause) => setRestartError(cause instanceof Error ? cause.message : tr('desktop_settings_50f002ffe4')))
      .finally(() => setRestarting(false));
  };

  return (
    <>
      <Card title={tr('desktop_settings_3e3b45ef84')}>
        <Row title={tr('desktop_settings_8868595f7a')} desc={status?.status_line ?? tr('desktop_settings_fbb1434b70')} align="center">
          <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <button type="button" onClick={() => void bridge.refreshAuth().catch(() => undefined)} style={ghostButtonStyle(t)}>{tr('desktop_settings_88275872d7')}</button>
            <button type="button" onClick={() => void bridge.refreshStatus().catch(() => undefined)} style={ghostButtonStyle(t)}>{tr('settings_linux_action_refresh')}</button>
            <button type="button" onClick={() => void bridge.refreshDoctor().catch(() => undefined)} style={ghostButtonStyle(t)}>{tr('desktop_settings_03b7295905')}</button>
          </div>
        </Row>
        <Row title={tr('session_details_model')} desc={status?.model_source ?? status?.model ?? bridge.desktop.currentModelSource ?? bridge.desktop.currentModel ?? '—'} align="center">{null}</Row>
        <Row title={tr('desktop_settings_3db7b06b5f')} desc={status?.cwd ?? bridge.activeSession?.projectPath ?? '—'} align="center">{null}</Row>
        <Row
          title={tr('desktop_settings_7b9f9de918')}
          desc={status
            ? tr('desktop_settings_19cd5aee7f', status.n_messages, status.input_tokens + status.output_tokens, status.total_cost_usd.toFixed(4))
            : tr('desktop_settings_1d540202bb')}
          align="center"
        >
          {null}
        </Row>
        <Row
          title="MCP / Hooks / Agents"
          desc={status
            ? `${status.n_mcp_connected}/${status.n_mcp_total} MCP，${status.n_hooks} hooks，${status.n_agents} agents`
            : tr('desktop_settings_1d540202bb')}
          align="center"
        >
          {null}
        </Row>
      </Card>

      <Card title={tr('settings_parity_configuration_files')}>
        {!snapshot?.files.length && (
          <Row title={tr('desktop_settings_1c3b62a989')} desc={tr('desktop_settings_22f9363289')}>{null}</Row>
        )}
        {snapshot?.files.map((file) => (
          <Row key={file.layer}
            title={<span className="mono" style={{ fontSize: 12, overflowWrap: 'anywhere' }}>{file.path}</span>}
            badge={<ProvenanceBadge destination={file.layer as Provenance} />}
            desc={file.parse_error ? tr('desktop_settings_446c557ba8', file.parse_error) : file.exists ? tr('desktop_settings_f601c04359') : tr('desktop_settings_44563f11f3')}
            align="center"
          >
            <button type="button" data-testid={`settings-file-open-${file.layer}`} disabled={!file.exists}
              style={ghostButtonStyle(t, !file.exists)}
              onClick={async () => {
                setFileError(null);
                try {
                  if (!window.lingxi?.openSettingsFile) throw new Error(tr('desktop_settings_fa9623db95'));
                  await window.lingxi.openSettingsFile(file.path);
                } catch (cause) {
                  setFileError(cause instanceof Error ? cause.message : tr('desktop_settings_ccbc76ba35'));
                }
              }}>{tr('desktop_settings_732f1c9ab8')}</button>
          </Row>
        ))}
        <Row title={tr('desktop_settings_af4d9b8338')} desc={tr('desktop_settings_b30c2365b0')}
          badge={<ProvenanceBadge destination="managed" />}>{tr('chat_agent_read_only')}</Row>
        {fileError && <div role="alert" data-testid="settings-file-error" style={{ padding: '12px 18px', color: t.danger, fontSize: 12.5 }}>{fileError}</div>}
      </Card>

      <Card title="Doctor">
        <Row
          title={tr('desktop_settings_7c5569c38a')}
          desc={doctor
            ? tr('desktop_settings_1dc84177fc', doctor.summary.passed, doctor.summary.warnings, doctor.summary.failed)
            : tr('desktop_settings_7c704114e2')}
          align="center"
        >
          <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
            <button type="button" onClick={() => void bridge.refresh().catch(() => undefined)} style={ghostButtonStyle(t)}>{tr('desktop_settings_c6fae70942')}</button>
            <button type="button" onClick={() => void bridge.refreshDiagnostics().catch(() => undefined)} style={ghostButtonStyle(t)}>{tr('desktop_settings_cd4c216b51')}</button>
          </div>
        </Row>
        {doctor?.checks.map((check) => (
          <Row key={check.name} title={check.name} desc={check.detail ?? tr('desktop_settings_6da48c3698')} align="center">
            <span style={{ color: check.status.type === 'fail' ? t.danger : check.status.type === 'warn' ? t.warn : t.ok, fontSize: 12.5, fontWeight: 600 }}>
              {check.status.type}
            </span>
          </Row>
        ))}
      </Card>

      <Card title={tr('desktop_settings_4f89901316')}>
        <Row title="Auth" desc={auth?.type === 'signed_in' ? `${auth.email} · ${auth.org_id}` : tr('desktop_settings_c1f6583730')} align="center">
          <span className="mono" style={{ color: t.text3, fontSize: 11.5 }}>{auth?.type ?? 'unknown'}</span>
        </Row>
        <Row title={tr('desktop_settings_b5f0cf9e9b')} desc={compaction ? tr('desktop_settings_1d1bb081f7', compaction.messages_before, compaction.messages_after, compaction.bytes_saved.toLocaleString()) : tr('desktop_settings_7fb3cbad4b')} align="center">
          <button type="button" onClick={() => void bridge.forceCompact().catch(() => undefined)} disabled={!hasSession || bridge.running || bridge.sessionLoading} style={ghostButtonStyle(t, !hasSession || bridge.running || bridge.sessionLoading)}>
            {tr('desktop_settings_a02cf1b2d5')}</button>
        </Row>
        <Row title={tr('desktop_settings_7a7bd86646')} desc={retry ? tr('desktop_settings_3751325174', retry.message, retry.attempt, retry.max_retries, retry.delay_ms) : tr('desktop_settings_c921aadbf6')} align="center">
          <button type="button" onClick={() => void bridge.clearSession().catch(() => undefined)} disabled={!hasSession || bridge.running || bridge.sessionLoading} style={ghostButtonStyle(t, !hasSession || bridge.running || bridge.sessionLoading)}>
            {tr('desktop_settings_8f55dcc2e4')}</button>
        </Row>
      </Card>

      <Card title={tr('desktop_diagnostics')}>
        <Row title={tr('desktop_logs')} desc={tr('desktop_settings_abacae5f94')} align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <button type="button" onClick={() => void bridge.copyDiagnostics()} style={ghostButtonStyle(t)}>{tr('desktop_settings_9da1918cdc')}</button>
            <button type="button" onClick={() => void bridge.exportDiagnostics()} style={ghostButtonStyle(t)}>{tr('desktop_settings_65b030b277')}</button>
            <button type="button" onClick={() => void bridge.refreshDiagnostics()} style={ghostButtonStyle(t)}>{tr('settings_linux_refresh_button')}</button>
          </div>
        </Row>
        <Row title={tr('desktop_settings_6f31c4d8b1')} desc={restartReason ?? tr('desktop_settings_e728877f4c')} align="center">
          <button
            type="button"
            onClick={handleRestart}
            disabled={restartReason !== null || restarting}
            style={ghostButtonStyle(t, restartReason !== null || restarting)}
          >
            {restarting ? tr('desktop_settings_0501e33c02') : tr('desktop_settings_6f31c4d8b1')}
          </button>
        </Row>
      </Card>

      {restartError && (
        <div role="alert" data-testid="diagnostics-restart-error" style={{ fontSize: 12.5, color: t.danger, marginTop: -18, marginBottom: 18 }}>
          {tr('desktop_settings_175d90197d')}{restartError}
        </div>
      )}

      <Card title={tr('desktop_settings_a55230ff9c')}>
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
            : tr('desktop_settings_6e43725444')}
        </div>
      </Card>
    </>
  );
}
