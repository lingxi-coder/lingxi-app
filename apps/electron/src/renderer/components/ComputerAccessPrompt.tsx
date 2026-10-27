import { useI18n } from '../i18n/useI18n';
/**
 * ComputerAccessPrompt — the renderer's approval surface for the `computer`
 * tool's `request_access` action, wired through the same
 * `ComputerAccessExchange` round-trip the TUI's `ComputerAccessView` resolves
 * (`tui-core/src/computer_access_bridge.rs`).
 *
 * Two-panel contract, preserving the TUI permission choices:
 *  - When `request.tcc_state` is present, a required macOS permission
 *    (Accessibility / Screen Recording) is missing — render the TCC panel:
 *    granted/not-granted per permission, an "Open System Settings" action per
 *    missing permission, and a "Try again" action that denies (there is no
 *    live re-check from this dialog; the user must re-trigger
 *    `request_access` after fixing permissions).
 *  - Otherwise render the app-allowlist panel: one pre-checked checkbox per
 *    requested app, a checkbox per requested capability flag
 *    (clipboardRead/clipboardWrite/systemKeyCombos), and a submit button
 *    ("Allow for this session").
 *
 * Escape while this panel is focused denies (matching the TUI and
 * {@link PermissionPrompt}). Local
 * checkbox state resets to a fresh pre-checked state whenever a NEW
 * `request.request_id` arrives, so it never carries over stale selections
 * from a previous request.
 */

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { ComputerAccessRequestDto, ComputerAccessResponseDto } from '@lingxi/bridge-client';
import type { SystemSettingsPane } from '../bridge/lingxi';
import { useT } from '../theme/ThemeContext';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';

export interface ComputerAccessPromptProps {
  /** The head request to render, or `null` to render nothing. */
  request: ComputerAccessRequestDto | null;
  /** Submit the app-allowlist panel's current selection. */
  onSubmit(requestId: number, response: ComputerAccessResponseDto): void;
  /** Deny the request (Esc, "Try again" on the TCC panel, or the Deny button). */
  onDeny(requestId: number): void;
  /** Open a macOS System Settings pane in the user's default handler. */
  onOpenSystemSettings(pane: SystemSettingsPane): void;
}

function initialCheckedApps(request: ComputerAccessRequestDto | null): Set<string> {
  return new Set(request?.apps.map((app) => app.label) ?? []);
}

