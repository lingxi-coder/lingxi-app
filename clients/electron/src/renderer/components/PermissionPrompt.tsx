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

import { useEffect, useRef, type CSSProperties } from 'react';
import type { PermissionRequest } from '@lingxi/bridge-client';
// Subpath, not the barrel — see the note in `bridge/conversation.ts`.
import { redactSensitiveText } from '@lingxi/bridge-client/toolview';
import { useT } from '../theme/ThemeContext';

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
  detail: string;
}

/** Human title + detail for each {@link PermissionRequest} kind. */
function describe(request: PermissionRequest): PermissionDescription {
  const kind = request.kind;
  switch (kind.type) {
    case 'tool_use_confirm':
      return {
        title: `Allow ${kind.tool_name}?`,
        // The FULL redacted payload — never a preview, and never a single
        // probed key. This dialog is a security decision, so showing MORE is
        // the safe failure mode; see `permissionDetail`.
        detail: permissionDetail(kind.tool_input_json),
      };
    case 'exit_plan_mode':
      return {
        title: 'Exit plan mode and proceed?',
        detail: kind.plan,
      };
    case 'bypass_permissions_mode':
      return {
        title: 'Enable bypass-permissions mode?',
        detail: 'The agent will act without asking for further confirmation.',
      };
    default:
      // Exhaustiveness guard — a new kind shows a generic prompt rather than nothing.
      return {
        title: 'Permission requested',
        detail: '',
      };
  }
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
  const primaryRef = useRef<HTMLButtonElement>(null);
  const promptHasFocus = useRef(false);
  useEffect(() => {
    if (!request) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    primaryRef.current?.focus();
    return () => {
      // Do not steal focus back after the user has moved into the global
      // Sidebar while this session prompt is still visible.
      if (promptHasFocus.current) previouslyFocused?.focus();
    };
  }, [request]);
  if (!request) return null;

  const { title, detail } = describe(request);
  const worker = request.worker;
  const showPersistentRule = !request.suppress_always_allow_rule && !request.auto_mode_prompt;
  const showAutoMode = Boolean(request.auto_mode_prompt && !request.suppress_always_allow_rule);
  const themeVariables = {
    '--permission-overlay': 'rgba(0, 0, 0, .32)',
    '--permission-window': t.windowBg,
    '--permission-surface': t.surface,
    '--permission-surface-hover': t.surfaceHover,
    '--permission-border': t.border,
    '--permission-border-strong': t.borderStrong,
    '--permission-text': t.text,
    '--permission-text-2': t.text2,
    '--permission-text-3': t.text3,
    '--permission-accent': t.accent,
    '--permission-accent-border': t.accentBorder,
    '--permission-danger': t.danger,
  } as CSSProperties;

  return (
    <div
      className="permission-prompt-overlay"
      style={themeVariables}
    >
      <div
        role="dialog"
        aria-labelledby="lingxi-permission-title"
        aria-describedby={detail ? 'lingxi-permission-detail' : undefined}
        className="permission-prompt-panel"
        onFocusCapture={() => { promptHasFocus.current = true; }}
        onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
        onKeyDown={(event) => {
          if (event.key !== 'Escape') return;
          event.preventDefault();
          onDeny(request.request_id);
        }}
      >
        <header className="permission-prompt-titlebar">
          <h2 id="lingxi-permission-title">{title}</h2>
        </header>

        <section className="permission-prompt-content">
          {worker && (
            <div className="permission-prompt-worker" style={{ color: worker.color || t.text3 }}>
              <span style={{ background: worker.color || t.text3 }} />
              {worker.name}
              {worker.team ? ` · ${worker.team}` : ''}
            </div>
          )}
          {detail && (
            <div
              id="lingxi-permission-detail"
              className="permission-prompt-detail mono"
            >
              {detail}
            </div>
          )}
        </section>

        <footer className="permission-prompt-footer">
          <div className="permission-prompt-actions">
            <button
              type="button"
              className="permission-prompt-action permission-prompt-action--deny"
              onClick={() => onDeny(request.request_id)}
            >
              Deny
            </button>
            {showPersistentRule && (
              <button
                type="button"
                className="permission-prompt-action permission-prompt-action--secondary"
                onClick={() => onApprove(request.request_id, { type: 'allow_always' })}
              >
                Allow matching actions
              </button>
            )}
            {showAutoMode && (
              <button
                type="button"
                className="permission-prompt-action permission-prompt-action--secondary"
                onClick={() => onApprove(request.request_id, { type: 'allow_auto' })}
              >
                {request.auto_mode_prompt === 'workflow_bash'
                  ? 'Yes, and switch to auto mode'
                  : 'Yes, and use auto mode'}
              </button>
            )}
            <button
              ref={primaryRef}
              type="button"
              className="permission-prompt-action permission-prompt-action--primary"
              onClick={() => onApprove(request.request_id, { type: 'allow_once' })}
            >
              Allow once
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
