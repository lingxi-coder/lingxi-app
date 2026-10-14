import { useMemo } from 'react';
import type { FusionStrategyRefDto } from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';
import { FUSION_SCOPE_LABELS, parseFusionCatalog } from './settings/pages/FusionCatalog';
import { fusionStrategyKey, parseFusionStrategyRef } from '../../shared/fusionStrategy';

export function FusionStrategyPicker({ catalogJson, value, explicit, disabled, onChange, onSettings }: {
  catalogJson?: string;
  value: FusionStrategyRefDto | null;
  explicit: boolean;
  disabled: boolean;
  onChange(value: FusionStrategyRefDto | null): void;
  onSettings(): void;
}) {
  const t = useT();
  const catalog = useMemo(() => parseFusionCatalog(catalogJson), [catalogJson]);
  const entries = catalog.scopes.flatMap((scope) => scope.strategies.map((strategy) => ({
    ...strategy, scope: scope.scope, admitted: scope.admitted, key: fusionStrategyKey({ scope: scope.scope, id: strategy.id }),
  })));
  const defaultKey = catalog.default_strategy ? fusionStrategyKey(catalog.default_strategy) : null;
  const defaultEntry = entries.find((entry) => entry.admitted && entry.key === defaultKey);
  const selected = value ? fusionStrategyKey(value) : '';
  return <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8, padding: '12px 18px 0' }}>
    <label style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 12, color: t.text3, minWidth: 0 }}>
      Fusion
      <select aria-label="Fusion 策略" disabled={disabled || explicit} value={selected}
        title={explicit ? '策略由输入的命令指定' : '选择本次运行的 Fusion 策略'}
        onChange={(event) => onChange(event.target.value ? parseFusionStrategyRef(event.target.value) : null)}
        style={{ maxWidth: 'min(320px, 50vw)', minWidth: 120, border: `1px solid ${t.border}`, borderRadius: 7,
          background: t.surface, color: t.text, padding: '5px 8px', font: 'inherit' }}>
        <option value="">{defaultEntry ? `默认 · ${defaultEntry.name} · ${FUSION_SCOPE_LABELS[defaultEntry.scope]}` : '选择策略…'}</option>
        {selected && !entries.some((entry) => entry.key === selected) && <option value={selected}>策略已不可用</option>}
        {entries.map((entry) => <option key={entry.key} value={entry.key} disabled={!entry.admitted}>{entry.name} · {FUSION_SCOPE_LABELS[entry.scope]}{entry.admitted ? '' : ' · 策略已不可用'}</option>)}
      </select>
    </label>
    <button type="button" onClick={onSettings} style={{ color: t.text3, fontSize: 12, padding: '5px 2px', border: 0, background: 'transparent', cursor: 'pointer' }}>管理策略</button>
  </div>;
}
