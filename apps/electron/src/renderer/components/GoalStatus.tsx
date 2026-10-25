import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useId, useState, type CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function GoalStatus({ objective, disabled, running, cancelling, onClear, onPause, onResume }: { objective: string; disabled: boolean; running: boolean; cancelling: boolean; onClear(): Promise<void>; onPause(): Promise<void>; onResume(): Promise<void> }) {
  const t = useT();
  useI18n();
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  const [pending, setPending] = useState(false);
  const [errorKey, setErrorKey] = useState('');
  const perform = async (action: () => Promise<void>, messageKey: string) => {
    if (pending || disabled || cancelling) return;
    setPending(true);
    setErrorKey('');
    try { await action(); }
    catch { setErrorKey(messageKey); }
    finally { setPending(false); }
  };
  const busy = disabled || pending || cancelling;
  return <section className="goal-status" style={{ '--goal-surface': t.surface, '--goal-border': t.border, '--goal-text': t.text, '--goal-muted': t.text3, '--goal-hover': t.surfaceHover, '--goal-focus': t.accent } as CSSProperties}>
    <div className="goal-status-row">
      <Icon name="goal" size={16} />
      <span className="goal-status-label" role="status" aria-label={tr("desktop_ui_goal_active")}>{cancelling ? tr("desktop_ui_goal_pausing") : running ? tr("desktop_ui_goal_pursuing") : tr("desktop_ui_goal_ready")}</span>
      <span className="goal-status-summary" title={objective}>{objective}</span>
      <button type="button" disabled={busy} aria-label={tr("desktop_ui_goal_clear")} title={tr("desktop_ui_goal_clear")} onClick={() => { void perform(onClear, 'desktop_ui_goal_delete_failed'); }}><Icon name="trash" size={16} /></button>
      <button type="button" disabled={busy} aria-label={running ? tr("desktop_ui_goal_pause") : tr("desktop_ui_goal_resume")} title={running ? tr("desktop_ui_goal_pause_hint") : tr("desktop_ui_goal_resume_hint")} onClick={() => { void perform(running ? onPause : onResume, 'desktop_ui_goal_update_failed'); }}><Icon name={running ? 'pauseCircle' : 'playCircle'} size={16} /></button>
      <button type="button" aria-label={expanded ? tr("desktop_ui_goal_collapse") : tr("desktop_ui_goal_expand")} title={expanded ? tr("desktop_ui_goal_collapse") : tr("desktop_ui_goal_expand")} aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(value => !value)}><Icon name={expanded ? 'collapse' : 'expand'} size={16} /></button>
    </div>
    {expanded && <div id={id} className="goal-status-detail">{objective || tr("desktop_ui_goal_hint")}</div>}
    {errorKey && <p className="goal-status-error" role="alert" style={{ color: t.danger }}>{tr(errorKey)}</p>}
  </section>;
}
