import { useId, useState, type CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function GoalStatus({ objective, disabled, running, cancelling, onClear, onPause, onResume }: { objective: string; disabled: boolean; running: boolean; cancelling: boolean; onClear(): Promise<void>; onPause(): Promise<void>; onResume(): Promise<void> }) {
  const t = useT();
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const perform = async (action: () => Promise<void>, message: string) => {
    if (pending || disabled || cancelling) return;
    setPending(true);
    setError('');
    try { await action(); }
    catch { setError(message); }
    finally { setPending(false); }
  };
  const busy = disabled || pending || cancelling;
  return <section className="goal-status" style={{ '--goal-surface': t.surface, '--goal-border': t.border, '--goal-text': t.text, '--goal-muted': t.text3, '--goal-hover': t.surfaceHover, '--goal-focus': t.accent } as CSSProperties}>
    <div className="goal-status-row">
      <Icon name="goal" size={16} />
      <span className="goal-status-label" role="status" aria-label="Goal active">{cancelling ? 'Pausing goal…' : running ? 'Pursuing goal' : 'Goal ready'}</span>
      <span className="goal-status-summary" title={objective}>{objective}</span>
      <button type="button" disabled={busy} aria-label="Clear goal" title="Clear goal" onClick={() => { void perform(onClear, 'Could not delete this goal. Try again.'); }}><Icon name="trash" size={16} /></button>
      <button type="button" disabled={busy} aria-label={running ? 'Pause goal' : 'Resume goal'} title={running ? 'Pause execution and keep goal' : 'Continue pursuing goal'} onClick={() => { void perform(running ? onPause : onResume, 'Could not change goal execution. Try again.'); }}><Icon name={running ? 'pauseCircle' : 'playCircle'} size={16} /></button>
      <button type="button" aria-label={expanded ? 'Collapse goal' : 'Expand goal'} title={expanded ? 'Collapse goal' : 'Expand goal'} aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(value => !value)}><Icon name={expanded ? 'collapse' : 'expand'} size={16} /></button>
    </div>
    {expanded && <div id={id} className="goal-status-detail">{objective || 'A goal is active for this conversation.'}</div>}
    {error && <p className="goal-status-error" role="alert" style={{ color: t.danger }}>{error}</p>}
  </section>;
}
