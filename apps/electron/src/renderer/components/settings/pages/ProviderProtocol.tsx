import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from '../SettingsScreen';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { PROVIDER_PROTOCOLS } from '../../../../shared/providerProtocols';
import { asRecord } from './configurationAdmin';

export const protocolLabel = (value: string) => ({
  open_ai_chat: 'OpenAI Chat Completions', open_ai_responses: 'OpenAI Responses',
  anthropic_messages: 'Anthropic Messages', gemini_generate_content: 'Gemini Generate Content',
} as Record<string, string>)[value] ?? value;

export function protocolDefaults(value: unknown): Record<string, unknown> {
  return asRecord(asRecord(value).defaultProtocols);
}

/** Edit Host-wide protocol defaults while preserving the remaining routing entries. */
export function protocolSettingsPatch(routing: unknown, profile: string, format: string) {
  const defaults = { ...protocolDefaults(routing) };
  if (format === '__inherit__') delete defaults[profile];
  else defaults[profile] = format || null;
  return { routing: { ...asRecord(routing), defaultProtocols: defaults } };
}

export function ProviderProtocol({ providerId, bridge, onLayerLockChange }: Pick<PageContentProps,
  'bridge' | 'onLayerLockChange'> & { providerId: string }) {
  useI18n();
  const t = useT();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  useEffect(() => {
    generation.current += 1; setSaving(false); setError(null);
    return () => { generation.current += 1; };
  }, [providerId]);
  const region = bridge.providerSettings?.providerRegion === 'china_mainland' ? 'china_mainland' : 'international';
  const profileId = providerId === 'glm-coding' ? 'zai-coding' : providerId;
  const account = PROVIDER_PROTOCOLS.find(p => p.profile === profileId && p.regions.includes(region))
    ?? PROVIDER_PROTOCOLS.find(p => p.profile === `${profileId}-intl` && p.regions.includes(region));
  if (!account) return null;
  const ownRouting = bridge.providerSettings?.routing;
  const own = protocolDefaults(ownRouting)[account.profile];
  const desired = own ?? account.native;
  const disabled = saving || bridge.running || !bridge.providerSettings || bridge.providerSettingsLocked?.includes('routing');
  const write = async (format: string) => {
    if (disabled) return;
    const token = generation.current; setSaving(true); setError(null); onLayerLockChange?.(true);
    try { await bridge.updateProviderSettings(protocolSettingsPatch(ownRouting, account.profile, format)); }
    catch (cause) { if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_6cdef1b894')); }
    finally { if (token === generation.current) setSaving(false); onLayerLockChange?.(false); }
  };
  return <Card title={tr('desktop_settings_e06568d15f')}>
    {bridge.providerSettingsLocked?.includes('routing') && <Row title="Managed policy"><span>Locked</span></Row>}
    <Row title={tr('desktop_settings_2746420a13')} desc={tr('desktop_settings_5334aa2c77')} align="center">
      <select aria-label={tr('desktop_settings_bc58b64d0d')} value={typeof own === 'string' ? own : own === null ? '' : '__inherit__'} disabled={disabled || account.formats.length < 2}
        onChange={event => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px', maxWidth: '100%' }}>
        <option value="__inherit__">{tr('desktop_settings_inherit_current', protocolLabel(String(desired)))}</option>
        <option value="">{tr('desktop_settings_auto_format', protocolLabel(account.native))}</option>
        {account.formats.map(format => <option key={format} value={format}>{protocolLabel(format)}</option>)}
      </select>
    </Row>
    {error && <Row title={tr('desktop_settings_f48af7b362')}><span role="alert">{error}</span></Row>}
  </Card>;
}
