import { tr } from '../../../i18n/locale';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';

/**
 * App / Electron / engine version numbers. None of these were previously
 * reachable by the renderer — the old settings modal's own About section
 * (the thing this task's brief pointed at) only ever showed static prose, never real
 * numbers. The three values themselves were already computed in the main
 * process (for `diagnosticReport`'s `runtime`/`bridgeRuntime` fields, used
 * by `copyDiagnostics`/`exportDiagnostics`) but never exposed as structured
 * data — this task adds one `versions` field to the existing `bootstrap()`
 * payload (reusing that same expression) rather than a new IPC channel, so
 * this page's numbers and the exported diagnostic report can never disagree.
 */
export function About({ bridge }: PageContentProps) {
  const t = useT();
  const versions = bridge.bootstrap?.versions;
  const valueStyle = { fontSize: 12.5, color: t.text2 } as const;

  return (
    <>
      <Card title={tr('settings_group_about')}>
        <Row title={tr('desktop_app')} align="center"><span className="mono" style={valueStyle}>{versions?.app ?? tr('chat_status_unknown')}</span></Row>
        <Row title="Electron" align="center"><span className="mono" style={valueStyle}>{versions?.electron ?? tr('chat_status_unknown')}</span></Row>
        <Row
          title={tr('desktop_shell_engine')}
          desc={versions?.engine ? undefined : tr('desktop_shell_no_engine_version')}
          align="center"
        >
          <span className="mono" style={valueStyle}>
            {versions?.engine ? `${versions.engine.serverName} · ${versions.engine.serverProtocol}` : tr('settings_linux_task_unavailable')}
          </span>
        </Row>
      </Card>
      <Card title={tr('settings_data_privacy')}>
        <Row title={tr('privacy_policy_link')} desc={tr('desktop_shell_privacy_desc')} align="center"><a href="https://lingxios.net/privacy" target="_blank" rel="noreferrer" style={valueStyle}>{tr('desktop_shell_read_policy')}</a></Row>
        <Row title={tr('privacy_deletion_link')} desc={tr('desktop_shell_deletion_desc')} align="center"><a href="https://lingxios.net/data-deletion" target="_blank" rel="noreferrer" style={valueStyle}>{tr('desktop_shell_read_deletion')}</a></Row>
        <Row title={tr('desktop_shell_privacy_contact')} align="center"><span style={valueStyle}>support.lingxi@gmail.com</span></Row>
      </Card>
    </>
  );
}
