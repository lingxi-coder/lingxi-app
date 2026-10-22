import { useCallback, useEffect, useState } from 'react';
import type { RemoteControlUiStatus, RemoteDeviceUi } from '../../../../shared/remoteControl';
import {
  connectionRows,
  platformLabel,
  remoteAvailability,
  remoteControlApi,
  remoteStateSummary,
  useRemoteControl,
} from '../../../bridge/remoteControl';
import { useT } from '../../../theme/ThemeContext';
import { Card, Row } from '../rows';
import { Toggle } from '../primitives';
import { ghostButtonStyle } from './ghostButton';

/**
 * Whether the "allow this computer" switch can be flipped. An administrator's
 * veto always locks it; a signed-out person can still switch it OFF (it may
 * have been left on) but not on.
 */
export function enabledSwitchDisabled(status: RemoteControlUiStatus): boolean {
  if (status.managedDisabled) return true;
  return !status.enabled && !remoteAvailability(status).available;
}

/** The sentence under the switch when it cannot be flipped, else `undefined`. */
export function enabledSwitchNote(status: RemoteControlUiStatus): string | undefined {
  if (status.managedDisabled) return '管理员已禁用远程控制，这里的开关不会生效。';
  if (!status.enabled && !status.signedIn) return '请先登录灵犀账号，再开启远程控制。';
  return undefined;
}

/** Approved devices, most recently seen first. */
export function deviceRows(devices: readonly RemoteDeviceUi[]): Array<RemoteDeviceUi & { platformName: string }> {
  return devices
    .slice()
    .sort((a, b) => b.lastSeen - a.lastSeen || a.name.localeCompare(b.name))
    .map((device) => ({ ...device, platformName: platformLabel(device.platform) }));
}

