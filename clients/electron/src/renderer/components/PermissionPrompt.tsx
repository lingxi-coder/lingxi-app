/**
 * PermissionPrompt — the renderer's allow/deny surface for engine-parked
 * permission requests (7.3).
 *
 * The main process forwards every inbound {@link PermissionRequest} over the
 * bridge; `useBridge` queues them and exposes the head as `pendingPermission`
 * plus `approve` / `deny`. This component renders a minimal modal for that head
 * request with three actions — allow-once / allow-always / deny — each calling
 * back through the bridge with the matching {@link PermissionResponseDto}.
 *
 * It is intentionally framework-light (inline styles via the theme tokens,
 * mirroring the rest of the renderer) and renders nothing when no request is
 * pending.
 */

import type { PermissionRequest } from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';

/** Human title + detail for each {@link PermissionRequest} kind. */
function describe(request: PermissionRequest): { title: string; detail: string } {
  const kind = request.kind;
  switch (kind.type) {
    case 'tool_use_confirm':
      return {
        title: `Allow ${kind.tool_name}?`,
        detail: previewToolInput(kind.tool_input_json),
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

/** Best-effort, never-throwing one-line preview of a tool's JSON input. */
function previewToolInput(inputJson: string): string {
  if (!inputJson) return '';
  try {
    const parsed: unknown = JSON.parse(inputJson);
    if (parsed && typeof parsed === 'object') {
      const obj = parsed as Record<string, unknown>;
      const candidate =
        obj['command'] ?? obj['file_path'] ?? obj['path'] ?? obj['pattern'] ?? obj['query'];
      if (typeof candidate === 'string' && candidate.length > 0) {
        return candidate;
      }
    }
    return inputJson;
  } catch {
    return inputJson;
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
  if (!request) return null;

  const { title, detail } = describe(request);
  const worker = request.worker;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
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
            Allow always
          </button>
          <button
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
      </div>
    </div>
  );
}
