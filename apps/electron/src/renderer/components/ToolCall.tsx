/**
 * One tool call in the transcript.
 *
 * Ordinary rows render the engine's PRE-DERIVED view. Completed questionnaires
 * additionally retain their verbatim structured question/answer content;
 * `view` (a `ToolHeaderDto`) and
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
import { AskUserQuestionSummary, questionAnswers } from './AskUserQuestionSummary';
import type { ToolIconDto } from '@lingxi/bridge-client';

import { standaloneJsonForDisplay } from '../markdown';
import type { ToolRunItem } from '../model/runItem';
import { toolDisplayHeader, toolHasBody, toolTruncationNotice } from '../model/runItem';
import { nativeToolResultProps, nativeToolUseProps } from './nativeUiSiteProps';
import { useT } from '../theme/ThemeContext';
import { ltrAnchored } from './bidi';
import { CodeBlock } from './CodeBlock';
import { DiffView } from './DiffView';
import { Disclosure } from './Disclosure';
import { Icon } from './Icon';
import { ToolActivityIcon } from './ToolActivityIcon';
import { ModUiParentSite } from './modUiAbovePrompt';

const TITLE_STYLE: CSSProperties = Object.freeze({
  fontSize: 13,
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
  /** Real desktop session id for Native ToolUse/ToolResult render sites. */
  modUiSessionId?: string;
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

const SEMANTIC_TOOL_ICONS = {
  read: 'file', search: 'search', list: 'list', edit: 'compose', terminal: 'terminal',
  globe: 'globe', workflow: 'workflow', list_checks: 'tasks', sparkles: 'spark',
  plug: 'plug', output: 'output', stop: 'stop', wrench: 'box',
} satisfies Record<ToolIconDto, string>;

/** Existing icon vocabulary mapped onto the engine's open-ended tool verbs. */
export function toolIconName(verb: string, tool?: string, icon?: ToolIconDto): string {
  // Generic MCP verbs do not distinguish visual/media and messaging tools.
  const identity = (verb.toLowerCase() === 'generic' ? tool?.split('__').at(-1) ?? verb : verb).toLowerCase();
  if (/(?:^|_)(?:send_message|send_input|message|chat)(?:_|$)/.test(identity)) return 'chat';
  if (/(?:^|_)(?:view_image|image|images|screenshot)(?:_|$)/.test(identity)) return 'image';
  if (icon) return SEMANTIC_TOOL_ICONS[icon] ?? 'box';
  const value = verb.toLowerCase();
  if (value === 'fetch') return 'globe';
  if (value === 'skill') return 'spark';
  if (value === 'output') return 'output';
  if (value === 'kill') return 'stop';
  if (['search', 'find', 'grep', 'web'].some((part) => value.includes(part))) return 'search';
  if (['bash', 'shell', 'terminal', 'command', 'exec'].some((part) => value.includes(part))) return 'terminal';
  // `TodoWrite` is a task tool even though it also contains "write".
  if (['todo', 'task', 'plan'].some((part) => value.includes(part))) return 'tasks';
  if (['edit', 'write', 'update', 'create', 'patch'].some((part) => value.includes(part))) return 'compose';
  if (['git', 'commit', 'branch'].some((part) => value.includes(part))) return 'git';
  if (['read', 'file', 'glob', 'directory', 'list'].some((part) => value.includes(part))) return 'file';
  return 'box';
}

function ToolGlyph({ item, permissionRequest }: { item: ToolRunItem; permissionRequest: boolean }) {
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
      {permissionRequest
        ? <Icon name="hand" size={18} stroke={1.7} />
        : <ToolActivityIcon name={toolIconName(item.view.verb, item.tool, item.view.icon)} />}
    </span>
  );
}

