import { tr } from '../../../i18n/locale';
import { useState, type CSSProperties } from 'react';
import { Icon } from '../../Icon';
import { Card, Row } from '../rows';
import { Toggle } from '../primitives';
import { useT } from '../../../theme/ThemeContext';
import {
  DEFAULT_IDLE_NOTIF_THRESHOLD_MS,
  defaultNotificationPreferences,
  type NotificationPreferences,
} from '../../../../shared/notificationPreferences';
import type { PageContentProps } from '../SettingsScreen';

/**
 * The idle-threshold choices, in render order. Pure and exported so the set
 * and its ordering can be pinned without mounting anything.
 *
 * 60s is upstream Claude Code's `messageIdleNotifThresholdMs` default and is
 * labelled as such; the others exist because a threshold that can only be the
 * default is not a setting.
 */
export function idleThresholdOptions(): { ms: number; label: string }[] {
  return [
    { ms: 15_000, label: tr('desktop_shell_seconds', 15) },
    { ms: DEFAULT_IDLE_NOTIF_THRESHOLD_MS, label: tr('desktop_shell_minutes', 1) },
    { ms: 180_000, label: tr('desktop_shell_minutes', 3) },
    { ms: 600_000, label: tr('desktop_shell_minutes', 10) },
  ];
}

/**
 * Which toggles a person can still reach once the master switch is off.
 * Exported for the same reason: it is the one rule this page enforces, and
 * "the sub-toggles keep responding after you turn notifications off" is a bug
 * that renders perfectly.
 */
export function rowsDisabled(prefs: NotificationPreferences): boolean {
  return !prefs.enabled;
}

export function Notifications({ bridge }: PageContentProps) {
  const t = useT();
  const [preview, setPreview] = useState(0);
  const prefs = bridge.bootstrap?.settings?.notifications ?? defaultNotificationPreferences();
  const disabled = rowsDisabled(prefs);

  const write = (patch: Partial<NotificationPreferences>) => {
    void bridge.setNotificationPreferences({ ...prefs, ...patch }).catch(() => undefined);
  };

  return (
    <div className="notification-settings" style={{
      '--notif-surface': t.surface, '--notif-bg': t.sidebarBg, '--notif-text': t.text,
      '--notif-secondary': t.text2, '--notif-muted': t.text3, '--notif-border': t.border,
      '--notif-accent': t.accent,
    } as CSSProperties}>
      <section className="notification-settings-hero" aria-label={tr('desktop_shell_notifications_preview')}>
        <div className="notification-settings-intro">
          <span className="notification-settings-symbol"><Icon name="bell" size={23} /></span>
          <h2>{tr('desktop_shell_notifications_heading')}</h2>
          <p>{tr('desktop_shell_notifications_subheading')}</p>
        </div>
        <div className="notification-preview-stage">
          <div className="notification-preview-card">
            <img src={new URL('../../../assets/app-icon.png', import.meta.url).href} width={40} height={40} alt="" />
            <div className="notification-preview-copy">
              <div className="notification-preview-meta"><span>LingXi Code</span><span>{tr('desktop_shell_now')}</span></div>
              <strong>{[tr('chat_background_waiting_title'), tr('chat_background_permission_title'), tr('chat_background_completed_title')][preview]}</strong>
              <p>{[tr('chat_background_waiting_text'), tr('desktop_shell_permission_preview'), tr('desktop_shell_completed_preview')][preview]}</p>
            </div>
          </div>
          <div className="notification-preview-options" role="group" aria-label={tr('desktop_shell_notification_types')}>
            {[tr('desktop_shell_waiting_input'), tr('desktop_shell_permission_needed'), tr('desktop_shell_task_complete')].map((label, index) => (
              <button key={label} type="button" aria-pressed={preview === index} onClick={() => setPreview(index)}>{label}</button>
            ))}
          </div>
          <p className="notification-preview-caption">{tr('desktop_shell_preview_caption')}</p>
        </div>
      </section>
      <Card title={tr('settings_notifications')}>
        <Row
          title={tr('settings_notif_enabled')}
          desc={tr('desktop_shell_notif_enabled_desc')}
          align="center"
        >
          <Toggle value={prefs.enabled} onChange={(enabled) => write({ enabled })} label={tr('settings_notif_enabled')} />
        </Row>
      </Card>

      <fieldset className="notification-settings-dependent" disabled={disabled}>
        <Card title={tr('settings_section_notification_type')}>
          <Row
            title={tr('settings_notif_idle_prompt')}
            desc={tr('desktop_shell_notif_idle_desc')}
            align="center"
          >
            <Toggle
              value={prefs.idlePromptNotifEnabled && !disabled}
              onChange={(v) => !disabled && write({ idlePromptNotifEnabled: v })}
              label={tr('settings_notif_idle_prompt')}
            />
          </Row>
          <Row
            title={tr('settings_notif_needs_input')}
            desc={tr('desktop_shell_notif_permission_desc')}
            align="center"
          >
            <Toggle
              value={prefs.inputNeededNotifEnabled && !disabled}
              onChange={(v) => !disabled && write({ inputNeededNotifEnabled: v })}
              label={tr('settings_notif_needs_input')}
            />
          </Row>
          <Row
            title={tr('settings_notif_background_task')}
            desc={tr('desktop_shell_notif_background_desc')}
            align="center"
          >
            <Toggle
              value={prefs.taskCompleteNotifEnabled && !disabled}
              onChange={(v) => !disabled && write({ taskCompleteNotifEnabled: v })}
              label={tr('settings_notif_background_task')}
            />
          </Row>
          <Row
            title={tr('settings_notif_cron_report')}
            desc={tr('desktop_shell_notif_scheduled_desc')}
            align="center"
          >
            <Toggle
              value={prefs.scheduledRunNotifEnabled && !disabled}
              onChange={(v) => !disabled && write({ scheduledRunNotifEnabled: v })}
              label={tr('settings_notif_cron_report')}
            />
          </Row>
        </Card>

        <Card title={tr('desktop_shell_notif_timing')}>
          <Row
            title={tr('settings_notif_idle_threshold')}
            desc={tr('desktop_shell_notif_timing_desc')}
            align="center"
          >
            <div className="notification-timing-options" role="group" aria-label={tr('settings_notif_idle_threshold')}>
              {idleThresholdOptions().map((option) => {
                const active = prefs.messageIdleNotifThresholdMs === option.ms;
                return (
                  <button
                    key={option.ms}
                    type="button"
                    data-idle-threshold={option.ms}
                    aria-pressed={active}
                    onClick={() => !disabled && write({ messageIdleNotifThresholdMs: option.ms })}
                  >
                    {option.label}
                  </button>
                );
              })}
            </div>
          </Row>
        </Card>
      </fieldset>
      <p className="notification-settings-footnote"><Icon name="info" size={14} />
        {tr('desktop_shell_notif_system_desc')}
      </p>
    </div>
  );
}
