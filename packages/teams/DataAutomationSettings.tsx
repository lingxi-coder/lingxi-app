import { useState, type FormEvent, type ReactElement } from 'react';
import type { TeamsApi } from './index.js';
import type { DataAutomationSettings as Settings, DataRuleConfig, TeamDataScope } from './lingxi-data.js';

export function DataAutomationSettings({ api, scope, dataset, fields, locale }: {
  api: TeamsApi; scope: TeamDataScope; dataset: string; fields: string[]; locale: string;
}): ReactElement | null {
  const [settings, setSettings] = useState<Settings>();
  const [selected, setSelected] = useState('');
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [transition, setTransition] = useState(false);
  const zh = locale.toLowerCase().startsWith('zh');
  const bot = settings?.bots.find(entry => entry.sharedBotId === selected);
  if (!api.data) return null;
  const data = api.data;

  async function refresh(): Promise<void> {
    setSettings(await data<Settings>({ scope, command: { action: 'dataAutomation', dataset } }));
  }

  async function perform(action: () => Promise<unknown>): Promise<void> {
    setBusy(true); setFailed(false);
    try { await action(); }
    catch { setFailed(true); }
    finally { setBusy(false); }
  }

  function saveAccess(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!bot) return;
    const form = new FormData(event.currentTarget);
    const access = form.get('access');
    const targets = form.getAll('handoffTargets').map(String);
    if (access !== 'read' && access !== 'edit' && access !== 'none') return;
    void perform(async () => {
      await data({ action: 'setBotDataAccess', scope, dataset, sharedBotId: bot.sharedBotId, expectedVersion: bot.version,
        access: access === 'none' ? null : access,
        ...(access === 'edit' && targets.length ? { handoff: { targets, dailyLimit: Number(form.get('handoffLimit')) } } : {}),
      });
      await refresh();
    });
  }

  function saveRule(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!settings) return;
    const form = new FormData(event.currentTarget);
    const rule = String(form.get('rule'));
    void perform(async () => {
      let definition: DataRuleConfig['definition'] = { kind: 'created' };
      if (transition) {
        const equals: unknown = JSON.parse(String(form.get('value')));
        if (equals !== null && typeof equals !== 'string' && typeof equals !== 'number' && typeof equals !== 'boolean') throw new Error('Invalid rule condition');
        definition = { kind: 'transition', field: String(form.get('field')), equals };
      }
      await data({ action: 'setDataRule', scope, dataset, rule, expectedVersion: settings.rules.find(value => value.rule === rule)?.version ?? 0,
        config: { sharedBotId: String(form.get('ruleBot')), dailyLimit: Number(form.get('ruleLimit')), definition } });
      await refresh();
    });
  }

  return <details className="tw-data-workspace" onToggle={event => {
    if (event.currentTarget.open && !settings && !busy) void perform(refresh);
  }}>
    <summary>{zh ? 'Bot 授权与规则' : 'Bot access and rules'}</summary>
    {failed && <p role="alert">{zh ? '无法更新自动化设置。' : 'Could not update automation settings.'}</p>}
    {!settings && failed && <button type="button" onClick={() => void perform(refresh)}>{zh ? '重试' : 'Retry'}</button>}
    {settings && <form key={bot ? bot.sharedBotId + ':' + bot.version : 'new'} method="post" onSubmit={saveAccess}>
      <label>Bot<select name="bot" value={selected} onChange={event => setSelected(event.currentTarget.value)} required>
        <option value="">{zh ? '选择 Bot' : 'Select a Bot'}</option>
        {settings.bots.map(value => <option key={value.sharedBotId} value={value.sharedBotId}>{value.name}</option>)}
      </select></label>
      {bot && <>
        <label>{zh ? '操作权限' : 'Access'}<select name="access" defaultValue={bot.access ?? 'none'}>
          <option value="none">{zh ? '无权限' : 'None'}</option><option value="read">{zh ? '读取' : 'Read'}</option>
          <option value="edit">{zh ? '读取与写入' : 'Read and write'}</option></select></label>
        <label>{zh ? '允许交接给' : 'May hand off to'}<select name="handoffTargets" multiple defaultValue={bot.handoff?.targets}>
          {settings.bots.filter(value => value.sharedBotId !== bot.sharedBotId && value.access).map(value =>
            <option key={value.sharedBotId} value={value.sharedBotId}>{value.name}</option>)}</select></label>
        <label>{zh ? '每日交接额度' : 'Daily handoff limit'}<input name="handoffLimit" type="number" min={1} max={100}
          defaultValue={bot.handoff?.dailyLimit ?? 10} required /></label>
        <button type="submit" disabled={busy}>{zh ? '保存 Bot 权限' : 'Save Bot access'}</button>
      </>}
    </form>}
    {settings && settings.bots.some(value => value.access) && <form method="post" onSubmit={saveRule}>
      <label>{zh ? '规则 ID' : 'Rule ID'}<input name="rule" required pattern="[A-Za-z0-9_\-]{1,64}" /></label>
      <label>{zh ? '执行 Bot' : 'Bot to run'}<select name="ruleBot" required>
        {settings.bots.filter(value => value.access).map(value => <option key={value.sharedBotId} value={value.sharedBotId}>{value.name}</option>)}
      </select></label>
      <label>{zh ? '触发条件' : 'Trigger'}<select name="trigger" value={transition ? 'transition' : 'created'} onChange={event => setTransition(event.currentTarget.value === 'transition')}>
        <option value="created">{zh ? '新增记录' : 'Record created'}</option>
        <option value="transition">{zh ? '字段变为指定值' : 'Field changes to a value'}</option></select></label>
      {transition && <>
        <label>{zh ? '字段' : 'Field'}<select name="field" required>{fields.map(field => <option key={field}>{field}</option>)}</select></label>
        <label>{zh ? '目标值（JSON）' : 'Target value (JSON)'}<input name="value" required /></label>
      </>}
      <label>{zh ? '每日触发额度' : 'Daily rule limit'}<input name="ruleLimit" type="number" min={1} max={100} defaultValue={10} required /></label>
      <button type="submit" disabled={busy}>{zh ? '保存并启用规则' : 'Save and enable rule'}</button>
    </form>}
    {!!settings?.rules.length && <ul>{settings.rules.filter(rule => rule.config).map(rule => <li key={rule.rule}>{rule.rule}{' '}
      <button type="button" disabled={busy} onClick={() => void perform(async () => {
        await data({ action: 'setDataRule', scope, dataset, rule: rule.rule, expectedVersion: rule.version, config: null });
        await refresh();
      })}>{zh ? '停用规则' : 'Disable rule'}</button></li>)}</ul>}
  </details>;
}
