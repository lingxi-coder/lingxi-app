import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from '../SettingsScreen';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';

export function normalizedProviderRegion(value: unknown): 'international' | 'china_mainland' {
  return value === 'china_mainland' ? 'china_mainland' : 'international';
}

const label = (region: string) => region === 'china_mainland' ? tr('settings_provider_region_china') : tr('settings_provider_region_international');

export function ProviderRegion({ bridge, editingLayer, onLayerLockChange }: Pick<PageContentProps, 'bridge' | 'snapshot' | 'editingLayer' | 'onJumpToLayer' | 'onLayerLockChange'>) {
  useI18n();
  const t = useT();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  useEffect(() => {
    generation.current += 1;
    setSaving(false);
    setError(null);
    return () => { generation.current += 1; };
  }, [editingLayer, bridge.activeSession?.sessionId]);
  const own = bridge.providerSettings?.providerRegion;
  const desired = normalizedProviderRegion(own);
  const disabled = saving || !bridge.providerSettings || bridge.running || bridge.providerSettingsLocked?.includes('providerRegion');

  const write = async (value: string) => {
    if (disabled) return;
    const token = generation.current;
    setSaving(true); setError(null); onLayerLockChange?.(true);
    try {
      await bridge.updateProviderSettings({ providerRegion: value || null });
    } catch (cause) {
      if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_4c91dcc54b'));
    } finally {
      if (token === generation.current) setSaving(false);
      onLayerLockChange?.(false);
    }
  };

  return <Card title={tr('settings_provider_region')}>
    {bridge.providerSettingsLocked?.includes('providerRegion') && <Row title="Managed policy"><span>Locked</span></Row>}
    <Row title={tr('desktop_settings_d50e29c2a9')} desc={tr('desktop_settings_d3bed56ee7')} align="center">
      <select aria-label={tr('settings_provider_region')} value={typeof own === 'string' ? own : ''} disabled={disabled}
        onChange={(event) => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px' }}>
        <option value="">{tr('desktop_settings_inherit_current', label(desired))}</option>
        <option value="international">{tr('settings_provider_region_international')}</option>
        <option value="china_mainland">{tr('settings_provider_region_china')}</option>
      </select>
    </Row>
    {error && <Row title={tr('desktop_settings_f48af7b362')}><span role="alert">{error}</span></Row>}
  </Card>;
}
