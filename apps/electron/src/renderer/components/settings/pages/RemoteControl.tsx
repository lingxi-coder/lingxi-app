import { currentLocale, tr } from '../../../i18n/locale';
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
  if (status.managedDisabled) return tr('desktop_shell_remote_managed');
  if (!status.enabled && !status.signedIn) return tr('desktop_shell_remote_signin');
  return undefined;
}

/** Approved devices, most recently seen first. */
export function deviceRows(devices: readonly RemoteDeviceUi[]): Array<RemoteDeviceUi & { platformName: string }> {
  return devices
    .slice()
    .sort((a, b) => b.lastSeen - a.lastSeen || a.name.localeCompare(b.name, currentLocale()))
    .map((device) => ({ ...device, platformName: platformLabel(device.platform) }));
}

/** "刚刚", "N 分钟前", … or a date, for a last-seen time. */
export function lastSeenLabel(lastSeen: number, now = Date.now()): string {
  if (!Number.isFinite(lastSeen) || lastSeen <= 0) return tr('desktop_shell_remote_never');
  const seconds = Math.max(0, Math.floor((now - lastSeen) / 1000));
  if (seconds < 60) return tr('project_relative_time_just_now');
  if (seconds < 3600) return new Intl.RelativeTimeFormat(currentLocale(), { numeric: 'auto' }).format(-Math.floor(seconds / 60), 'minute');
  if (seconds < 86_400) return new Intl.RelativeTimeFormat(currentLocale(), { numeric: 'auto' }).format(-Math.floor(seconds / 3600), 'hour');
  return new Date(lastSeen).toLocaleDateString(currentLocale());
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
    return <Card><div style={{ padding: 18, color: t.text3, fontSize: 13 }}>{tr('desktop_shell_remote_unsupported')}</div></Card>;
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
      <section aria-label={tr('desktop_shell_remote_info')} style={{
        marginBottom: 24, padding: '16px 18px', borderRadius: 12,
        background: t.surface, border: `0.5px solid ${t.border}`,
      }}>
        <h2 style={{ margin: 0, fontSize: 15, fontWeight: 600, color: t.text }}>{tr('desktop_shell_remote_anywhere')}</h2>
        <p style={{ margin: '8px 0 0', fontSize: 12.5, lineHeight: 1.6, color: t.text3, maxWidth: 620 }}>
          {tr('desktop_shell_remote_description')}
        </p>
      </section>

      {error && <p role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{error}</p>}

      <Card title={tr('desktop_shell_this_computer')}>
        <Row
          title={tr('desktop_shell_remote_enable')}
          desc={note
            ? <span role="note" id="remote-control-enabled-note">{note}</span>
            : tr('desktop_shell_remote_enable_desc')}
          align="center"
        >
          <Toggle
            value={status?.enabled ?? false}
            disabled={!status || busy || enabledSwitchDisabled(status)}
            label={tr('desktop_shell_remote_enable')}
            onChange={(enabled) => void run(() => api.setEnabled(enabled))}
          />
        </Row>
        <Row title={tr('session_details_status')} align="center" desc={summary?.detail ? <span role="alert">{summary.detail}</span> : undefined}>
          <span data-remote-state={status?.state ?? 'unknown'} style={{ fontSize: 13, fontWeight: 600, color: toneColor }}>
            {summary?.label ?? tr('desktop_shell_reading')}
          </span>
        </Row>
        <Row
          title={tr('desktop_shell_remote_auto')}
          desc={tr('desktop_shell_remote_auto_desc')}
          align="center"
        >
          <Toggle
            value={startupValue}
            disabled={busy || status?.managedDisabled === true}
            label={tr('desktop_shell_remote_auto')}
            onChange={(enabled) => void run(async () => setAtStartup(await api.setAtStartup(enabled)))}
          />
        </Row>
      </Card>

      <Card title={tr('desktop_shell_connections')}>
        {connections.length === 0 && (
          <div role="status" style={{ padding: '16px 18px', fontSize: 13, color: t.text3 }}>{tr('desktop_shell_no_connections')}</div>
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

      <Card title={tr('desktop_shell_approved_devices')}>
        {rows.length === 0 && (
          <div role="status" style={{ padding: '16px 18px', fontSize: 13, color: t.text3 }}>{tr('desktop_shell_no_approved_devices')}</div>
        )}
        {rows.map((device, index) => (
          <div key={device.deviceId} className="remote-control-device" style={{
            display: 'flex', alignItems: 'center', gap: 12, padding: '12px 18px',
            borderTop: index === 0 ? undefined : `0.5px solid ${t.border}`,
          }}>
            <div style={{ flex: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
              <div style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{device.name || tr('desktop_shell_unnamed_device')}</div>
              <div style={{ fontSize: 12, color: t.text3, marginTop: 3 }}>
                {device.platformName} · {tr('desktop_shell_last_connected', lastSeenLabel(device.lastSeen))}
              </div>
            </div>
            {confirming === device.deviceId ? (
              <div role="group" aria-label={tr('desktop_shell_confirm_remove_named', device.name)} style={{ display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0 }}>
                <span style={{ fontSize: 12, color: t.text2 }}>{tr('desktop_shell_remove_device_desc')}</span>
                <button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)}
                  onClick={() => void run(async () => { setDevices(await api.revokeDevice(device.deviceId)); setConfirming(null); })}>
                  {tr('desktop_shell_confirm_remove')}
                </button>
                <button type="button" disabled={busy} style={ghostButtonStyle(t, busy)} onClick={() => setConfirming(null)}>{tr('common_cancel')}</button>
              </div>
            ) : (
              <button type="button" disabled={busy} aria-label={tr('desktop_shell_remove_named', device.name)}
                style={{ ...ghostButtonStyle(t, busy, true), flexShrink: 0 }} onClick={() => setConfirming(device.deviceId)}>
                {tr('settings_parity_remove')}
              </button>
            )}
          </div>
        ))}
      </Card>
    </div>
  );
}