export function ComputerAccessPrompt({ request, onSubmit, onDeny, onOpenSystemSettings }: ComputerAccessPromptProps) {
  const { tr } = useI18n();
  const t = useT();
  const primaryRef = useRef<HTMLButtonElement>(null);
  const promptHasFocus = useRef(false);

  const [checkedApps, setCheckedApps] = useState<Set<string>>(() => initialCheckedApps(request));
  const [clipboardRead, setClipboardRead] = useState(Boolean(request?.clipboard_read));
  const [clipboardWrite, setClipboardWrite] = useState(Boolean(request?.clipboard_write));
  const [systemKeyCombos, setSystemKeyCombos] = useState(Boolean(request?.system_key_combos));

  // A NEW request always starts from a fresh pre-checked state — never carry
  // over stale checkbox state from a previous request.
  useEffect(() => {
    setCheckedApps(initialCheckedApps(request));
    setClipboardRead(Boolean(request?.clipboard_read));
    setClipboardWrite(Boolean(request?.clipboard_write));
    setSystemKeyCombos(Boolean(request?.system_key_combos));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request?.request_id]);

  useEffect(() => {
    if (!request) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    primaryRef.current?.focus();
    return () => {
      if (promptHasFocus.current) previouslyFocused?.focus();
    };
  }, [request]);

  if (!request) return null;

  const tcc = request.tcc_state;

  const toggleApp = (label: string): void => {
    setCheckedApps((previous) => {
      const next = new Set(previous);
      if (next.has(label)) next.delete(label);
      else next.add(label);
      return next;
    });
  };

  const submit = (): void => {
    onSubmit(request.request_id, {
      granted_apps: request.apps.map((app) => app.label).filter((label) => checkedApps.has(label)),
      clipboard_read: clipboardRead,
      clipboard_write: clipboardWrite,
      system_key_combos: systemKeyCombos,
    });
  };

  const checkboxRowStyle: CSSProperties = {
    display: 'flex', alignItems: 'center', gap: 10,
    padding: '7px 2px', fontSize: 13, color: t.text,
  };

  if (!tcc) {
    const appName = request.apps.length === 1 ? request.apps[0]!.label : tr('desktop_computer_these_apps');
    const variables = {
      '--dialog-window': t.windowBg,
      '--dialog-surface': t.surface,
      '--dialog-surface-hover': t.surfaceHover,
      '--dialog-border': t.border,
      '--dialog-text': t.text,
      '--dialog-text-2': t.text2,
      '--computer-risk': t.dark ? '#ff9f66' : '#c74300',
    } as CSSProperties;
    return (
      <section
        className="inline-interaction-card computer-access-prompt"
        style={variables}
        role="region"
        aria-labelledby="lingxi-computer-access-title"
        aria-describedby="lingxi-computer-access-risk lingxi-computer-access-detail"
        onFocusCapture={() => { promptHasFocus.current = true; }}
        onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
        onKeyDown={(event) => {
          if (event.key !== 'Escape' || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
          event.preventDefault();
          event.stopPropagation();
          onDeny(request.request_id);
        }}
      >
        <div className="computer-access-content">
          <p className="computer-access-risk-label"><Icon name="shieldAlert" size={19} stroke={1.7} />{tr('desktop_computer_risk')}</p>
          <h2 id="lingxi-computer-access-title">{tr('desktop_computer_allow_prompt', appName)}</h2>
          <p id="lingxi-computer-access-risk" className="computer-access-copy">
            {tr(request.apps.length === 1 ? 'desktop_computer_prompt_risk_one' : 'desktop_computer_prompt_risk_many')}
          </p>
          <p id="lingxi-computer-access-detail" className="computer-access-reason">{request.reason}</p>
          <AppAllowlistPanel
            request={request}
            checkedApps={checkedApps}
            onToggleApp={toggleApp}
            clipboardRead={clipboardRead}
            onClipboardRead={setClipboardRead}
            clipboardWrite={clipboardWrite}
            onClipboardWrite={setClipboardWrite}
            systemKeyCombos={systemKeyCombos}
            onSystemKeyCombos={setSystemKeyCombos}
            checkboxRowStyle={checkboxRowStyle}
          />
        </div>
        <footer className="computer-access-footer">
          <span className="computer-access-scope">{tr('desktop_computer_session_tier_scope', request.tier === 'read' ? tr('desktop_computer_read_access') : request.tier === 'click' ? tr('desktop_computer_click_access') : tr('desktop_computer_full_access'))}</span>
          <DesktopDialogActions>
            <DesktopDialogButton variant="cancel" onClick={() => onDeny(request.request_id)}>
              {tr('permission_deny')} <kbd aria-hidden="true">Esc</kbd>
            </DesktopDialogButton>
            <DesktopDialogButton ref={primaryRef} variant="primary" onClick={submit}>
              {tr('desktop_computer_allow_session')} <kbd aria-hidden="true">↵</kbd>
            </DesktopDialogButton>
          </DesktopDialogActions>
        </footer>
      </section>
    );
  }

  return (
    <DesktopDialog
      title={tr('desktop_computer_macos_permissions')}
      summary={request.reason}
      icon="shieldAlert"
      size="regular"
      titleId="lingxi-computer-access-title"
      summaryId="lingxi-computer-access-detail"
      ariaLabelledBy="lingxi-computer-access-title"
      ariaDescribedBy="lingxi-computer-access-detail"
      className="computer-access-dialog"
      onFocusCapture={() => { promptHasFocus.current = true; }}
      onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
      onEscape={() => onDeny(request.request_id)}
      footer={(
        <DesktopDialogActions>
          <DesktopDialogButton
            variant="cancel"
            onClick={() => onDeny(request.request_id)}
          >
            {tr('permission_deny')}</DesktopDialogButton>
          <DesktopDialogButton ref={primaryRef} variant="primary" onClick={() => onDeny(request.request_id)}>
            {tr('desktop_computer_try_again')}</DesktopDialogButton>
        </DesktopDialogActions>
      )}
    >
      <TccPanel tcc={tcc} onOpenSystemSettings={onOpenSystemSettings} />
    </DesktopDialog>
  );
}

function TccPanel({
  tcc,
  onOpenSystemSettings,
}: {
  tcc: NonNullable<ComputerAccessRequestDto['tcc_state']>;
  onOpenSystemSettings(pane: SystemSettingsPane): void;
}) {
  const { tr } = useI18n();
  const t = useT();
  const rows: Array<{ pane: SystemSettingsPane; label: string; granted: boolean }> = [
    { pane: 'accessibility', label: tr('desktop_computer_accessibility'), granted: tcc.accessibility },
    { pane: 'screen_recording', label: tr('desktop_computer_screen_recording'), granted: tcc.screen_recording },
  ];
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      {rows.map((row) => (
        <div
          key={row.pane}
          style={{
            display: 'flex', alignItems: 'center', justifyContent: 'space-between',
            padding: '8px 10px', borderRadius: 8,
            background: t.surface, border: `0.5px solid ${t.border}`,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span
              aria-hidden="true"
              style={{
                width: 8, height: 8, borderRadius: '50%',
                background: row.granted ? t.ok : t.danger,
              }}
            />
            <span style={{ fontSize: 13, color: t.text }}>{row.label}</span>
            <span style={{ fontSize: 11, color: t.text3 }}>{row.granted ? tr('desktop_computer_granted') : tr('desktop_computer_not_granted')}</span>
          </div>
          {!row.granted && (
            <button
              type="button"
              onClick={() => onOpenSystemSettings(row.pane)}
              style={{
                padding: '5px 9px', borderRadius: 6, cursor: 'pointer',
                fontSize: 11.5, fontWeight: 600, fontFamily: 'inherit',
                color: t.accent, background: 'transparent',
                border: `0.5px solid ${t.accentBorder}`,
              }}
            >
              {tr('desktop_computer_open_system_settings', row.label)}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

function AppAllowlistPanel({
  request,
  checkedApps,
  onToggleApp,
  clipboardRead,
  onClipboardRead,
  clipboardWrite,
  onClipboardWrite,
  systemKeyCombos,
  onSystemKeyCombos,
  checkboxRowStyle,
}: {
  request: ComputerAccessRequestDto;
  checkedApps: Set<string>;
  onToggleApp(label: string): void;
  clipboardRead: boolean;
  onClipboardRead(value: boolean): void;
  clipboardWrite: boolean;
  onClipboardWrite(value: boolean): void;
  systemKeyCombos: boolean;
  onSystemKeyCombos(value: boolean): void;
  checkboxRowStyle: CSSProperties;
}) {
  const { tr } = useI18n();
  const t = useT();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div>
        <div style={{ fontSize: 10.5, fontWeight: 600, color: t.text3, textTransform: 'uppercase', letterSpacing: 0.4, marginBottom: 4 }}>
          {tr('desktop_computer_apps_tier', request.tier === 'read' ? tr('desktop_computer_tier_read') : request.tier === 'click' ? tr('desktop_computer_tier_click') : tr('desktop_computer_tier_full'))}
        </div>
        {request.apps.map((app) => (
          <label key={app.label} style={checkboxRowStyle}>
            <input
              type="checkbox"
              checked={checkedApps.has(app.label)}
              onChange={() => onToggleApp(app.label)}
            />
            {app.label}
          </label>
        ))}
      </div>
      {(request.clipboard_read || request.clipboard_write || request.system_key_combos) && (
        <div>
          <div style={{ fontSize: 10.5, fontWeight: 600, color: t.text3, textTransform: 'uppercase', letterSpacing: 0.4, marginBottom: 4 }}>
            {tr('desktop_computer_capabilities')}</div>
          {request.clipboard_read && (
            <label style={checkboxRowStyle}>
              <input type="checkbox" checked={clipboardRead} onChange={(event) => onClipboardRead(event.target.checked)} />
              {tr('desktop_computer_clipboard_read')}</label>
          )}
          {request.clipboard_write && (
            <label style={checkboxRowStyle}>
              <input type="checkbox" checked={clipboardWrite} onChange={(event) => onClipboardWrite(event.target.checked)} />
              {tr('desktop_computer_clipboard_write')}</label>
          )}
          {request.system_key_combos && (
            <label style={checkboxRowStyle}>
              <input type="checkbox" checked={systemKeyCombos} onChange={(event) => onSystemKeyCombos(event.target.checked)} />
              {tr('desktop_computer_system_shortcuts')}</label>
          )}
        </div>
      )}
    </div>
  );
}
