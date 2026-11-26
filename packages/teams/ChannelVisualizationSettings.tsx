import { useEffect, useState } from 'react';
import type { TeamsApi } from './index.js';
import { teamsText } from './i18n.js';
import { visualizationRequest, type VisualizationScope } from './visualization.js';
import { visualizationPackageBytes } from './visualization-transfer.js';

type Settings = { limitMiB: number; revision: number; canEdit: boolean };
const ceiling = visualizationPackageBytes / (1024 * 1024);

export function ChannelVisualizationSettings({ api, scope, locale }: {
  api: TeamsApi; scope: VisualizationScope; locale: string;
}) {
  const { teamId, conversationId, generation } = scope;
  const t = teamsText(locale);
  const [settings, setSettings] = useState<Settings>();
  const [limit, setLimit] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    void visualizationRequest<Settings>(api, { teamId, conversationId, generation }, { action: 'settings' }).then(value => {
      if (active) { setSettings(value); setLimit(String(value.limitMiB)); }
    }, () => { if (active) setError(true); });
    return () => { active = false; };
  }, [api, teamId, conversationId, generation]);

  if (!settings) return error ? <p role="status">{t('teams_data_unavailable')}</p> : null;
  if (!settings.canEdit) return <p>{t('teams_visualization_limit')} · {settings.limitMiB}</p>;
  const value = Number(limit);
  return <form className="tw-form" onSubmit={async event => {
    event.preventDefault();
    if (saving || !Number.isInteger(value) || value < 1 || value > ceiling || value === settings.limitMiB) return;
    setSaving(true); setError(false);
    try {
      setSettings(await visualizationRequest<Settings>(api, scope, { action: 'setLimit', limitMiB: value, expectedRevision: settings.revision }));
    } catch {
      const current = await visualizationRequest<Settings>(api, scope, { action: 'settings' }).catch(() => undefined);
      if (current) { setSettings(current); setLimit(String(current.limitMiB)); }
      setError(true);
    } finally { setSaving(false); }
  }}>
    <label>{t('teams_visualization_limit')}
      <input type="number" min={1} max={ceiling} step={1} required value={limit} disabled={saving} onChange={event => setLimit(event.target.value)} />
    </label>
    <button type="submit" className="tw-btn" disabled={saving || !Number.isInteger(value) || value < 1 || value > ceiling || value === settings.limitMiB}>{t('common_save')}</button>
    {error && <p role="alert">{t('teams_data_unavailable')}</p>}
  </form>;
}
