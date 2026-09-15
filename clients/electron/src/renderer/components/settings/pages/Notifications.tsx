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
    { ms: 15_000, label: '15 秒' },
    { ms: DEFAULT_IDLE_NOTIF_THRESHOLD_MS, label: '1 分钟' },
    { ms: 180_000, label: '3 分钟' },
    { ms: 600_000, label: '10 分钟' },
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
  const prefs = bridge.bootstrap?.settings?.notifications ?? defaultNotificationPreferences();
  const disabled = rowsDisabled(prefs);

  const write = (patch: Partial<NotificationPreferences>) => {
    void bridge.setNotificationPreferences({ ...prefs, ...patch }).catch(() => undefined);
  };

  return (
    <>
      <Card title="通知">
        <Row
          title="启用通知"
          desc="关闭后灵犀不再发送任何系统通知。通知只在灵犀窗口不在前台时出现。"
          align="center"
        >
          <Toggle value={prefs.enabled} onChange={(enabled) => write({ enabled })} label="启用通知" />
        </Row>
      </Card>

      <Card title="通知类型">
        <Row
          title="等待你的输入"
          desc="回合结束后你一直没回来时提醒。中途回到窗口就不会提醒。"
          align="center"
        >
          <Toggle
            value={prefs.idlePromptNotifEnabled && !disabled}
            onChange={(v) => !disabled && write({ idlePromptNotifEnabled: v })}
            label="等待你的输入"
          />
        </Row>
        <Row
          title="需要我操作"
          desc="请求权限或向你提问时提醒。权限提示在 6 秒内答复则不会提醒。"
          align="center"
        >
          <Toggle
            value={prefs.inputNeededNotifEnabled && !disabled}
            onChange={(v) => !disabled && write({ inputNeededNotifEnabled: v })}
            label="需要我操作"
          />
        </Row>
        <Row
          title="后台任务完成"
          desc="后台 Agent 或命令结束时提醒。"
          align="center"
        >
          <Toggle
            value={prefs.taskCompleteNotifEnabled && !disabled}
            onChange={(v) => !disabled && write({ taskCompleteNotifEnabled: v })}
            label="后台任务完成"
          />
        </Row>
        <Row
          title="定时任务报告"
          desc="定时任务执行结束后提醒。每个任务自己的通知策略仍然生效。"
          align="center"
        >
          <Toggle
            value={prefs.scheduledRunNotifEnabled && !disabled}
            onChange={(v) => !disabled && write({ scheduledRunNotifEnabled: v })}
            label="定时任务报告"
          />
        </Row>
      </Card>

      <Card title="时机">
        <Row
          title="闲置提醒等待时间"
          desc="回合结束后等待多久才提醒。等待期间你回到窗口，就不会提醒。"
          align="center"
        >
          <div style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
            {idleThresholdOptions().map((option) => {
              const active = prefs.messageIdleNotifThresholdMs === option.ms;
              return (
                <button
                  key={option.ms}
                  type="button"
                  data-idle-threshold={option.ms}
                  aria-pressed={active}
                  onClick={() => !disabled && write({ messageIdleNotifThresholdMs: option.ms })}
                  style={{
                    padding: '5px 14px', borderRadius: 7, border: 'none', fontFamily: 'inherit',
                    cursor: disabled ? 'default' : 'pointer',
                    background: active ? t.surface : 'transparent',
                    color: active ? t.text : t.text3,
                    opacity: disabled ? 0.5 : 1,
                    fontSize: 12.5, fontWeight: active ? 600 : 500,
                  }}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
        </Row>
      </Card>
    </>
  );
}
