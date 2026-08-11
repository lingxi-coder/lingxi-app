/**
 * The model's working plan, pinned between the transcript and the composer.
 *
 * Mounted as a third `flexShrink: 0` sibling inside `<main>` so it SHRINKS the
 * scroll viewport rather than floating over it — an overlay would cover the
 * newest tool output at exactly the moment the plan changes, which is when you
 * most want to see both.
 *
 * It is NOT the same thing as the right-hand `BetaTasks` aside: that lists
 * background tasks (spawned agents and shells), while this is the model's own
 * todo checklist for the current turn. They coexist on purpose.
 */

import { memo } from 'react';
import type { PlanTaskDto } from '@lingxi/bridge-client';

import { useT } from '../theme/ThemeContext';
import { planGlyph, planOverflowSummary, planWindow } from './planOverflow';

export interface PlanTasksProps {
  tasks: readonly PlanTaskDto[];
}

export const PlanTasks = memo(function PlanTasks({ tasks }: PlanTasksProps) {
  const t = useT();
  if (tasks.length === 0) return null;

  const { visible, hidden } = planWindow(tasks);
  const overflow = planOverflowSummary(hidden);
  const done = tasks.filter((task) => task.state === 'completed').length;

  return (
    <div
      aria-label="Working plan"
      style={{
        flexShrink: 0,
        borderTop: `0.5px solid ${t.border}`,
        background: t.surface,
        padding: '8px 16px 9px',
        maxHeight: '38vh',
        overflowY: 'auto',
      }}
    >
      <div
        style={{
          display: 'flex', alignItems: 'center', gap: 8, marginBottom: 5,
          fontSize: 10.5, fontWeight: 600, letterSpacing: 0.6,
          textTransform: 'uppercase', color: t.text4,
        }}
      >
        <span>Plan</span>
        <span className="mono" style={{ letterSpacing: 0, textTransform: 'none' }}>
          {done}/{tasks.length}
        </span>
      </div>
      <ul style={{ listStyle: 'none', display: 'flex', flexDirection: 'column', gap: 2 }}>
        {visible.map((task, i) => {
          const active = task.state === 'in_progress';
          const complete = task.state === 'completed';
          return (
            <li
              key={task.id ?? `${i}:${task.subject}`}
              style={{
                display: 'flex', alignItems: 'baseline', gap: 8,
                fontSize: 12.5, lineHeight: 1.5,
                color: active ? t.text : complete ? t.text4 : t.text2,
                fontWeight: active ? 600 : 400,
                textDecoration: complete ? 'line-through' : 'none',
              }}
            >
              <span
                aria-hidden="true"
                style={{ flexShrink: 0, color: active ? t.accent : complete ? t.ok : t.text4 }}
              >
                {planGlyph(task.state)}
              </span>
              <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {/* `active_form` is the status-line spelling ("Porting the
                    renderer"), not the row spelling — the row keeps `subject`. */}
                {task.subject}
              </span>
            </li>
          );
        })}
        {overflow && (
          <li style={{ fontSize: 11.5, color: t.text4, paddingLeft: 20 }}>… {overflow}</li>
        )}
      </ul>
    </div>
  );
});
