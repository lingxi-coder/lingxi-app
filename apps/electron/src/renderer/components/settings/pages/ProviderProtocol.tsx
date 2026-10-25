import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
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
  useI18n();
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
    catch (cause) { if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_6cdef1b894')); }
    finally { if (token === generation.current) setSaving(false); onLayerLockChange?.(false); }
  };
  return <Card title={tr('desktop_settings_e06568d15f')}>
    <Row title={tr('desktop_settings_2746420a13')} desc={tr('desktop_settings_5334aa2c77')} align="center">
      <select aria-label={tr('desktop_settings_bc58b64d0d')} value={typeof own === 'string' ? own : own === null ? '' : '__inherit__'} disabled={disabled || account.formats.length < 2}
        onChange={event => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px', maxWidth: '100%' }}>
        <option value="__inherit__">{tr('desktop_settings_inherit_current', protocolLabel(String(desired)))}</option>
        <option value="">{tr('desktop_settings_auto_format', protocolLabel(account.native))}</option>
        {account.formats.map(format => <option key={format} value={format}>{protocolLabel(format)}</option>)}
      </select>
    </Row>
    {active !== null && <Row title={tr('desktop_settings_09fef6487f', protocolLabel(String(active)))}
      desc={active !== desired ? tr('desktop_settings_2b334ad347', protocolLabel(String(desired))) : undefined} align="center">
      {active !== desired && <button type="button" disabled={disabled} style={ghostButtonStyle(t)} onClick={() => {
        const token = generation.current; setSaving(true); setError(null);
        void bridge.restartBridge().catch((cause: unknown) => {
          if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_04a1a37d03'));
        }).finally(() => { if (token === generation.current) setSaving(false); });
      }}>{tr('settings_provider_apply_reconnect')}</button>}
    </Row>}
    {error && <Row title={tr('desktop_settings_f48af7b362')}><span role="alert">{error}</span></Row>}
  </Card>;
}
