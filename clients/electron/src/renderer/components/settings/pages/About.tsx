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
    <Card title="关于">
      <Row title="应用" align="center"><span className="mono" style={valueStyle}>{versions?.app ?? '未知'}</span></Row>
      <Row title="Electron" align="center"><span className="mono" style={valueStyle}>{versions?.electron ?? '未知'}</span></Row>
      <Row
        title="引擎"
        desc={versions?.engine ? undefined : '尚未连接过引擎，暂无版本信息。'}
        align="center"
      >
        <span className="mono" style={valueStyle}>
          {versions?.engine ? `${versions.engine.serverName} · ${versions.engine.serverProtocol}` : '不可用'}
        </span>
      </Row>
    </Card>
  );
}
