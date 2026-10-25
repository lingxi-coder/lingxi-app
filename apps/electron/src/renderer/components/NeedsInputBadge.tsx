import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useT } from '../theme/ThemeContext';
import type { SessionRuntimeStatus } from '../bridge/bridgeTypes';

export function sessionNeedsInput(status?: Pick<SessionRuntimeStatus, 'pendingInteractions' | 'pendingAskUserQuestions'>): boolean {
  return (status?.pendingInteractions ?? 0) > 0 || (status?.pendingAskUserQuestions ?? 0) > 0;
}

export function NeedsInputBadge() {
  const t = useT();
  useI18n();
  return <span className="session-needs-input" role="status" style={{
    flexShrink: 0, marginLeft: 8, padding: '2px 9px', borderRadius: 999,
    background: t.dark ? '#19365c' : '#dfe8fc', color: t.dark ? '#8ab6ff' : '#3478f6',
    fontSize: 11.5, lineHeight: '18px', fontWeight: 500, whiteSpace: 'nowrap',
  }}>{tr("desktop_ui_needs_input")}</span>;
}
