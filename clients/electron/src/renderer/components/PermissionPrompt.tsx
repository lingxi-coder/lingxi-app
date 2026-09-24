/**
 * PermissionPrompt — the renderer's allow/deny surface for engine-parked
 * permission requests (7.3).
 *
 * The main process forwards every inbound {@link PermissionRequest} over the
 * bridge; `useBridge` queues them and exposes the head as `pendingPermission`
 * plus `approve` / `deny`. This component renders a compact session panel for
 * that head request with allow-once / deny actions, plus allow-always only when
 * the engine permits a persistent rule.
 *
 * It is intentionally framework-light (CSS variables supplied by the current
 * theme, mirroring the rest of the renderer) and renders nothing when no
 * request is pending.
 */

import type { CSSProperties } from 'react';
import type { PermissionRequest } from '@lingxi/bridge-client';
// Subpath, not the barrel — see the note in `bridge/conversation.ts`.
import { redactSensitiveText } from '@lingxi/bridge-client/toolview';
import { useT } from '../theme/ThemeContext';
import { DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';

/**
 * Execution-bearing keys, shown FIRST. These are what the user is actually
 * authorizing; everything else in the payload is context.
 */
const DANGEROUS_KEYS = ['command', 'code', 'script'] as const;

/** Render one payload value as text: strings raw, anything else as JSON. */
function renderValue(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value === undefined) return 'undefined';
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/**
 * Everything the confirm payload contains, redacted — NOT one probed key.
 *
 * The shared `toolInputDetail` returns the FIRST match of a fixed probe order
 * (`file_path, path, notebook_path, pattern, query, url, command, …`), which is
 * right for a transcript header and wrong here: a payload carrying both a
 * path-ish key and a command — an MCP or third-party tool with
 * `{"path": "/tmp", "command": "curl … | sh"}` — renders as `/tmp`, and the
 * command being authorized is invisible in the dialog that authorizes it. A
 * single-key probe cannot honor "show MORE"; only showing the whole payload can.
 *
 * A single-entry payload still renders as the bare value, so the overwhelmingly
 * common `{"command": …}` keeps its clean, unlabeled shell-script look. Nothing
 * is clamped or collapsed to one line: the detail box is a `pre-wrap` scroller
 * built to show all of it, and a 40-line script cut at 160 characters hides
 * exactly the tail worth reading.
 */
export function permissionDetail(toolInputJson: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(toolInputJson) as unknown;
  } catch {
    return redactSensitiveText(toolInputJson);
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return redactSensitiveText(toolInputJson);
  }
  const input = parsed as Record<string, unknown>;
  const keys = Object.keys(input);
  if (keys.length === 0) return redactSensitiveText(toolInputJson);
  if (keys.length === 1) return redactSensitiveText(renderValue(input[keys[0] as string]));
  const ordered = [
    ...DANGEROUS_KEYS.filter((key) => keys.includes(key)),
    ...keys.filter((key) => !(DANGEROUS_KEYS as readonly string[]).includes(key)),
  ];
  return redactSensitiveText(
    ordered.map((key) => `${key}: ${renderValue(input[key])}`).join('\n'),
  );
}

interface PermissionDescription {
  title: string;
  summary: string;
  detailLabel: string;
  detailCaption: string;
  detail: string;
}

/** Human title + detail for each {@link PermissionRequest} kind. */
function describe(request: PermissionRequest): PermissionDescription {
  const kind = request.kind;
  switch (kind.type) {
    case 'tool_use_confirm':
      return {
        title: `Allow ${kind.tool_name}?`,
        summary: `LingXi wants to use ${kind.tool_name}. Review the requested input before continuing.`,
        detailLabel: 'Requested action',
        detailCaption: kind.tool_name,
        // The FULL redacted payload — never a preview, and never a single
        // probed key. This dialog is a security decision, so showing MORE is
        // the safe failure mode; see `permissionDetail`.
        detail: permissionDetail(kind.tool_input_json),
      };
    case 'exit_plan_mode':
      return {
        title: 'Exit plan mode and proceed?',
        summary: 'LingXi wants to leave plan mode and begin working from this plan.',
        detailLabel: 'Plan to execute',
        detailCaption: 'May run commands or modify files',
        detail: kind.plan,
      };
    case 'bypass_permissions_mode':
      return {
        title: 'Enable bypass-permissions mode?',
        summary: 'LingXi will be able to act without asking for further confirmation.',
        detailLabel: 'Permission scope',
        detailCaption: 'Commands, files, and connected services',
        detail: 'The agent will act without asking for further confirmation.',
      };
    default:
      // Exhaustiveness guard — a new kind shows a generic prompt rather than nothing.
      return {
        title: 'Permission requested',
        summary: 'LingXi needs your approval before it can continue.',
        detailLabel: 'Requested action',
        detailCaption: 'Current session',
        detail: '',
      };
  }
}

function requestIcon(request: PermissionRequest): string {
  if (request.kind.type === 'exit_plan_mode') return 'tasks';
  if (request.kind.type === 'bypass_permissions_mode') return 'shieldAlert';
  if (request.kind.type !== 'tool_use_confirm') return 'box';
  const name = request.kind.tool_name.toLowerCase();
  if (/(bash|shell|terminal|command|exec)/.test(name)) return 'terminal';
  if (/(write|edit|patch|update)/.test(name)) return 'compose';
  if (/(read|file|folder)/.test(name)) return 'file';
  return 'box';
}

