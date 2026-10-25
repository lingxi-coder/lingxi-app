import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from '../SettingsScreen';
import { Card, isEditableLayer, OverriddenNotice, Row } from '../rows';
import { rowState } from '../useEngineSettings';
import { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle } from './ghostButton';

export function normalizedProviderRegion(value: unknown): 'international' | 'china_mainland' {
  return value === 'china_mainland' ? 'china_mainland' : 'international';
}

const label = (region: string) => region === 'china_mainland' ? tr('settings_provider_region_china') : tr('settings_provider_region_international');

export function ProviderRegion({ bridge, snapshot, editingLayer, onJumpToLayer, onLayerLockChange }: Pick<PageContentProps, 'bridge' | 'snapshot' | 'editingLayer' | 'onJumpToLayer' | 'onLayerLockChange'>) {
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
  const state = snapshot ? rowState(snapshot, 'providerRegion', editingLayer) : null;
  const own = snapshot?.layers[editingLayer]?.providerRegion;
  const desired = normalizedProviderRegion(snapshot?.effective.providerRegion);
  const active = snapshot?.active ? normalizedProviderRegion(snapshot.active.providerRegion) : null;
  const pending = active !== null && desired !== active;
  const disabled = saving || !snapshot?.layers[editingLayer] || state?.kind === 'locked' || state?.kind === 'layer-broken';

  const write = async (value: string) => {
    if (disabled) return;
    const token = generation.current;
    setSaving(true); setError(null); onLayerLockChange?.(true);
    try {
      await bridge.updateProviderSettings(editingLayer, { providerRegion: value || null });
    } catch (cause) {
      if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_4c91dcc54b'));
    } finally {
      if (token === generation.current) setSaving(false);
      onLayerLockChange?.(false);
    }
  };

  return <Card title={tr('settings_provider_region')}>
    <Row title={tr('desktop_settings_d50e29c2a9')} desc={tr('desktop_settings_d3bed56ee7')} align="center">
      <select aria-label={tr('settings_provider_region')} value={typeof own === 'string' ? own : ''} disabled={disabled}
        onChange={(event) => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px' }}>
        <option value="">{tr('desktop_settings_inherit_current', label(desired))}</option>
        <option value="international">{tr('settings_provider_region_international')}</option>
        <option value="china_mainland">{tr('settings_provider_region_china')}</option>
      </select>
    </Row>
    {state?.kind === 'overridden' && <Row title={tr('desktop_settings_ad25986d5b')}>
      <OverriddenNotice editingLayer={editingLayer} effectiveLayer={state.by}
        onJump={() => { if (isEditableLayer(state.by)) onJumpToLayer(state.by); }} />
    </Row>}
    {active !== null && <Row title={tr('desktop_settings_09fef6487f', label(active))} desc={pending ? tr('desktop_settings_d1f2d32000') : undefined} align="center">
      {pending && <button type="button" disabled={saving} style={ghostButtonStyle(t)} onClick={() => {
        const token = generation.current;
        setError(null); setSaving(true);
        void bridge.restartBridge().catch((cause: unknown) => {
          if (token === generation.current) setError(cause instanceof Error ? cause.message : tr('desktop_settings_4f7295ff1a'));
        }).finally(() => { if (token === generation.current) setSaving(false); });
      }}>{tr('settings_provider_apply_reconnect')}</button>}
    </Row>}
    {error && <Row title={tr('desktop_settings_f48af7b362')}><span role="alert">{error}</span></Row>}
  </Card>;
}
