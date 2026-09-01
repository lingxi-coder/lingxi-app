/**
 * PermissionPrompt — the renderer's allow/deny surface for engine-parked
 * permission requests (7.3).
 *
 * The main process forwards every inbound {@link PermissionRequest} over the
 * bridge; `useBridge` queues them and exposes the head as `pendingPermission`
 * plus `approve` / `deny`. This component renders a compact session panel for
 * that head request with three actions — allow-once / allow-always / deny — each calling
 * back through the bridge with the matching {@link PermissionResponseDto}.
 *
 * It is intentionally framework-light (inline styles via the theme tokens,
 * mirroring the rest of the renderer) and renders nothing when no request is
 * pending.
 */

import { useEffect, useRef } from 'react';
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

/** Human title + detail for each {@link PermissionRequest} kind. */
function describe(request: PermissionRequest): { title: string; detail: string } {
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
      return { title: 'Permission requested', detail: '' };
  }
}

export interface PermissionPromptProps {
  /** The head request to render, or `null` to render nothing. */
  request: PermissionRequest | null;
  /** Approve the request (defaults to allow-once in the hook). */
  onApprove(requestId: number, response: { type: 'allow_once' | 'allow_always' }): void;
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

  return (
    <div
      role="dialog"
      aria-label={title}
      aria-describedby={detail ? 'lingxi-permission-detail lingxi-permission-scope' : 'lingxi-permission-scope'}
      onFocusCapture={() => { promptHasFocus.current = true; }}
      onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        onDeny(request.request_id);
      }}
      style={{
        position: 'absolute', inset: 0, zIndex: 60,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: 'rgba(0,0,0,0.32)',
      }}
    >
      <div
        style={{
          width: 420, maxWidth: '90%', borderRadius: 14, overflow: 'hidden',
          background: t.windowBg, border: `0.5px solid ${t.border}`,
          boxShadow: '0 18px 48px rgba(0,0,0,0.34)',
        }}
      >
        <div style={{ padding: '18px 20px 14px' }}>
          {worker && (
            <div
              style={{
                display: 'inline-flex', alignItems: 'center', gap: 6, marginBottom: 8,
                fontSize: 11, fontWeight: 600, color: worker.color || t.text3,
              }}
            >
              <span
                style={{
                  width: 7, height: 7, borderRadius: '50%',
                  background: worker.color || t.text3,
                }}
              />
              {worker.name}
              {worker.team ? ` · ${worker.team}` : ''}
            </div>
          )}
          <div style={{ fontSize: 15, fontWeight: 600, color: t.text, marginBottom: detail ? 8 : 0 }}>
            {title}
          </div>
          {detail && (
            <div
              id="lingxi-permission-detail"
              className="mono"
              style={{
                fontSize: 12, color: t.text2, lineHeight: 1.5, maxHeight: 180, overflow: 'auto',
                whiteSpace: 'pre-wrap', wordBreak: 'break-word',
                background: t.surface, border: `0.5px solid ${t.border}`,
                borderRadius: 8, padding: '8px 10px',
              }}
            >
              {detail}
            </div>
          )}
        </div>
        <div
          style={{
            display: 'flex', gap: 8, padding: '12px 16px',
            borderTop: `0.5px solid ${t.border}`, background: t.surface,
          }}
        >
          <button
            type="button"
            onClick={() => onDeny(request.request_id)}
            style={{
              flex: 1, padding: '8px 10px', borderRadius: 8, cursor: 'pointer',
              fontSize: 12.5, fontWeight: 600, fontFamily: 'inherit',
              color: t.danger, background: 'transparent',
              border: `0.5px solid ${t.border}`,
            }}
          >
            Deny
          </button>
          <button
            type="button"
            onClick={() => onApprove(request.request_id, { type: 'allow_always' })}
            style={{
              flex: 1, padding: '8px 10px', borderRadius: 8, cursor: 'pointer',
              fontSize: 12.5, fontWeight: 600, fontFamily: 'inherit',
              color: t.text2, background: 'transparent',
              border: `0.5px solid ${t.border}`,
            }}
          >
            Allow matching actions
          </button>
          <button
            ref={primaryRef}
            type="button"
            onClick={() => onApprove(request.request_id, { type: 'allow_once' })}
            style={{
              flex: 1, padding: '8px 10px', borderRadius: 8, cursor: 'pointer',
              fontSize: 12.5, fontWeight: 600, fontFamily: 'inherit',
              color: '#fff', background: t.accent,
              border: `0.5px solid ${t.accentBorder}`,
            }}
          >
            Allow once
          </button>
        </div>
        <div id="lingxi-permission-scope" style={{ padding: '0 16px 12px', background: t.surface, color: t.text3, fontSize: 10.5, lineHeight: 1.45 }}>
          “Allow matching actions” saves a narrowed rule for this workspace when the engine supports it.
        </div>
      </div>
    </div>
  );
}
