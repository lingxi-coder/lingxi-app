import { useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import type { ModelContextBudgetDto } from '@lingxi/bridge-client';
import type { UsageSnapshot } from '../bridge/conversation';
import { formatTokens } from '../formatTokens';
import { useT } from '../theme/ThemeContext';

export function contextWindowUsage(usage: UsageSnapshot | null, capacity?: number) {
  if (!usage || !capacity || !Number.isFinite(capacity) || capacity <= 0) return null;
  // Claude calculateContextPercentages counts the input window, not generated output.
  const counters = [usage.inputTokens, usage.cacheReadTokens, usage.cacheCreationTokens];
  if (counters.some(value => !Number.isFinite(value) || value < 0)) return null;
  const used = counters.reduce((sum, value) => sum + value, 0);
  if (!Number.isFinite(used)) return null;
  return { used, capacity, percent: Math.min(100, Math.round(used / capacity * 100)) };
}

/** Latest API context footprint; session-wide billing totals are deliberately separate. */
export function ContextWindow({ usage, capacity, maximum, priceBoundary, policy }: { usage: UsageSnapshot | null; capacity?: number; maximum?: number; priceBoundary?: number | null; policy?: ModelContextBudgetDto['policy'] }) {
  const t = useT();
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const tooltip = useRef<HTMLDivElement>(null);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const open = (hovered || focused) && !dismissed;
  const [position, setPosition] = useState({ left: 0, bottom: 0 });
  const snapshot = contextWindowUsage(usage, capacity);
  const summary = snapshot ? `${snapshot.percent}% used (${100 - snapshot.percent}% left)` : 'Usage unavailable';
  const tokens = snapshot ? `${formatTokens(snapshot.used)} / ${formatTokens(snapshot.capacity)} tokens used`
    : capacity && Number.isFinite(capacity) && capacity > 0 ? `${formatTokens(capacity)} token capacity` : 'Context size unavailable';

  useEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const height = tooltip.current?.getBoundingClientRect().height ?? 0;
      setPosition({
        left: Math.max(12, Math.min(rect.left + rect.width / 2 - 126, window.innerWidth - 264)),
        bottom: Math.max(12, Math.min(window.innerHeight - rect.top + 8, window.innerHeight - height - 12)),
      });
    };
    const dismiss = (event: globalThis.KeyboardEvent) => { if (event.key === 'Escape') setDismissed(true); };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    window.addEventListener('keydown', dismiss);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('keydown', dismiss);
    };
  }, [open, capacity, maximum, priceBoundary, policy, summary, tokens]);

  return <>
    <button ref={trigger} type="button" className="context-window-trigger"
      style={{ '--context-color': t.text3, '--context-track': t.border, '--context-focus': t.accent } as CSSProperties}
      aria-label={`Context window (estimated): ${summary}. ${tokens}`} aria-describedby={open ? id : undefined}
      onMouseEnter={() => { setHovered(true); setDismissed(false); }} onMouseLeave={() => setHovered(false)}
      onFocus={() => { setFocused(true); setDismissed(false); }} onBlur={() => setFocused(false)}
      onClick={() => { setDismissed(false); trigger.current?.focus(); }}>
      <svg width="16" height="16" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7" fill="none" stroke="var(--context-track)" strokeWidth="3" />
        {snapshot && snapshot.used > 0 && <circle cx="10" cy="10" r="7" fill="none" stroke="currentColor" strokeWidth="3"
          pathLength="100" strokeDasharray={`${Math.min(100, snapshot.used / snapshot.capacity * 100)} 100`}
          strokeLinecap="round" transform="rotate(-90 10 10)" />}
      </svg>
    </button>
    {open && createPortal(<div ref={tooltip} id={id} role="tooltip" className="context-window-tooltip" style={position}>
      <div className="context-window-heading">Context window:</div>
      <div>{summary}</div>
      <div>{tokens}</div>
      <div className="context-window-details">
        <div>最近一次请求输入 / 应用预算</div>
        {policy && <div>应用策略：{policy === 'long' ? '长上下文' : policy === 'custom' ? '自定义' : '自动'}</div>}
        {maximum != null && <div>模型最大上下文：{formatTokens(maximum)}</div>}
        {priceBoundary != null && <div>输入超过 {priceBoundary.toLocaleString()} tokens 后，整次请求适用长上下文价格。</div>}
      </div>
    </div>, document.body)}
  </>;
}
