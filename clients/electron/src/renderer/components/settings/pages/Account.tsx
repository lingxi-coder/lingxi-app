import type { AuthStateDto } from '@lingxi/bridge-client';

import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { ghostButtonStyle } from './ghostButton';
import { credentialStatusKind, type CredentialStatusKind } from './ProviderCredentials';
import { providerById } from '../../../../shared/providers';

function authSummary(auth: AuthStateDto | null | undefined): { title: string; detail: string } {
  if (!auth) return { title: '状态未知', detail: '尚未收到 auth_state。' };
  if (auth.type === 'signed_in') return { title: '已登录', detail: `${auth.email} · ${auth.org_id}` };
  return { title: '未登录', detail: '当前引擎没有持久登录态。' };
}

function statusCopy(kind: CredentialStatusKind): string {
  switch (kind) {
    case 'runtime': return '运行时提供';
    case 'secure': return '安全存储';
    case 'fallback-configured': return '已配置，回退存储';
    case 'fallback-unconfigured': return '未配置，需回退';
    default: return '未设置';
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
      <Card title="账户状态">
        <Row title="引擎登录" desc="登录和登出通过引擎命令执行；这里展示当前 auth_state。">
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
                刷新状态
              </button>
              <button
                type="button"
                disabled={!sessionReady || turnBusy}
                onClick={() => { void bridge.login().catch(() => undefined); }}
                style={ghostButtonStyle(t, !sessionReady || turnBusy)}
              >
                登录
              </button>
              <button
                type="button"
                disabled={!sessionReady || turnBusy}
                onClick={() => { void bridge.logout().catch(() => undefined); }}
                style={ghostButtonStyle(t, !sessionReady || turnBusy)}
              >
                登出
              </button>
              <button
                type="button"
                onClick={() => onNavigate('provider-credentials')}
                style={ghostButtonStyle(t)}
              >
                打开凭据页
              </button>
            </div>
          </div>
        </Row>

        <Row title="可用说明" desc="auth_state 只告诉你是否登录；真正的密钥、API base URL 和提供者级别状态仍在 Provider 凭据页里编辑。">
          <button type="button" onClick={() => onNavigate('provider-credentials')} style={ghostButtonStyle(t)}>管理 Provider 凭据</button>
        </Row>
      </Card>

      <Card title="Provider 凭据概览">
        <div style={{ padding: '10px 18px 0', color: t.text3, fontSize: 12.5, lineHeight: 1.55 }}>
          这里只做状态总览，不替代安全编辑页。已配置的凭据与运行时提供的值都会显示出来。
        </div>
        {providerCredentials.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有保存任何 Provider 凭据。</div>
        )}
        {providerCredentials.map((entry) => {
          const provider = providerById(entry.providerId);
          const kind = credentialStatusKind(entry);
          return (
            <Row
              key={entry.providerId}
              title={provider?.label ?? entry.providerId}
              desc={`${statusCopy(kind)} · ${entry.configured ? '已配置' : '未配置'}${entry.encryptionAvailable ? ' · 可加密' : ''}${entry.runtimeOnly ? ' · 仅运行时' : ''}`}
              align="center"
            >
              <button
                type="button"
                onClick={() => onNavigate('provider-credentials')}
                style={ghostButtonStyle(t)}
              >
                打开
              </button>
            </Row>
          );
        })}
      </Card>
    </>
  );
}