export const ToolCall = memo(function ToolCall({ item, modUiSessionId, open, onSetOpen }: ToolCallProps) {
  const t = useT();
  const { result } = item;
  const view = toolDisplayHeader(item);
  const permissionRequest = /permission/i.test(`${item.tool} ${view.label} ${view.title}`);
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
  const permissionStatusText = headline ?? '';
  const permissionStatusInBody = permissionRequest && expandable && Boolean(permissionStatusText);
  const summaryResponse = permissionStatusInBody
    ? (truncationNotice ? '(truncated)' : '')
    : response;
  const permissionStatusTone = /denied|rejected|failed|cancel/i.test(permissionStatusText)
    ? 'danger'
    : /accepted|approved|granted|allowed/i.test(permissionStatusText)
      ? 'success'
      : 'neutral';
  const permissionStatusIcon = permissionStatusTone === 'success'
    ? 'check'
    : permissionStatusTone === 'danger'
      ? 'x'
      : 'info';

  const summary = (
    <span className="tool-call-summary">
      <ToolGlyph item={item} permissionRequest={permissionRequest} />
      <span
        className={item.status === 'running' ? 'tool-row-title running-sweep' : 'tool-row-title'}
        style={{
          flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
          fontSize: permissionRequest ? 14 : 13,
          lineHeight: permissionRequest ? 1.35 : 1.4,
          fontWeight: permissionRequest ? 620 : 500,
        }}
      >
        {title}
        {view.sub_line && (
          <span className="tool-row-inline-detail mono-code">
            {' · '}{view.sub_line.prefix}{view.sub_line.text}
          </span>
        )}
        {summaryResponse && <span>{' · '}{summaryResponse}</span>}
      </span>
    </span>
  );

  const resultContent = <>
    {diff && (
      <div style={{ marginTop: 6 }}>
        <DiffView diff={diff} />
      </div>
    )}
    {body !== undefined && (
      permissionRequest && jsonBody === undefined ? (
        <div className="permission-tool-message">{body}</div>
      ) : jsonBody !== undefined ? (
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
    {permissionStatusInBody && (
      <div className="permission-tool-status" data-tone={permissionStatusTone} role="status">
        <Icon name={permissionStatusIcon} size={14} stroke={2} />
        <span>{permissionStatusText}</span>
      </div>
    )}
    {truncationNotice && (
      <div style={{ marginTop: 4, fontSize: 11.5, fontStyle: 'italic', color: t.text4 }}>
        {truncationNotice}
      </div>
    )}
  </>;
  const answeredQuestions = questionAnswers(item);
  const rowContent = answeredQuestions ? (
    <AskUserQuestionSummary id={item.id} rows={answeredQuestions} open={open ?? true}
      onToggle={() => onSetOpen(item.id, !(open ?? true))}
      keepMounted={Boolean(modUiSessionId)}
      renderBody={(body) => <ModUiParentSite sessionId={modUiSessionId ?? ''}
        surface="desktop" component="ToolResult" instanceId={`tool-result:${item.id}`}
        props={nativeToolResultProps(item)} engineFallback={body}>{body}</ModUiParentSite>} />
  ) : (
    <div
      className="transcript-tool-row"
      data-status={item.status}
      data-kind={permissionRequest ? 'permission' : undefined}
      style={{
        maxWidth: 760,
        display: 'flex', flexDirection: 'column', gap: 4,
        padding: '2px 0', position: 'relative',
        '--tool-label-color': item.status === 'error' ? t.danger : permissionRequest ? t.text2 : t.text3,
        '--tool-hover-background': t.surfaceHover,
        '--tool-hover-color': item.status === 'error' ? t.danger : t.text,
        '--tool-focus-color': item.status === 'error' ? t.danger : t.text,
        '--tool-icon-color': item.status === 'error' ? t.danger : permissionRequest ? t.accent : t.text3,
        '--permission-text': t.text2,
        '--permission-muted': t.text3,
        '--sweep-base': item.status === 'error' ? t.danger : t.text3,
        '--sweep-highlight': item.status === 'error' ? t.danger : t.text,
        '--permission-accent': t.accent,
        '--permission-surface': t.surface,
        '--permission-border': t.border,
        '--permission-success': t.ok,
        '--permission-danger': t.danger,
      } as CSSProperties}
    >
      {/* The call and its response share one Thought-like disclosure row. */}
      {expandable ? (
        <Disclosure
          id={item.id}
          open={isOpen}
          keepMounted={item.status !== 'running' && Boolean(modUiSessionId)}
          onToggle={() => onSetOpen(item.id, !isOpen)}
          summary={summary}
          buttonClassName="tool-disclosure-trigger"
          buttonStyle={{ width: '100%', maxWidth: '100%', minHeight: 40, margin: 0, padding: '2px 0', borderRadius: 7 }}
          bodyStyle={permissionRequest
            ? { marginLeft: 0, padding: '0 14px 14px 56px' }
            : { marginLeft: 29 }}
        >
          <ModUiParentSite
            sessionId={item.status === 'running' ? '' : modUiSessionId ?? ''}
            surface="desktop"
            component="ToolResult"
            instanceId={`tool-result:${item.id}`}
            props={nativeToolResultProps(item)}
            engineFallback={isOpen ? resultContent : null}
          >
            {isOpen ? resultContent : null}
          </ModUiParentSite>
        </Disclosure>
      ) : (
        <>
          <div className="tool-call-line" style={{ minHeight: 40, padding: '2px 0' }}>
            {summary}
          </div>
          {item.status !== 'running' && modUiSessionId && (
            <ModUiParentSite
              sessionId={modUiSessionId}
              surface="desktop"
              component="ToolResult"
              instanceId={`tool-result:${item.id}`}
              props={nativeToolResultProps(item)}
              engineFallback={null}
            />
          )}
        </>
      )}
    </div>
  );
  return modUiSessionId ? <ModUiParentSite
    sessionId={modUiSessionId}
    surface="desktop"
    component="ToolUse"
    instanceId={`tool-use:${item.id}`}
    props={nativeToolUseProps(item)}
    engineFallback={rowContent}
  >{rowContent}</ModUiParentSite> : rowContent;
});