/** "刚刚", "N 分钟前", … or a date, for a last-seen time. */
export function lastSeenLabel(lastSeen: number, now = Date.now()): string {
  if (!Number.isFinite(lastSeen) || lastSeen <= 0) return '从未连接';
  const seconds = Math.max(0, Math.floor((now - lastSeen) / 1000));
  if (seconds < 60) return '刚刚';
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)} 小时前`;
  return new Date(lastSeen).toLocaleDateString();
}

export function RemoteControl() {
  const t = useT();
  const { status } = useRemoteControl();
  const api = remoteControlApi();
  const [atStartup, setAtStartup] = useState<boolean | null>(null);
  const [devices, setDevices] = useState<RemoteDeviceUi[]>([]);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  // The status carries `atStartup`; the separate read covers the moment before
  // the first status arrives, and the write result keeps both honest.
  const startupValue = status?.atStartup ?? atStartup ?? false;
  const approvedCount = status?.connections.filter((c) => c.approved).length ?? 0;

  const loadDevices = useCallback(() => {
    if (!api) return;
    void api.devices().then(setDevices).catch(() => undefined);
  }, [api]);

  // A device is approved while the page is open: reload when the set of connections changes.
  useEffect(loadDevices, [loadDevices, approvedCount, status?.connections.length]);

  if (!api) {
    return <Card><div style={{ padding: 18, color: t.text3, fontSize: 13 }}>这个版本的灵犀不支持远程控制。</div></Card>;
  }

  const run = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try { await action(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };

  const summary = status ? remoteStateSummary(status) : null;
  const toneColor = summary
    ? summary.tone === 'ok' ? t.ok : summary.tone === 'warn' ? t.warn : summary.tone === 'danger' ? t.danger : t.text3
    : t.text3;
  const note = status ? enabledSwitchNote(status) : undefined;
  const rows = deviceRows(devices);
  const connections = status ? connectionRows(status) : [];

  return (
    <div className="remote-control-settings">
      <section aria-label="远程控制说明" style={{
        marginBottom: 24, padding: '16px 18px', borderRadius: 12,
        background: t.surface, border: `0.5px solid ${t.border}`,
      }}>
        <h2 style={{ margin: 0, fontSize: 15, fontWeight: 600, color: t.text }}>在手机或浏览器上接着用灵犀</h2>
        <p style={{ margin: '8px 0 0', fontSize: 12.5, lineHeight: 1.6, color: t.text3, maxWidth: 620 }}>
          手机或浏览器可以查看并操控你开启了远程控制的会话；文件和工具始终留在这台电脑上，不会上传。
          在灵犀手机应用里，或用同一账号登录网页 /remote 即可连接。新设备第一次连接时，需要在这里输入它显示的 6 位配对码批准。
          远程控制按会话开启：在会话菜单里打开，或在对话中输入 /remote-control。
        </p>
      </section>

      {error && <p role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{error}</p>}

      <Card title="这台电脑">
        <Row
          title="允许远程控制这台电脑"
          desc={note
            ? <span role="note" id="remote-control-enabled-note">{note}</span>
            : '开启后，这台电脑会保持在线，等待你的设备连接。只有你开启了远程控制的会话才会被看到。'}
          align="center"
        >
          <Toggle
            value={status?.enabled ?? false}
            disabled={!status || busy || enabledSwitchDisabled(status)}
            label="允许远程控制这台电脑"
            onChange={(enabled) => void run(() => api.setEnabled(enabled))}
          />
        </Row>
        <Row title="状态" align="center" desc={summary?.detail ? <span role="alert">{summary.detail}</span> : undefined}>
          <span data-remote-state={status?.state ?? 'unknown'} style={{ fontSize: 13, fontWeight: 600, color: toneColor }}>
            {summary?.label ?? '读取中…'}
          </span>
        </Row>
        <Row
          title="所有新会话自动开启远程控制"
          desc="新建或打开的会话默认就能被你的设备看到。已有会话不受影响。"
          align="center"
        >
          <Toggle
            value={startupValue}
            disabled={busy || status?.managedDisabled === true}
            label="所有新会话自动开启远程控制"
            onChange={(enabled) => void run(async () => setAtStartup(await api.setAtStartup(enabled)))}
          />
        </Row>
      </Card>

      <Card title="当前连接">
        {connections.length === 0 && (
          <div role="status" style={{ padding: '16px 18px', fontSize: 13, color: t.text3 }}>没有设备正在连接。</div>
        )}
        {connections.map((connection, index) => (
          <div key={connection.connId} className="remote-control-connection" style={{
            display: 'flex', alignItems: 'center', gap: 12, padding: '12px 18px',
            borderTop: index === 0 ? undefined : `0.5px solid ${t.border}`,
          }}>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{connection.title}</div>
              <div style={{ fontSize: 12, color: connection.approved ? t.text3 : t.warn, marginTop: 3 }}>{connection.detail}</div>
            </div>
          </div>
        ))}
      </Card>

      <Card title="已批准的设备">
        {rows.length === 0 && (
          <div role="status" style={{ padding: '16px 18px', fontSize: 13, color: t.text3 }}>还没有批准过任何设备。</div>
        )}
        {rows.map((device, index) => (
          <div key={device.deviceId} className="remote-control-device" style={{
            display: 'flex', alignItems: 'center', gap: 12, padding: '12px 18px',
            borderTop: index === 0 ? undefined : `0.5px solid ${t.border}`,
          }}>
            <div style={{ flex: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
              <div style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{device.name || '未命名设备'}</div>
              <div style={{ fontSize: 12, color: t.text3, marginTop: 3 }}>
                {device.platformName} · 上次连接 {lastSeenLabel(device.lastSeen)}
              </div>
            </div>
            {confirming === device.deviceId ? (
              <div role="group" aria-label={`确认移除 ${device.name}`} style={{ display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0 }}>
                <span style={{ fontSize: 12, color: t.text2 }}>移除后它需要重新配对。</span>
                <button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)}
                  onClick={() => void run(async () => { setDevices(await api.revokeDevice(device.deviceId)); setConfirming(null); })}>
                  确认移除
                </button>
                <button type="button" disabled={busy} style={ghostButtonStyle(t, busy)} onClick={() => setConfirming(null)}>取消</button>
              </div>
            ) : (
              <button type="button" disabled={busy} aria-label={`移除 ${device.name}`}
                style={{ ...ghostButtonStyle(t, busy, true), flexShrink: 0 }} onClick={() => setConfirming(device.deviceId)}>
                移除
              </button>
            )}
          </div>
        ))}
      </Card>
    </div>
  );
}
