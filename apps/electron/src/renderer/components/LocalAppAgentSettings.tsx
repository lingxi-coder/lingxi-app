import { useEffect, useState } from 'react';
import type { DesktopLocalApp, LocalAppAgentQuestion } from '../../shared/localApps';
import { LOCAL_APP_AGENT_DEFAULT_MAX_TURNS, LOCAL_APP_AGENT_DEFAULT_TIMEOUT_MS, type LocalAppAgentCatalog, type LocalAppAgentSettings, type LocalAppAgentToolGrant } from '../../shared/localAppAgent';
import { AskUserQuestionPrompt } from './AskUserQuestionPrompt';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';

function api() { const value = window.lingxi?.localApps; if (!value) throw new Error('Desktop application host is unavailable'); return value; }
function sameTool(a: LocalAppAgentToolGrant, b: LocalAppAgentToolGrant): boolean {
  if (a.id !== b.id || a.kind !== b.kind) return false;
  return a.kind === 'builtin' && b.kind === 'builtin' ? a.name === b.name : a.kind === 'mcp' && b.kind === 'mcp'
    && a.serverConfigId === b.serverConfigId && a.configRevision === b.configRevision && a.rawToolName === b.rawToolName && a.definitionDigest === b.definitionDigest;
}

export function LocalAppAgentSettingsDialog({ app, onClose, onSaved }: { app: DesktopLocalApp; onClose(): void; onSaved(): void }) {
  const [catalog, setCatalog] = useState<LocalAppAgentCatalog>();
  const [settings, setSettings] = useState<LocalAppAgentSettings | undefined>(() => app.agent ? structuredClone(app.agent) : undefined);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let current = true;
    void api().agentCatalog(app.id).then(value => {
      if (!current) return;
      setCatalog(value);
      setSettings(app.agent ? structuredClone(app.agent) : { enabled: false,
        model: value.models.find(model => model.canSelect) ?? { providerId: '', modelId: '' },
        tools: [], background: false, maxTurns: LOCAL_APP_AGENT_DEFAULT_MAX_TURNS, timeoutMs: LOCAL_APP_AGENT_DEFAULT_TIMEOUT_MS });
    }).catch(error => { if (current) { setError(error instanceof Error ? error.message : String(error)); setCatalog({ models: [], tools: [] }); } });
    return () => { current = false; };
  }, [app.id, app.revision]);
  const selectedModel = catalog?.models.find(model => model.providerId === settings?.model.providerId && model.modelId === settings?.model.modelId);
  const modelKey = (value: { providerId: string; modelId: string }) => JSON.stringify([value.providerId, value.modelId]);
  const save = async () => {
    if (!settings || saving) return;
    setSaving(true); setError('');
    try { await api().setAgentSettings(app.id, settings); onSaved(); onClose(); }
    catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { setSaving(false); }
  };
  return <DesktopDialog title={`${app.name} · Agent`} summary="Choose the model and tools this app can use. Each task is separate from your chats." ariaLabel={`Agent settings for ${app.name}`} onEscape={saving ? undefined : onClose}
    footer={<DesktopDialogActions><DesktopDialogButton onClick={onClose} disabled={saving}>Cancel</DesktopDialogButton><DesktopDialogButton variant="primary" onClick={() => void save()} disabled={saving || !settings || (settings.enabled && !selectedModel?.canSelect)}>{saving ? 'Saving…' : 'Save permissions'}</DesktopDialogButton></DesktopDialogActions>}>
    {error && <p role="alert" className="local-app-agent-error">{error}</p>}
    {!settings || !catalog ? <p>Loading available models and tools…</p> : <div className="local-app-agent-settings">
      <label className="local-app-agent-check"><input type="checkbox" checked={settings.enabled} onChange={event => setSettings({ ...settings, enabled: event.target.checked })}/> Allow this app to call an Agent</label>
      <label>Model<select aria-label="Application Agent model" value={modelKey(settings.model)} onChange={event => { const model = catalog.models.find(value => modelKey(value) === event.target.value); if (model) setSettings({ ...settings, model: { providerId: model.providerId, modelId: model.modelId } }); }}>
        {!selectedModel && <option value={modelKey(settings.model)}>Select an available model</option>}
        {catalog.models.map(model => <option key={modelKey(model)} value={modelKey(model)} disabled={!model.canSelect}>{model.label}{model.canSelect ? '' : ' — not configured'}</option>)}
      </select></label>
      {selectedModel && <p className="local-app-agent-hint">{selectedModel.supportsImages ? 'Accepts images' : 'Text only'} · {selectedModel.supportsStructuredOutput ? 'Structured results supported' : 'No structured results'}</p>}
      {!catalog.models.some(model => model.canSelect) && <p className="local-app-agent-hint">Configure a model provider in Settings before enabling Agent access.</p>}
      <fieldset><legend>Requested tools</legend><p className="local-app-agent-hint">Choose each tool you want to allow. Tool actions may still ask for your approval.</p>
        {catalog.tools.map(tool => <label key={tool.id} className="local-app-agent-tool"><input type="checkbox" checked={settings.tools.some(grant => sameTool(grant, tool))} onChange={event => setSettings({ ...settings, tools: event.target.checked ? [...settings.tools.filter(grant => grant.id !== tool.id), tool] : settings.tools.filter(grant => grant.id !== tool.id) })}/><span><strong>{tool.label}</strong><small>{tool.description ?? (tool.kind === 'mcp' ? 'Connected tool' : 'LingXi tool')}</small></span></label>)}
        {!app.manifest.agent?.tools.length && <p className="local-app-agent-hint">This app does not request any tools. It can still use text, images and structured results.</p>}
        {app.manifest.agent?.tools.filter(id => !catalog.tools.some(tool => tool.id === id)).map(id => <p className="local-app-agent-hint" key={id}>Unavailable requested tool: {id}</p>)}
        {settings.tools.some(grant => !catalog.tools.some(tool => sameTool(grant, tool))) && <><p className="local-app-agent-hint">Some previously allowed tools changed or are unavailable. Select their current versions above to renew access, or remove their old permissions.</p><button type="button" onClick={() => setSettings({ ...settings, tools: settings.tools.filter(grant => catalog.tools.some(tool => sameTool(grant, tool))) })}>Remove outdated tool permissions</button></>}
      </fieldset>
      <div className="local-app-agent-limits"><label>Maximum turns<input type="number" min="1" max="10000" value={settings.maxTurns} onChange={event => setSettings({ ...settings, maxTurns: Number(event.target.value) })}/></label><label>Time limit (minutes)<input type="number" min="1" max="1440" value={settings.timeoutMs / 60000} onChange={event => setSettings({ ...settings, timeoutMs: Number(event.target.value) * 60000 })}/></label></div>
      <label className="local-app-agent-check"><input type="checkbox" checked={settings.background} onChange={event => setSettings({ ...settings, background: event.target.checked })}/> Let Agent tasks finish when this app is closed</label>
      <p className="local-app-agent-hint">Closing an app otherwise cancels its Agent task. Updating permissions cancels any task already running.</p>
    </div>}
  </DesktopDialog>;
}

