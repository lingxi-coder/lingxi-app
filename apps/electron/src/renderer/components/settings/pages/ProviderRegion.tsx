import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from '../SettingsScreen';
import { Card, isEditableLayer, OverriddenNotice, Row } from '../rows';
import { rowState } from '../useEngineSettings';
import { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle } from './ghostButton';

export function normalizedProviderRegion(value: unknown): 'international' | 'china_mainland' {
  return value === 'china_mainland' ? 'china_mainland' : 'international';
}

const label = (region: string) => region === 'china_mainland' ? '中国大陆' : '国际';

export function ProviderRegion({ bridge, snapshot, editingLayer, onJumpToLayer, onLayerLockChange }: Pick<PageContentProps, 'bridge' | 'snapshot' | 'editingLayer' | 'onJumpToLayer' | 'onLayerLockChange'>) {
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
      if (token === generation.current) setError(cause instanceof Error ? cause.message : '无法保存模型区域。');
    } finally {
      if (token === generation.current) setSaving(false);
      onLayerLockChange?.(false);
    }
  };

  return <Card title="模型区域">
    <Row title="使用区域" desc="模型列表和请求按区域筛选。未设置时使用国际区域；其他区域的配置与凭据会保留。" align="center">
      <select aria-label="模型使用区域" value={typeof own === 'string' ? own : ''} disabled={disabled}
        onChange={(event) => { void write(event.target.value); }}
        style={{ background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px' }}>
        <option value="">继承（当前：{label(desired)}）</option>
        <option value="international">国际</option>
        <option value="china_mainland">中国大陆</option>
      </select>
    </Row>
    {state?.kind === 'overridden' && <Row title="生效层">
      <OverriddenNotice editingLayer={editingLayer} effectiveLayer={state.by}
        onJump={() => { if (isEditableLayer(state.by)) onJumpToLayer(state.by); }} />
    </Row>}
    {active !== null && <Row title={`当前运行：${label(active)}`} desc={pending ? '区域已保存。任务空闲后重新连接以应用；当前请求不会被切换。' : undefined} align="center">
      {pending && <button type="button" disabled={saving} style={ghostButtonStyle(t)} onClick={() => {
        const token = generation.current;
        setError(null); setSaving(true);
        void bridge.restartBridge().catch((cause: unknown) => {
          if (token === generation.current) setError(cause instanceof Error ? cause.message : '暂时无法重新连接。');
        }).finally(() => { if (token === generation.current) setSaving(false); });
      }}>应用并重新连接</button>}
    </Row>}
    {error && <Row title="保存或应用失败"><span role="alert">{error}</span></Row>}
  </Card>;
}
