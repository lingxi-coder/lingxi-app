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
      <section className="notification-settings-hero" aria-label="通知预览">
        <div className="notification-settings-intro">
          <span className="notification-settings-symbol"><Icon name="bell" size={23} /></span>
          <h2>重要进展，恰时提醒</h2>
          <p>离开窗口时，依然知道哪些事情需要你。</p>
        </div>
        <div className="notification-preview-stage">
          <div className="notification-preview-card">
            <img src={new URL('../../../assets/app-icon.png', import.meta.url).href} width={40} height={40} alt="" />
            <div className="notification-preview-copy">
              <div className="notification-preview-meta"><span>LingXi Code</span><span>现在</span></div>
              <strong>{['灵犀正在等待你', '灵犀需要你的许可', '灵犀已完成'][preview]}</strong>
              <p>{['后台对话需要你的输入才能继续', '灵犀需要你的许可来使用终端', '代码检查已完成，点按查看结果'][preview]}</p>
            </div>
          </div>
          <div className="notification-preview-options" role="group" aria-label="预览通知类型">
            {['等待输入', '需要许可', '任务完成'].map((label, index) => (
              <button key={label} type="button" aria-pressed={preview === index} onClick={() => setPreview(index)}>{label}</button>
            ))}
          </div>
          <p className="notification-preview-caption">外观示意 · 实际样式由系统控制，不会发送通知</p>
        </div>
      </section>
      <Card title="通知">
        <Row
          title="启用通知"
          desc="在这台设备上接收系统通知。普通对话仅在窗口不在前台时提醒；定时任务按自己的通知策略提醒。"
          align="center"
        >
          <Toggle value={prefs.enabled} onChange={(enabled) => write({ enabled })} label="启用通知" />
        </Row>
      </Card>

      <fieldset className="notification-settings-dependent" disabled={disabled}>
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

        <Card title="提醒时机">
          <Row
            title="闲置提醒等待时间"
            desc="回合结束后等待多久才提醒。等待期间你回到窗口，就不会提醒。"
            align="center"
          >
            <div className="notification-timing-options" role="group" aria-label="闲置提醒等待时间">
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
        系统通知的横幅、声音与显示权限，可在系统设置中调整。
      </p>
    </div>
  );
}