/** Mounted with the library, but its trusted dialog remains visible while another page is selected. */
export function LocalAppAgentQuestions() {
  const [questions, setQuestions] = useState<LocalAppAgentQuestion[]>([]);
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  useEffect(() => {
    if (!window.lingxi?.localApps) return;
    let active = true, generation = 0;
    const refresh = async () => {
      const request = ++generation;
      try { const value = await api().agentQuestions(); if (active && request === generation) setQuestions(value); }
      catch (error) { if (active) setError(error instanceof Error ? error.message : String(error)); }
    };
    void refresh(); const stop = api().onChanged(() => void refresh());
    return () => { active = false; stop(); };
  }, []);
  const current = questions[0];
  if (!current) return null;
  const answer = async (answers: Record<string, string> | null) => {
    if (submitting) return;
    setSubmitting(true); setError('');
    try { await api().answerAgentQuestion(current.id, answers); setQuestions(rows => rows.filter(row => row.id !== current.id)); }
    catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { setSubmitting(false); }
  };
  return <DesktopDialog title={`${current.appName} needs your input`} eyebrow="Application Agent" ariaLabel={`${current.appName} Agent question`} onEscape={() => void answer(null)}>
    {error && <p role="alert" className="local-app-agent-error">{error}</p>}
    <div aria-busy={submitting}><AskUserQuestionPrompt key={current.id} request={current.request} onSubmit={(_id, answers) => void answer(answers)} onCancel={() => void answer(null)}/></div>
  </DesktopDialog>;
}
