import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from '../SettingsScreen';
import { Card, Row } from '../rows';
import { rowState } from '../useEngineSettings';
import { useT } from '../../../theme/ThemeContext';
import { PROVIDER_PROTOCOLS } from '../../../../shared/providerProtocols';
import { asRecord } from './configurationAdmin';
import { ghostButtonStyle } from './ghostButton';

export const protocolLabel = (value: string) => ({
  open_ai_chat: 'OpenAI Chat Completions', open_ai_responses: 'OpenAI Responses',
  anthropic_messages: 'Anthropic Messages', gemini_generate_content: 'Gemini Generate Content',
} as Record<string, string>)[value] ?? value;

export function protocolDefaults(value: unknown): Record<string, unknown> {
  return asRecord(asRecord(value).defaultProtocols);
}

/** Edit the selected layer only; preserve its other routing entries. */
export function protocolSettingsPatch(routing: unknown, profile: string, format: string) {
  const defaults = { ...protocolDefaults(routing) };
  if (format === '__inherit__') delete defaults[profile];
  else defaults[profile] = format || null;
  return { routing: { ...asRecord(routing), defaultProtocols: defaults } };
}

export function ProviderProtocol({ providerId, bridge, snapshot, editingLayer, onLayerLockChange }: Pick<PageContentProps,
  'bridge' | 'snapshot' | 'editingLayer' | 'onJumpToLayer' | 'onLayerLockChange'> & { providerId: string }) {
  const t = useT();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  useEffect(() => {
    generation.current += 1; setSaving(false); setError(null);
    return () => { generation.current += 1; };
  }, [providerId, editingLayer, bridge.activeSession?.sessionId]);
  const region = snapshot?.effective.providerRegion === 'china_mainland' ? 'china_mainland' : 'international';
  const profileId = providerId === 'glm-coding' ? 'zai-coding' : providerId;
  const account = PROVIDER_PROTOCOLS.find(p => p.profile === profileId && p.regions.includes(region))
    ?? PROVIDER_PROTOCOLS.find(p => p.profile === `${profileId}-intl` && p.regions.includes(region));
  if (!account) return null;
  const state = snapshot ? rowState(snapshot, 'routing', editingLayer) : null;
  const ownRouting = snapshot?.layers[editingLayer]?.routing;
  const own = protocolDefaults(ownRouting)[account.profile];
  const desired = protocolDefaults(snapshot?.effective.routing)[account.profile] ?? account.native;
  const active = snapshot?.active ? protocolDefaults(snapshot.active.routing)[account.profile] ?? account.native : null;
  const disabled = saving || bridge.running || !snapshot?.layers[editingLayer]
    || state?.kind === 'locked' || state?.kind === 'layer-broken';
  const write = async (format: string) => {
    if (disabled) return;
    const token = generation.current; setSaving(true); setError(null); onLayerLockChange?.(true);
    try { await bridge.updateProviderSettings(editingLayer, protocolSettingsPatch(ownRouting, account.profile, format)); }
    catch (cause) { if (token === generation.current) setError(cause instanceof Error ? cause.message : '无法保存默认格式。'); }
    finally { if (token === generation.current) setSaving(false); onLayerLockChange?.(false); }
  };
  return <Card title="请求格式">
    <Row title="默认格式" desc="请求格式保存在所选配置层；账号凭据保存在此设备。搜索与语音使用各自支持的连接方式。" align="center">
      <select aria-label="默认请求格式" value={typeof own === 'string' ? own : own === null ? '' : '__inherit__'} disabled={disabled || account.formats.length < 2}
        onChange={event => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px', maxWidth: '100%' }}>
        <option value="__inherit__">继承（当前：{protocolLabel(String(desired))}）</option>
        <option value="">自动（{protocolLabel(account.native)}）</option>
        {account.formats.map(format => <option key={format} value={format}>{protocolLabel(format)}</option>)}
      </select>
    </Row>
    {active !== null && <Row title={`当前运行：${protocolLabel(String(active))}`}
      desc={active !== desired ? `已保存：${protocolLabel(String(desired))}。重新连接或重启 app 后应用。` : undefined} align="center">
      {active !== desired && <button type="button" disabled={disabled} style={ghostButtonStyle(t)} onClick={() => {
        const token = generation.current; setSaving(true); setError(null);
        void bridge.restartBridge().catch((cause: unknown) => {
          if (token === generation.current) setError(cause instanceof Error ? cause.message : '无法重新连接。');
        }).finally(() => { if (token === generation.current) setSaving(false); });
      }}>应用并重新连接</button>}
    </Row>}
    {error && <Row title="保存或应用失败"><span role="alert">{error}</span></Row>}
  </Card>;
}