export interface PermissionPromptProps {
  /** The head request to render, or `null` to render nothing. */
  request: PermissionRequest | null;
  /** Approve the request (defaults to allow-once in the hook). */
  onApprove(requestId: number, response: { type: 'allow_once' | 'allow_always' | 'allow_auto' }): void;
  /** Deny the request. */
  onDeny(requestId: number): void;
}

export function PermissionPrompt({ request, onApprove, onDeny }: PermissionPromptProps) {
  const t = useT();
  if (!request) return null;

  const description = describe(request);
  const { title, summary, detailLabel, detailCaption, detail } = description;
  const worker = request.worker;
  const showPersistentRule = !request.suppress_always_allow_rule && !request.auto_mode_prompt;
  const showAutoMode = Boolean(request.auto_mode_prompt && !request.suppress_always_allow_rule);
  const elevatedRisk = request.kind.type === 'bypass_permissions_mode';
  const riskCopy = elevatedRisk
    ? 'This can expose or modify sensitive data. Continue only if you trust the current session.'
    : showAutoMode
      ? 'Auto mode can approve future actions without asking. Review the requested scope carefully.'
      : showPersistentRule
        ? '“Allow matching actions” saves a rule for this workspace. Use it only when you trust future matching requests.'
        : 'This decision applies only to the current request.';

  const dialogVariables = {
    '--dialog-window': t.windowBg,
    '--dialog-surface': t.surface,
    '--dialog-surface-hover': t.surfaceHover,
    '--dialog-border': t.border,
    '--dialog-border-strong': t.borderStrong,
    '--dialog-text': t.text,
    '--dialog-text-2': t.text2,
    '--dialog-text-3': t.text3,
    '--dialog-accent': elevatedRisk ? t.danger : t.accent,
    '--dialog-accent-border': elevatedRisk ? t.danger : t.accentBorder,
    '--dialog-danger': t.danger,
  } as CSSProperties;

  return (
    <section
      className="inline-interaction-card permission-prompt-inline"
      style={dialogVariables}
      role="region"
      aria-labelledby="lingxi-permission-heading"
      aria-describedby="lingxi-permission-summary lingxi-permission-risk"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          onDeny(request.request_id);
        }
      }}
    >
      <header className="inline-interaction-header">
        <span className="inline-interaction-heading-icon" aria-hidden="true">
          <Icon name={elevatedRisk ? 'shieldAlert' : 'hand'} size={19} stroke={1.7} />
        </span>
        <h2 id="lingxi-permission-heading">Permission request</h2>
        <button
          type="button"
          className="inline-interaction-close"
          aria-label="Deny permission request"
          title="Deny"
          onClick={() => onDeny(request.request_id)}
        >
          <Icon name="x" size={17} stroke={1.8} />
        </button>
      </header>

      <div className="inline-interaction-body">
        <h3 id="lingxi-permission-title" className="permission-prompt-title">{title}</h3>
        <p id="lingxi-permission-summary" className="permission-prompt-summary">{summary}</p>

        <div className="permission-prompt-request-card">
          <div className="permission-prompt-request-heading">
            <span className="permission-prompt-request-icon" aria-hidden="true">
              <Icon name={requestIcon(request)} size={20} stroke={1.7} />
            </span>
            <span className="permission-prompt-request-copy">
              <strong>{detailLabel}</strong>
              <small>{detailCaption}</small>
            </span>
            {worker && (
              <span className="permission-prompt-worker" style={{ color: worker.color || t.text3 }}>
                <span style={{ background: worker.color || t.text3 }} />
                {worker.name}
                {worker.team ? ` · ${worker.team}` : ''}
              </span>
            )}
          </div>

          {detail && (
            <div id="lingxi-permission-detail" className="permission-prompt-detail mono">
              {detail}
            </div>
          )}
        </div>

        <p id="lingxi-permission-risk" className="permission-prompt-risk">
          <Icon name="info" size={16} stroke={1.7} />
          <span>{riskCopy}</span>
        </p>
      </div>

      <footer className="inline-interaction-footer">
        <DesktopDialogActions>
          <DesktopDialogButton variant="cancel" onClick={() => onDeny(request.request_id)}>
            Deny
          </DesktopDialogButton>
          {showPersistentRule && (
            <DesktopDialogButton variant="secondary" onClick={() => onApprove(request.request_id, { type: 'allow_always' })}>
              Allow matching actions
            </DesktopDialogButton>
          )}
          {showAutoMode && (
            <DesktopDialogButton variant="secondary" onClick={() => onApprove(request.request_id, { type: 'allow_auto' })}>
              {request.auto_mode_prompt === 'workflow_bash'
                ? 'Yes, and switch to auto mode'
                : 'Yes, and use auto mode'}
            </DesktopDialogButton>
          )}
          <DesktopDialogButton variant="primary" onClick={() => onApprove(request.request_id, { type: 'allow_once' })}>
            Allow once
          </DesktopDialogButton>
        </DesktopDialogActions>
      </footer>
    </section>
  );
}
