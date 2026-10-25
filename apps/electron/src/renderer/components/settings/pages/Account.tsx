import { tr } from '../../../i18n/locale';
import { LingxiAccount } from './LingxiAccount';
import type { AuthStateDto } from '@lingxi/bridge-client';

import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { ghostButtonStyle } from './ghostButton';
import { credentialStatusKind, type CredentialStatusKind } from './ProviderCredentials';
import { providerById } from '../../../../shared/providers';

function authSummary(auth: AuthStateDto | null | undefined): { title: string; detail: string } {
  if (!auth) return { title: tr('desktop_shell_auth_unknown'), detail: tr('desktop_shell_auth_pending') };
  if (auth.type === 'signed_in') return { title: tr('account_signed_in'), detail: `${auth.email} · ${auth.org_id}` };
  return { title: tr('desktop_shell_signed_out'), detail: tr('desktop_shell_engine_signed_out') };
}

function statusCopy(kind: CredentialStatusKind): string {
  switch (kind) {
    case 'runtime': return tr('desktop_shell_runtime_credential');
    case 'secure': return tr('desktop_shell_secure_storage');
    case 'fallback-configured': return tr('desktop_shell_fallback_configured');
    case 'fallback-unconfigured': return tr('desktop_shell_fallback_unconfigured');
    default: return tr('common_unset');
  }
}

export function Account({ bridge, onNavigate }: PageContentProps) {
  const t = useT();
  const auth = bridge.desktop.auth;
  const summary = authSummary(auth);
  const providerCredentials = bridge.bootstrap?.providerCredentials ?? [];
  const sessionReady = Boolean(bridge.activeSession?.sessionId) && !bridge.sessionLoading;
  const turnBusy = bridge.running || bridge.sessionLoading;

  return (
    <>
      <LingxiAccount />
      <Card title={tr('desktop_shell_account_state')}>
        <Row title={tr('desktop_shell_engine_login')} desc={tr('desktop_shell_engine_login_desc')}>
          <div style={{ display: 'grid', gap: 8, justifyItems: 'end' }}>
            <div style={{ textAlign: 'right' }}>
              <div style={{ fontSize: 14, fontWeight: 600, color: t.text }}>{summary.title}</div>
              <div style={{ fontSize: 12.5, color: t.text3, marginTop: 3 }}>{summary.detail}</div>
            </div>
            <div style={{ display: 'flex', flexWrap: 'wrap', justifyContent: 'flex-end', gap: 8 }}>
              <button
                type="button"
                disabled={!sessionReady || turnBusy}
                onClick={() => { void bridge.refreshAuth().catch(() => undefined); }}
                style={ghostButtonStyle(t, !sessionReady || turnBusy)}
              >
                {tr('settings_linux_action_refresh')}
              </button>
              <button
                type="button"
                disabled={!sessionReady || turnBusy}
                onClick={() => { void bridge.login().catch(() => undefined); }}
                style={ghostButtonStyle(t, !sessionReady || turnBusy)}
              >
                {tr('settings_provider_login')}
              </button>
              <button
                type="button"
                disabled={!sessionReady || turnBusy}
                onClick={() => { void bridge.logout().catch(() => undefined); }}
                style={ghostButtonStyle(t, !sessionReady || turnBusy)}
              >
                {tr('settings_logout')}
              </button>
              <button
                type="button"
                onClick={() => onNavigate('provider-credentials')}
                style={ghostButtonStyle(t)}
              >
                {tr('desktop_shell_open_credentials')}
              </button>
            </div>
          </div>
        </Row>

        <Row title={tr('desktop_shell_availability_info')} desc={tr('desktop_shell_auth_info')}>
          <button type="button" onClick={() => onNavigate('provider-credentials')} style={ghostButtonStyle(t)}>{tr('settings_parity_manage_credentials')}</button>
        </Row>
      </Card>

      <Card title={tr('desktop_shell_credentials_overview')}>
        <div style={{ padding: '10px 18px 0', color: t.text3, fontSize: 12.5, lineHeight: 1.55 }}>
          {tr('desktop_shell_credentials_desc')}
        </div>
        {providerCredentials.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_shell_no_credentials')}</div>
        )}
        {providerCredentials.map((entry) => {
          const provider = providerById(entry.providerId);
          const kind = credentialStatusKind(entry);
          return (
            <Row
              key={entry.providerId}
              title={provider?.label ?? entry.providerId}
              desc={`${statusCopy(kind)} · ${entry.configured ? tr('settings_parity_secret_configured') : tr('settings_parity_not_configured')}${entry.encryptionAvailable ? ` · ${tr('desktop_shell_encryptable')}` : ''}${entry.runtimeOnly ? ` · ${tr('desktop_shell_runtime_only')}` : ''}`}
              align="center"
            >
              <button
                type="button"
                onClick={() => onNavigate('provider-credentials')}
                style={ghostButtonStyle(t)}
              >
                {tr('common_open')}
              </button>
            </Row>
          );
        })}
      </Card>
    </>
  );
}
