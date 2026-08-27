/**
 * One tool call in the transcript.
 *
 * The row renders the engine's PRE-DERIVED view and nothing else. It never
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

import { standaloneJsonForDisplay } from '../markdown';
import type { ToolRunItem } from '../model/runItem';
import { toolHasBody, toolTruncationNotice } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { ltrAnchored } from './bidi';
import { CodeBlock } from './CodeBlock';
import { DiffView } from './DiffView';
import { Disclosure } from './Disclosure';
import { Icon } from './Icon';

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

/** Existing icon vocabulary mapped onto the engine's open-ended tool verbs. */
export function toolIconName(verb: string): string {
  const value = verb.toLowerCase();
  if (['search', 'find', 'grep', 'web'].some((part) => value.includes(part))) return 'search';
  if (['bash', 'shell', 'terminal', 'command', 'exec'].some((part) => value.includes(part))) return 'terminal';
  // `TodoWrite` is a task tool even though it also contains "write".
  if (['todo', 'task', 'plan'].some((part) => value.includes(part))) return 'tasks';
  if (['edit', 'write', 'update', 'create', 'patch'].some((part) => value.includes(part))) return 'code';
  if (['git', 'commit', 'branch'].some((part) => value.includes(part))) return 'git';
  if (['read', 'file', 'glob', 'directory', 'list'].some((part) => value.includes(part))) return 'file';
  return 'box';
}

function ToolGlyph({ item }: { item: ToolRunItem }) {
  return (
    <span
      className="tool-row-icon"
      aria-hidden="true"
      style={{
        width: 20, height: 20, flexShrink: 0,
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
        color: 'var(--tool-icon-color)',
      }}
    >
      <Icon name={toolIconName(item.view.verb)} size={18} color="currentColor" stroke={1.8} />
    </span>
  );
}

export const ToolCall = memo(function ToolCall({ item, open, onSetOpen }: ToolCallProps) {
  const t = useT();
  const { view, result } = item;
  const expandable = toolHasBody(item);
  const isOpen = open ?? false;

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
  const jsonBody = body === undefined ? undefined : standaloneJsonForDisplay(body);
  const diff = result?.diff;

  // Keep truncation visible in the compact response line and repeat the
  // detailed notice under the body once it is opened.
  const truncationNotice = toolTruncationNotice(item);

  const response = [headline, truncationNotice ? '(truncated)' : undefined]
    .filter(Boolean)
    .join(' ');

  const summary = (
    <span className="tool-call-summary">
      <ToolGlyph item={item} />
      <span
        className={item.status === 'running' ? 'tool-row-title running-sweep' : 'tool-row-title'}
        style={{
          flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis',
          whiteSpace: 'nowrap', fontSize: 14.5, lineHeight: 1.45, fontWeight: 500,
        }}
      >
        {title}
        {view.sub_line && (
          <span className="tool-row-inline-detail mono-code">
            {' · '}{view.sub_line.prefix}{view.sub_line.text}
          </span>
        )}
        {response && <span>{' · '}{response}</span>}
      </span>
    </span>
  );

  return (
    <div
      className="transcript-tool-row"
      data-status={item.status}
      style={{
        maxWidth: 760,
        display: 'flex', flexDirection: 'column', gap: 4,
        padding: '2px 0', position: 'relative',
        '--tool-label-color': item.status === 'error' ? t.danger : t.text2,
        '--tool-hover-color': item.status === 'error' ? t.danger : t.text,
        '--tool-focus-color': item.status === 'error' ? t.danger : t.accent,
        '--tool-icon-color': item.status === 'error' ? t.danger : item.status === 'running' ? t.accent : t.text3,
        '--sweep-base': item.status === 'error' ? t.danger : t.text2,
        '--sweep-highlight': item.status === 'error' ? t.danger : t.accent,
      } as CSSProperties}
    >
      {/* The call and its response share one Thought-like disclosure row. */}
      {expandable ? (
        <Disclosure
          id={item.id}
          open={isOpen}
          onToggle={() => onSetOpen(item.id, !isOpen)}
          summary={summary}
          buttonClassName="tool-disclosure-trigger"
          buttonStyle={{ width: '100%', maxWidth: '100%', minHeight: 40, margin: 0, padding: '2px 0', borderRadius: 7 }}
          bodyStyle={{ marginLeft: 29 }}
        >
            {diff && (
              <div style={{ marginTop: 6 }}>
                <DiffView diff={diff} />
              </div>
            )}
            {body !== undefined && (
              jsonBody !== undefined ? (
                <div style={{ marginTop: 6 }}>
                  <CodeBlock code={jsonBody} language="json" variant="tool" />
                </div>
              ) : (
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
              )
            )}
            {/* The engine clamped the body; say so where the shortfall is
                actually visible, not only on the collapsed label. */}
            {truncationNotice && (
              <div style={{ marginTop: 4, fontSize: 11.5, fontStyle: 'italic', color: t.text4 }}>
                {truncationNotice}
              </div>
            )}
        </Disclosure>
      ) : (
        <div className="tool-call-line" style={{ minHeight: 40, padding: '2px 0' }}>
          {summary}
        </div>
      )}
    </div>
  );
});
