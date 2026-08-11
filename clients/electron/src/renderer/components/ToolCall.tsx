/**
 * One tool call in the transcript — replaces the old `AgentCard`.
 *
 * The card renders the engine's PRE-DERIVED view and nothing else. It never
 * looks at `input_json` or `result_json`; `view` (a `ToolHeaderDto`) and
 * `result` (a `ToolResultDisplayDto`) arrived already composed, which is the
 * whole point of the change — four clients each summarising the same payload
 * had drifted into four different answers.
 *
 * Three details worth keeping:
 *
 *  - **The title truncates tail-FIRST.** `Update(/very/long/path/to/host.rs)`
 *    must keep `host.rs` when it overflows, so the argument is laid out
 *    `direction: rtl` and clipped at its head. (The parentheses are rendered as
 *    separate LTR runs so bidi does not reorder them around the path, and the
 *    path itself is LRM-fenced — see `bidi.ts` for why an unfenced absolute
 *    path renders with its leading `/` moved to the END.)
 *  - **No body ⇒ no chevron.** The previous card set `expandable: true`
 *    unconditionally, so an empty result offered a disclosure that opened onto
 *    nothing.
 *  - **Open/closed state is owned by the caller.** This list recycles rows;
 *    row-local state would be reassigned to a different call on scroll.
 */

import { memo, type CSSProperties } from 'react';

import type { ToolRunItem } from '../model/runItem';
import { toolDefaultOpen, toolHasBody, toolTruncationNotice } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { ltrAnchored } from './bidi';
import { DiffView } from './DiffView';
import { Disclosure } from './Disclosure';
import { Icon } from './Icon';

const CARD_STYLE: CSSProperties = Object.freeze({
  display: 'flex',
  flexDirection: 'column',
  gap: 4,
  maxWidth: 720,
  position: 'relative',
});

const TITLE_STYLE: CSSProperties = Object.freeze({
  fontSize: 14,
  fontWeight: 500,
  minWidth: 0,
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
  // Clip the HEAD of a long argument, not its tail — a path's filename is the
  // part that identifies it.
  direction: 'rtl',
  textAlign: 'left',
});

const BODY_STYLE: CSSProperties = Object.freeze({
  margin: '6px 0 0 22px',
  padding: 10,
  maxHeight: 260,
  overflow: 'auto',
  whiteSpace: 'pre-wrap',
  overflowWrap: 'anywhere',
  borderRadius: 7,
  fontSize: 11.5,
  lineHeight: 1.5,
});

export interface ToolCallProps {
  item: ToolRunItem;
  /** Explicit user choice, or `undefined` to use the engine's default. */
  open?: boolean;
  /** Record an explicit choice for this id in the caller's store. */
  onSetOpen(id: string, next: boolean): void;
}

