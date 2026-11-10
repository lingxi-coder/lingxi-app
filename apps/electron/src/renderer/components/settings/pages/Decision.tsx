import './Decision.css';
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { desktopHost } from '../../../bridge/bridgeConnection';
import { useI18n } from '../../../i18n/useI18n';
import { useT } from '../../../theme/ThemeContext';
import { Card } from '../rows';
import { inputStyle, ghostButtonStyle } from './ghostButton';
import type { DecisionConfiguration, DecisionInput, DecisionQuestion, DecisionResult, DecisionSnapshot } from '../../../../shared/decision';
import type { ProviderAccountMetadata } from '../../../../shared/providerAccounts';
import { assertDecisionImageSelectionBudget, assertDecisionInputBudget } from '../../../../shared/decisionInputBudget';

const defaults: Record<DecisionConfiguration['source'], DecisionConfiguration> = {
  openai: { source: 'openai', protocol: 'openai_decisions', endpoint: 'https://api.openai.com/v1/decisions', model: 'gpt-6-luna' },
  jev: { source: 'jev', protocol: 'typesafe_systemone', endpoint: 'https://api.typesafe.ai/v1/systemone', model: 'jev-1.14.0' },
  custom: { source: 'custom', protocol: 'typesafe_systemone', endpoint: 'http://127.0.0.1:8000/v1/systemone', model: '' },
};
type QuestionDraft = { id: string; type: DecisionQuestion['type']; instructions: string; criteria: string };
export function buildDecisionInput(state: string, json: boolean, questions: QuestionDraft[]): DecisionInput {
  const result: Record<string, DecisionQuestion> = Object.create(null);
  for (const q of questions) {
    if (!q.id.trim() || Object.prototype.hasOwnProperty.call(result, q.id) || !q.instructions.trim()) throw new Error('Questions need unique names and instructions');
    if (q.type === 'binary') result[q.id] = { type: 'binary', instructions: q.instructions };
    else if (q.type === 'score') result[q.id] = { type: 'score', instructions: q.instructions, levels: q.criteria.split('\n').map(v => v.trim()).filter(Boolean) };
    else {
      const options: Record<string, string> = Object.create(null);
      for (const line of q.criteria.split('\n').filter(v => v.trim())) {
        const at = line.indexOf('='); const key = (at < 0 ? line : line.slice(0, at)).trim();
        if (!key || Object.prototype.hasOwnProperty.call(options, key)) throw new Error('Choice IDs must be nonempty and unique');
        options[key] = (at < 0 ? line : line.slice(at + 1)).trim();
      }
      result[q.id] = { type: 'choice', instructions: q.instructions, options };
    }
  }
  return { state: json ? JSON.parse(state) : state, questions: result };
}
export function Decision() {
  const { tr } = useI18n(); const t = useT();
  const [snapshot, setSnapshot] = useState<DecisionSnapshot | null>(null);
  const [revision, setRevision] = useState('initial');
  const [draft, setDraft] = useState<DecisionConfiguration>({ ...defaults.openai });
  const [accounts, setAccounts] = useState<ProviderAccountMetadata[]>([]);
  const [key, setKey] = useState(''); const [clearKey, setClearKey] = useState(false);
  const [saving, setSaving] = useState(false); const [running, setRunning] = useState(false);
  const [error, setError] = useState(''); const [result, setResult] = useState<DecisionResult | null>(null);
  const [images, setImages] = useState<Array<{ name: string; data_url: string }>>([]);
  const [state, setState] = useState(''); const [json, setJson] = useState(false);
  const [questions, setQuestions] = useState<QuestionDraft[]>([{ id: 'answer', type: 'binary', instructions: '', criteria: '' }]);
  const alive = useRef(false); const operation = useRef<string | null>(null); const busy = useRef(false);
  const imageSelection = useRef<AbortController | null>(null);
  const resultJson = useMemo(() => result ? JSON.stringify(result, null, 2) : '', [result]);
  const accept = (next: DecisionSnapshot) => { setSnapshot(next); setRevision(next.revision); setDraft(next.configuration ?? { ...defaults.openai }); setKey(''); setClearKey(false); };
  const reload = async () => {
    try { const next = await desktopHost()?.decision?.snapshot(); if (alive.current && next) { accept(next); setError(''); } }
    catch (e) { if (alive.current) setError(String(e instanceof Error ? e.message : e)); }
  };
  useEffect(() => {
    alive.current = true; const api = desktopHost();
    void reload();
    void api?.providerAccounts?.snapshot().then(next => { if (alive.current) setAccounts(next.accounts); }).catch(() => undefined);
    const off = api?.decision?.onChanged(next => { if (alive.current) { setSnapshot(next); setResult(null); } });
    return () => { alive.current = false; imageSelection.current?.abort(); off?.(); if (operation.current) void api?.decision?.cancel(operation.current); };
  }, []);
  const save = async (disable = false) => {
    const api = desktopHost()?.decision; if (!api || busy.current) return;
    busy.current = true; setSaving(true); setError('');
    try { const next = await api.save(revision, disable ? null : draft, clearKey ? '' : key || undefined); if (alive.current) { accept(next); setResult(null); } }
    catch (e) { if (alive.current) setError(e instanceof Error ? e.message : String(e)); }
    finally { busy.current = false; if (alive.current) setSaving(false); }
  };
  const evaluate = async () => {
    const api = desktopHost()?.decision; if (!api || operation.current) return;
    const id = crypto.randomUUID(); operation.current = id; setRunning(true); setError(''); setResult(null);
    try {
      const input = { ...buildDecisionInput(state, json, questions), images: images.map(({ data_url }) => ({ data_url })) };
      assertDecisionInputBudget(input);
      const next = await api.evaluate(id, input);
      if (alive.current && operation.current === id) setResult(next);
    }
    catch (e) { if (alive.current && operation.current === id) setError(e instanceof Error ? e.message : String(e)); }
    finally { if (operation.current === id) { operation.current = null; if (alive.current) setRunning(false); } }
  };
  const chooseImages = async (files: FileList | null) => {
    imageSelection.current?.abort();
    const selection = new AbortController(); imageSelection.current = selection;
    try {
      const selected = Array.from(files ?? []);
      let input: { state: unknown; questions: unknown };
      try { input = buildDecisionInput(state, json, questions); }
      catch { input = { state, questions }; } // Image selection also works while the form is incomplete.
      assertDecisionImageSelectionBudget(selected, input);
      const next = await Promise.all(selected.map(file => new Promise<{ name: string; data_url: string }>((resolve, reject) => {
        const reader = new FileReader();
        const cleanup = () => { selection.signal.removeEventListener('abort', cancel); reader.onload = null; reader.onerror = null; reader.onabort = null; };
        const cancel = () => { reader.abort(); cleanup(); reject(new Error('Image selection cancelled')); };
        reader.onload = () => { cleanup(); resolve({ name: file.name, data_url: String(reader.result) }); };
        reader.onerror = () => { cleanup(); reject(new Error('Could not read image')); };
        reader.onabort = () => { cleanup(); reject(new Error('Image selection cancelled')); };
        selection.signal.addEventListener('abort', cancel, { once: true });
        if (selection.signal.aborted) { cancel(); return; }
        try { reader.readAsDataURL(file); } catch (error) { cleanup(); reject(error); }
      })));
      if (alive.current && imageSelection.current === selection && !selection.signal.aborted) { setImages(next); setError(''); }
    } catch (e) {
      const report = !selection.signal.aborted;
      selection.abort();
      if (alive.current && imageSelection.current === selection && report) setError(e instanceof Error ? e.message : String(e));
    } finally { if (imageSelection.current === selection) imageSelection.current = null; }
  };
  const clearImages = () => { imageSelection.current?.abort(); imageSelection.current = null; setImages([]); };
  const updateQuestion = (index: number, patch: Partial<QuestionDraft>) => setQuestions(current => current.map((q, i) => i === index ? { ...q, ...patch } : q));
  const field = { ...inputStyle(t), width: '100%', boxSizing: 'border-box' as const };
  const button = ghostButtonStyle(t);
  return <article className="decision-settings" style={{ color: t.text, '--decision-accent': t.accent, '--decision-text': t.text, '--decision-border': t.border, '--decision-surface': t.surface } as CSSProperties}>
    <h1>Decision</h1><p style={{ color: t.text3 }}>{tr('decision_description')}</p>
    <div role="status" aria-live="polite">{error ? <p role="alert" style={{ color: t.danger }}>{error} <button style={button} onClick={() => void reload()}>{tr('decision_reload')}</button></p> : null}
      {snapshot && snapshot.revision !== revision ? <p>{tr('decision_conflict')} <button style={button} onClick={() => void reload()}>{tr('decision_reload')}</button></p> : null}
    </div>
    <Card title={tr('decision_service')}><div style={{ padding: 18, display: 'grid', gap: 14 }}>
      <label>{tr('decision_backend')}<select style={field} value={draft.source} onChange={e => { setDraft({ ...defaults[e.target.value as DecisionConfiguration['source']] }); setKey(''); setClearKey(false); }}>
        <option value="openai">OpenAI Decisions</option><option value="jev">Jev · TypeSafe</option><option value="custom">{tr('decision_custom')}</option>
      </select></label>
      {draft.source === 'custom' ? <label>{tr('decision_service_name')}<input style={field} value={draft.name ?? ''} maxLength={128} onChange={e => setDraft({ ...draft, name: e.target.value })} /></label> : null}
      {draft.source === 'custom' ? <label>{tr('decision_protocol')}<select style={field} value={draft.protocol} onChange={e => setDraft({ ...draft, protocol: e.target.value as DecisionConfiguration['protocol'] })}><option value="openai_decisions">OpenAI Decisions</option><option value="typesafe_systemone">TypeSafe System One</option></select></label> : null}
      <label>{tr('decision_endpoint')}<input style={field} value={draft.endpoint} readOnly={draft.source !== 'custom'} onChange={e => setDraft({ ...draft, endpoint: e.target.value })} /></label>
      <label>{tr('decision_model')}<input style={field} value={draft.model} onChange={e => setDraft({ ...draft, model: e.target.value })} /></label>
      {draft.source === 'openai' ? <label>{tr('decision_account')}<select style={field} value={draft.accountId ?? ''} onChange={e => { setDraft({ ...draft, accountId: e.target.value || undefined }); setKey(''); setClearKey(false); }}><option value="">{tr('decision_separate_key')}</option>{accounts.filter(a => a.providerId === 'openai' && a.executionBindingId === 'openai' && a.authKind === 'api_key' && a.enabled).map(a => <option key={a.accountId} value={a.accountId}>{a.displayName}</option>)}</select></label> : null}
      {!draft.accountId ? <><label>API Key<input style={field} type="password" autoComplete="new-password" value={key} placeholder={snapshot?.credentialConfigured ? tr('decision_key_saved') : tr('decision_key_optional')} onChange={e => { setKey(e.target.value); setClearKey(false); }} /></label><label><input type="checkbox" checked={clearKey} onChange={e => { setClearKey(e.target.checked); setKey(''); }} /> {tr('decision_clear_key')}</label></> : null}
      <div style={{ display: 'flex', gap: 10 }}><button style={button} disabled={saving || !snapshot} onClick={() => void save()}>{tr('decision_save')}</button><button style={button} disabled={saving || !snapshot?.configuration} onClick={() => void save(true)}>{tr('decision_disable')}</button></div>
      <p style={{ color: t.text3, margin: 0 }}>{snapshot?.configuration ? `${snapshot.configuration.name ?? snapshot.configuration.source} · ${snapshot.configuration.model}` : tr('decision_unconfigured')}</p>
    </div></Card>
    <Card title={tr('decision_test')}><div style={{ padding: 18, display: 'grid', gap: 14 }}>
      <p style={{ color: t.text3, margin: 0 }}>{tr('decision_test_hint')}</p>
      <label><input type="checkbox" checked={json} onChange={e => setJson(e.target.checked)} /> JSON</label>
      <label>{tr('decision_input')}<textarea style={{ ...field, minHeight: 100 }} value={state} onChange={e => setState(e.target.value)} /></label>
      <label>{tr('decision_images')}<input type="file" accept="image/png,image/jpeg" multiple onChange={e => void chooseImages(e.target.files)} /></label>
      {images.length ? <div>{images.map(image => <span key={image.name} style={{ marginRight: 8 }}>{image.name}</span>)}<button style={button} onClick={clearImages}>{tr('decision_clear_images')}</button></div> : null}
      {questions.map((q, i) => <fieldset key={i} style={{ border: `1px solid ${t.border}`, borderRadius: 8, display: 'grid', gap: 10, padding: 14 }}>
        <legend>{tr('decision_question')} {i + 1}</legend>
        <label>{tr('decision_name')}<input style={field} value={q.id} onChange={e => updateQuestion(i, { id: e.target.value })} /></label>
        <label>{tr('decision_type')}<select style={field} value={q.type} onChange={e => updateQuestion(i, { type: e.target.value as QuestionDraft['type'] })}><option value="binary">{tr('decision_binary')}</option><option value="choice">{tr('decision_choice')}</option><option value="score">{tr('decision_score')}</option></select></label>
        <label>{tr('decision_instructions')}<textarea style={field} value={q.instructions} onChange={e => updateQuestion(i, { instructions: e.target.value })} /></label>
        {q.type !== 'binary' ? <label>{tr(q.type === 'choice' ? 'decision_choices_hint' : 'decision_levels_hint')}<textarea style={{ ...field, minHeight: 70 }} value={q.criteria} onChange={e => updateQuestion(i, { criteria: e.target.value })} /></label> : null}
        {questions.length > 1 ? <button style={button} onClick={() => setQuestions(current => current.filter((_, n) => n !== i))}>{tr('decision_remove')}</button> : null}
      </fieldset>)}
      <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}><button style={button} disabled={questions.length >= 400} onClick={() => setQuestions(current => [...current, { id: `answer_${current.length + 1}`, type: 'binary', instructions: '', criteria: '' }])}>{tr('decision_add_question')}</button>
        <button style={button} disabled={running || !snapshot?.configuration || saving} onClick={() => void evaluate()}>{running ? tr('decision_running') : tr('decision_run')}</button>
        {running ? <button style={button} onClick={() => { if (operation.current) void desktopHost()?.decision.cancel(operation.current); }}>{tr('decision_cancel')}</button> : null}
      </div>
      {result ? <div aria-live="polite"><p>{result.actual_model ?? tr('decision_not_reported')} · {result.elapsed_ms} ms</p><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 12 }}>{resultJson}</pre></div> : null}
    </div></Card>
  </article>;
}