/** `12s` / `1m 04s` — the heartbeat clock for a still-running call. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1_000));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}m ${String(seconds).padStart(2, '0')}s`;
}

function StatusDot({ status }: { status: ToolRunItem['status'] }) {
  const t = useT();
  if (status === 'running') {
    return (
      <span
        aria-label="running"
        style={{
          width: 10, height: 10, borderRadius: 99, background: t.accent, flexShrink: 0,
          boxShadow: `0 0 0 4px color-mix(in oklab, ${t.accent} 22%, transparent)`,
          animation: 'shimmer 1.3s infinite',
        }}
      />
    );
  }
  const failed = status === 'error';
  return (
    <span
      aria-label={failed ? 'failed' : 'done'}
      style={{
        width: 14, height: 14, borderRadius: 4, flexShrink: 0,
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
        background: failed ? t.danger : t.ok, color: t.windowBg,
      }}
    >
      <Icon name={failed ? 'x' : 'check'} size={10} stroke={3} />
    </span>
  );
}

export const ToolCall = memo(function ToolCall({ item, open, onSetOpen }: ToolCallProps) {
  const t = useT();
  const { view, result } = item;
  const expandable = toolHasBody(item);
  const isOpen = open ?? toolDefaultOpen(item);

  // Compose from the parts so the argument can be truncated on its own; fall
  // back to the pre-composed English `title` when there is no primary.
  const title = view.primary
    ? (
      <>
        <span dir="ltr">{`${view.label}(`}</span>
        {/* LRM-fenced: `direction: rtl` would otherwise send the leading `/` of
            an absolute path to the far right. */}
        <span style={TITLE_STYLE}>{ltrAnchored(view.primary)}</span>
        <span dir="ltr">{`)${view.qualifier ?? ''}`}</span>
      </>
    )
    : <span>{view.title}</span>;

  const headline = result?.headline;
  const body = result?.body ?? item.note;
  const diff = result?.diff;
  // `body_lines` is the count BEFORE clamping, so the affordance can promise
  // the right number without measuring anything.
  const lines = result && result.body_lines > 0 ? result.body_lines : undefined;

  // The clamped body is the ONLY place the promise made by `Show N lines` can
  // be broken, so the truncation has to be visible in both states: in the
  // affordance while closed, and under the body once open. Announcing it only
  // while hidden left the user staring at fewer lines than they were promised
  // with nothing to explain the difference.
  const truncationNotice = toolTruncationNotice(item);

  const summary = (
    <span style={{ fontSize: 12, color: t.text3 }}>
      {isOpen
        ? 'Hide'
        : diff
          ? `Show diff (+${diff.additions} −${diff.removals})`
          : lines !== undefined
            ? `Show ${lines} ${lines === 1 ? 'line' : 'lines'}`
            : 'Show output'}
      {truncationNotice ? ' (truncated)' : ''}
    </span>
  );

  return (
    <div style={CARD_STYLE}>
      {/* Header line: status, title, elapsed clock. */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
        <StatusDot status={item.status} />
        <span
          style={{
            display: 'flex', minWidth: 0, alignItems: 'baseline',
            fontSize: 14, fontWeight: 500,
            color: item.status === 'error' ? t.danger : t.text,
          }}
        >
          {title}
        </span>
        {item.status === 'running' && item.elapsedMs !== undefined && (
          <span className="mono" style={{ fontSize: 11, color: t.text4, flexShrink: 0 }}>
            {formatElapsed(item.elapsedMs)}
          </span>
        )}
      </div>

      {/* The header's own second line, e.g. `$ cargo test --all`. */}
      {view.sub_line && (
        <div
          className="mono-code"
          style={{
            display: 'flex', gap: 6, marginLeft: 22, fontSize: 12, color: t.text2,
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}
        >
          <span style={{ color: t.text4, flexShrink: 0 }}>{view.sub_line.prefix}</span>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{view.sub_line.text}</span>
        </div>
      )}

      {/* The result headline — the terminal's `⎿` line. */}
      {headline && (
        <div
          style={{
            display: 'flex', gap: 6, marginLeft: 22, fontSize: 13,
            color: item.status === 'error' ? t.danger : t.text2,
          }}
        >
          <span style={{ color: t.text4, flexShrink: 0 }} aria-hidden="true">⎿</span>
          <span>{headline}</span>
        </div>
      )}

      {/*
        Only a call with real content earns a disclosure — the old card set
        `expandable: true` unconditionally, so an empty result showed a chevron
        that opened onto nothing. Both the diff and the body live INSIDE it: a
        diff can carry up to `MAX_WIRE_DIFF_ROWS` (400) rows, and mounting that
        for every edit in a non-virtualized transcript is the DOM cost this
        whole collapse design exists to avoid.
      */}
      {expandable && (
        <div style={{ display: 'flex', flexDirection: 'column', marginLeft: 22 }}>
          <Disclosure
            id={item.id}
            open={isOpen}
            onToggle={() => onSetOpen(item.id, !isOpen)}
            summary={summary}
            buttonStyle={{ marginTop: 4 }}
          >
            {diff && (
              <div style={{ marginTop: 6 }}>
                <DiffView diff={diff} />
              </div>
            )}
            {body !== undefined && (
              <pre
                className="mono-code"
                style={{
                  ...BODY_STYLE,
                  marginLeft: 0,
                  background: t.windowBg,
                  color: item.status === 'error' ? t.danger : t.text2,
                  border: `0.5px solid ${t.border}`,
                }}
              >
                {body}
              </pre>
            )}
            {/* The engine clamped the body; say so where the shortfall is
                actually visible, not only on the collapsed label. */}
            {truncationNotice && (
              <div style={{ marginTop: 4, fontSize: 11.5, fontStyle: 'italic', color: t.text4 }}>
                {truncationNotice}
              </div>
            )}
          </Disclosure>
        </div>
      )}
    </div>
  );
});
