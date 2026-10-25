import { tr } from '../../../i18n/locale';
import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import type { HookDto } from '@lingxi/bridge-client';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import {
  DomainOperationBanner,
  EmptyDetail,
  Field,
  JsonRecord,
  actionRowStyle,
  asRecord,
  asString,
  asStringArray,
  managerDetailStyle,
  nextConfigurationOperationId,
  noteStyle,
  parseEventEnvelope,
  prettyJson,
  keyValueTextToRecord,
  recordToKeyValueText,
  searchInputStyle,
  secondaryMetaStyle,
  sidebarButtonStyle,
  sidebarListStyle,
  sidebarSectionTitleStyle,
  SourcePill,
  textareaStyle,
} from './configurationAdmin';

interface HookDocumentEnvelope {
  scope: string;
  revision_sha256: string;
  own_json: string;
  effective_json: string;
}

interface HookEventSummary {
  event: string;
  groupCount: number;
  handlerCount: number;
}

export function hooksPageModel(input: { hooks?: unknown }) {
  const hooks = asRecord(input.hooks);
  return {
    editable: true as const,
    escapeHatch: 'diagnostics' as const,
    events: Object.entries(hooks).map(([event, groups]) => ({
      event,
      count: Array.isArray(groups) ? groups.length : 0,
    })),
  };
}

type HookSelection =
  | { kind: 'all' }
  | { kind: 'event'; event: string }
  | { kind: 'group'; event: string; groupIndex: number }
  | { kind: 'handler'; event: string; groupIndex: number; handlerIndex: number };

const HOOK_EVENTS = [
  'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'PermissionRequest',
  'PermissionDenied', 'UserPromptSubmit', 'UserPromptExpansion', 'Notification',
  'Stop', 'StopFailure', 'SubagentStart', 'SubagentStop', 'SessionStart', 'SessionEnd',
  'Setup', 'PreCompact', 'PostCompact', 'PreModelSwitch', 'PostModelSwitch',
  'FileChanged', 'ConfigChange', 'InstructionsLoaded', 'WorktreeCreate',
  'WorktreeRemove', 'CwdChanged', 'PostToolBatch', 'MessageDisplay', 'DirectoryAdded',
  'Elicitation', 'ElicitationResult', 'TaskCompleted', 'TaskCreated', 'TeammateIdle',
] as const;

const HOOK_TYPES = ['command', 'http', 'agent', 'prompt', 'mcp_tool'] as const;

function selectionEvent(selection: HookSelection): string | null {
  return selection.kind === 'all' ? null : selection.event;
}

function selectionGroup(selection: HookSelection): number | null {
  return selection.kind === 'group' || selection.kind === 'handler' ? selection.groupIndex : null;
}

function hooksDocumentFromDetails(raw: string | null | undefined): HookDocumentEnvelope | null {
  const parsed = parseEventEnvelope<Partial<HookDocumentEnvelope> | null>(raw, null);
  if (!parsed || typeof parsed.scope !== 'string' || typeof parsed.revision_sha256 !== 'string') return null;
  return {
    scope: parsed.scope,
    revision_sha256: parsed.revision_sha256,
    own_json: typeof parsed.own_json === 'string' ? parsed.own_json : '{}',
    effective_json: typeof parsed.effective_json === 'string' ? parsed.effective_json : '{}',
  };
}

function parseObject(text: string): JsonRecord {
  try {
    const parsed = JSON.parse(text) as unknown;
    return asRecord(parsed);
  } catch {
    return {};
  }
}

function hookEvents(hooks: JsonRecord): HookEventSummary[] {
  return Object.entries(hooks)
    .map(([event, groups]) => {
      const list = Array.isArray(groups) ? groups : [];
      const handlerCount = list.reduce((total, item) => {
        const handlers = asRecord(item).hooks;
        return total + (Array.isArray(handlers) ? handlers.length : 0);
      }, 0);
      return { event, groupCount: list.length, handlerCount };
    })
    .sort((left, right) => left.event.localeCompare(right.event));
}

function runtimeSummary(hook: HookDto): string {
  const parts = [
    hook.event,
    hook.matcher ? `${tr('desktop_shell_hook_matcher')}: ${hook.matcher}` : null,
    hook.hook_type ? `${tr('desktop_shell_hooks_execution_type')}: ${hook.hook_type}` : null,
    hook.source ? `${tr('audio_source_title')}: ${hook.source}` : null,
    tr('desktop_shell_hook_timeout_value', hook.timeout_ms),
  ].filter(Boolean);
  return parts.join(' · ');
}

export function Hooks({ bridge, snapshot, editingLayer, onNavigate, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const hookOperation = bridge.configurationOperations?.hook ?? null;
  const [document, setDocument] = useState<HookDocumentEnvelope | null>(null);
  const [selection, setSelection] = useState<HookSelection>({ kind: 'all' });
  const [pendingSelection, setPendingSelection] = useState<HookSelection | null>(null);
  const [search, setSearch] = useState('');
  const [showDetail, setShowDetail] = useState(false);
  const [newEvent, setNewEvent] = useState<(typeof HOOK_EVENTS)[number]>('PreToolUse');
  const [draftText, setDraftText] = useState('{}');
  const [pageError, setPageError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const next = hooksDocumentFromDetails(hookOperation?.details_json);
    if (next) setDocument(next);
  }, [hookOperation?.details_json]);

  useEffect(() => {
    setLoading(true);
    void bridge.hookAdmin({ action: 'get_document', scope: editingLayer })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_shell_hooks_read_error')))
      .finally(() => setLoading(false));
  }, [bridge.hookAdmin, editingLayer]);

  const ownHooks = useMemo(() => parseObject(document?.own_json ?? '{}'), [document?.own_json]);
  const effectiveHooks = useMemo(() => parseObject(document?.effective_json ?? '{}'), [document?.effective_json]);
  const draftHooks = useMemo(() => parseObject(draftText), [draftText]);
  const availableEvents = HOOK_EVENTS.filter((event) => !(event in draftHooks));
  const eventSummaries = useMemo(() => hookEvents(draftHooks), [draftHooks]);
  const visibleQuery = search.trim().toLowerCase();
  const filteredEvents = visibleQuery
    ? eventSummaries.filter((entry) => entry.event.toLowerCase().includes(visibleQuery))
    : eventSummaries;
  const runtimeHooks = [...bridge.hooksCatalog].sort((left, right) => left.event.localeCompare(right.event) || left.name.localeCompare(right.name));
  const selectedEvent = selectionEvent(selection);
  const selectedGroupIndex = selectionGroup(selection);
  const selectedGroups = selectedEvent && Array.isArray(draftHooks[selectedEvent])
    ? draftHooks[selectedEvent] as JsonRecord[]
    : [];
  const selectedGroup = selectedGroupIndex === null ? null : asRecord(selectedGroups[selectedGroupIndex]);
  const selectedHandlers = selectedGroup && Array.isArray(selectedGroup.hooks)
    ? selectedGroup.hooks.map(asRecord)
    : [];
  const selectedHandler = selection.kind === 'handler'
    ? selectedHandlers[selection.handlerIndex] ?? null
    : null;

  useEffect(() => {
    setDraftText(prettyJson(ownHooks));
  }, [ownHooks]);

  useEffect(() => {
    if (selection.kind !== 'all' && !(selection.event in draftHooks)) {
      setSelection({ kind: 'all' });
    }
  }, [draftHooks, selection]);

  useEffect(() => {
    if (availableEvents.length > 0 && !availableEvents.includes(newEvent)) {
      setNewEvent(availableEvents[0]);
    }
  }, [availableEvents, newEvent]);

  const dirty = draftText !== prettyJson(ownHooks);

  const mutateDraft = (mutation: (next: JsonRecord) => void) => {
    setDraftText((current) => {
      let next: JsonRecord;
      try {
        const parsed = JSON.parse(current) as unknown;
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
          setPageError(tr('desktop_shell_hooks_object'));
          return current;
        }
        next = parsed as JsonRecord;
      } catch {
        setPageError(tr('desktop_shell_hooks_fix_json'));
        return current;
      }
      mutation(next);
      setPageError(null);
      return prettyJson(next);
    });
  };

  const updateGroup = (event: string, groupIndex: number, key: string, value: unknown) => {
    mutateDraft((next) => {
      const groups = Array.isArray(next[event]) ? next[event] as JsonRecord[] : [];
      const group = asRecord(groups[groupIndex]);
      if (value === undefined || value === '') delete group[key];
      else group[key] = value;
      groups[groupIndex] = group;
      next[event] = groups;
    });
  };

  const updateHandler = (event: string, groupIndex: number, handlerIndex: number, key: string, value: unknown) => {
    mutateDraft((next) => {
      const groups = Array.isArray(next[event]) ? next[event] as JsonRecord[] : [];
      const group = asRecord(groups[groupIndex]);
      const handlers = Array.isArray(group.hooks) ? group.hooks.map(asRecord) : [];
      const handler = asRecord(handlers[handlerIndex]);
      if (value === undefined || value === '') delete handler[key];
      else handler[key] = value;
      handlers[handlerIndex] = handler;
      group.hooks = handlers;
      groups[groupIndex] = group;
      next[event] = groups;
    });
  };

  const requestSelection = (next: HookSelection) => {
    if (dirty) {
      setPendingSelection(next);
      setShowDetail(true);
      return;
    }
    setPendingSelection(null);
    setSelection(next);
    setShowDetail(true);
  };

  const discardDraft = () => {
    setDraftText(prettyJson(ownHooks));
    setPendingSelection(null);
  };

  const validateHooks = () => {
    let hooks: unknown;
    try {
      hooks = JSON.parse(draftText);
    } catch (cause) {
      setPageError(cause instanceof Error ? tr('desktop_shell_hooks_invalid_detail', cause.message) : tr('desktop_shell_hooks_invalid_json'));
      return;
    }
    setPageError(null);
    setLoading(true);
    void bridge.hookAdmin({
      action: 'validate_document',
      operation_id: nextConfigurationOperationId(),
      payload_json: JSON.stringify({ scope: editingLayer, hooks }),
    })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_shell_hooks_validate_error')))
      .finally(() => setLoading(false));
  };

  const saveHooks = () => {
    if (!document) return;
    let hooks: unknown;
    try {
      hooks = JSON.parse(draftText);
    } catch (cause) {
      setPageError(cause instanceof Error ? tr('desktop_shell_hooks_invalid_detail', cause.message) : tr('desktop_shell_hooks_invalid_json'));
      return;
    }
    setPageError(null);
    setLoading(true);
    void bridge.hookAdmin({
      action: 'save_document',
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      revision: document.revision_sha256,
      payload_json: JSON.stringify({ scope: editingLayer, hooks }),
    })
      .then(() => bridge.hookAdmin({ action: 'get_document', scope: editingLayer }))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_shell_hooks_save_error')))
      .finally(() => setLoading(false));
  };

  const detail = !document
    ? <EmptyDetail t={t} title={tr('desktop_shell_hooks_unloaded')} body={tr('desktop_shell_hooks_unloaded_desc')} />
    : (
      <div className="hooks-editor-body">
        <div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
            <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>
              {selection.kind === 'all' ? tr('desktop_shell_hooks_current') : selection.event}
            </div>
            <SourcePill t={t} label={editingLayer} />
          </div>
          <div style={secondaryMetaStyle(t)}>
            {tr('desktop_shell_hooks_layer_desc')}
          </div>
        </div>

        {selection.kind === 'event' && <div className="hooks-child-list">
          {selectedGroups.map((group, groupIndex) => <button type="button" className="configuration-entry" key={groupIndex} onClick={() => requestSelection({ kind: 'group', event: selection.event, groupIndex })} style={sidebarButtonStyle(t, false)}>
            <strong>{tr('desktop_shell_hook_group', groupIndex + 1)}</strong><span>{asString(group.matcher, '*')} · {tr('desktop_shell_hook_handlers', Array.isArray(group.hooks) ? group.hooks.length : 0)}</span>
          </button>)}
        </div>}
        {selection.kind === 'group' && <div className="hooks-child-list">
          {selectedHandlers.map((handler, handlerIndex) => <button type="button" className="configuration-entry" key={handlerIndex} onClick={() => requestSelection({ kind: 'handler', event: selection.event, groupIndex: selection.groupIndex, handlerIndex })} style={sidebarButtonStyle(t, false)}>
            <strong>{tr('desktop_shell_hook_handler', handlerIndex + 1)}</strong><span>{asString(handler.type, 'command')}</span>
          </button>)}
        </div>}

        {selection.kind === 'event' && (
          <div style={noteStyle(t)}>
            <div style={{ marginBottom: 8 }}>{tr('desktop_shell_hooks_matcher_groups', selectedGroups.length)}</div>
            <div style={actionRowStyle()}>
              <button type="button" onClick={() => {
                let groupIndex = 0;
                mutateDraft((next) => {
                  const groups = Array.isArray(next[selection.event]) ? next[selection.event] as JsonRecord[] : [];
                  groupIndex = groups.length;
                  groups.push({ hooks: [{ type: 'command', command: '' }] });
                  next[selection.event] = groups;
                });
                setSelection({ kind: 'handler', event: selection.event, groupIndex, handlerIndex: 0 });
              }} style={ghostButtonStyle(t)}>{tr('desktop_shell_hooks_add_group')}</button>
              <button type="button" onClick={() => {
                mutateDraft((next) => { delete next[selection.event]; });
                setSelection({ kind: 'all' });
              }} style={ghostButtonStyle(t, false, true)}>{tr('desktop_shell_hooks_delete_event')}</button>
            </div>
          </div>
        )}

        {selection.kind === 'group' && selectedGroup && (
          <div style={{ display: 'grid', gap: 14 }}>
            <Field t={t} label={tr('desktop_shell_hook_matcher')}>
              <input value={asString(selectedGroup.matcher)} onChange={(event) => updateGroup(selection.event, selection.groupIndex, 'matcher', event.target.value)} placeholder={tr('desktop_shell_hooks_matcher_placeholder')} aria-label={tr('desktop_shell_hook_matcher')} style={{ ...inputStyle(t), width: '100%' }} />
            </Field>
            <div style={noteStyle(t)}>{tr('desktop_shell_hooks_handlers', selectedHandlers.length)}</div>
            <div style={actionRowStyle()}>
              <button type="button" onClick={() => {
                let handlerIndex = 0;
                mutateDraft((next) => {
                  const groups = Array.isArray(next[selection.event]) ? next[selection.event] as JsonRecord[] : [];
                  const group = asRecord(groups[selection.groupIndex]);
                  const handlers = Array.isArray(group.hooks) ? group.hooks.map(asRecord) : [];
                  handlerIndex = handlers.length;
                  handlers.push({ type: 'command', command: '' });
                  group.hooks = handlers;
                  groups[selection.groupIndex] = group;
                  next[selection.event] = groups;
                });
                setSelection({ kind: 'handler', event: selection.event, groupIndex: selection.groupIndex, handlerIndex });
              }} style={ghostButtonStyle(t)}>{tr('desktop_shell_hooks_add_handler')}</button>
              <button type="button" onClick={() => {
                mutateDraft((next) => {
                  const groups = Array.isArray(next[selection.event]) ? next[selection.event] as JsonRecord[] : [];
                  groups.splice(selection.groupIndex, 1);
                  if (groups.length === 0) delete next[selection.event];
                  else next[selection.event] = groups;
                });
                setSelection({ kind: 'event', event: selection.event });
              }} style={ghostButtonStyle(t, false, true)}>{tr('desktop_shell_hooks_delete_group')}</button>
            </div>
          </div>
        )}

        {selection.kind === 'handler' && selectedHandler && (
          <div style={{ display: 'grid', gap: 14 }}>
            <div className="hooks-field-grid">
              <Field t={t} label={tr('desktop_shell_hooks_execution_type')}>
                <select value={asString(selectedHandler.type, 'command')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'type', event.target.value)} aria-label={tr('desktop_shell_hooks_execution_type')} style={{ ...inputStyle(t), width: '100%' }}>
                  {HOOK_TYPES.map((type) => <option key={type} value={type}>{type}</option>)}
                </select>
              </Field>
              <Field t={t} label={tr('desktop_priority')}>
                <input type="number" value={typeof selectedHandler.priority === 'number' ? selectedHandler.priority : 0} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'priority', Number(event.target.value))} aria-label={tr('desktop_priority')} style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
              <Field t={t} label={tr('desktop_shell_hooks_timeout')}>
                <input type="number" min={0} value={typeof selectedHandler.timeout === 'number' ? selectedHandler.timeout : ''} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'timeout', event.target.value ? Number(event.target.value) : undefined)} aria-label={tr('desktop_shell_hooks_timeout')} style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
              <Field t={t} label={tr('desktop_shell_hook_status_message')}>
                <input value={asString(selectedHandler.statusMessage)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'statusMessage', event.target.value)} aria-label={tr('desktop_shell_hook_status_message')} style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
            </div>
            <Field t={t} label={tr('desktop_shell_hooks_condition')}>
              <input value={asString(selectedHandler.if)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'if', event.target.value)} placeholder="Bash(git push:*)" aria-label={tr('desktop_shell_hooks_condition')} style={{ ...inputStyle(t), width: '100%' }} />
            </Field>

            {asString(selectedHandler.type, 'command') === 'command' && (
              <div className="hooks-field-grid">
                <Field t={t} label={tr('desktop_shell_hook_command')}>
                  <input value={asString(selectedHandler.command)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'command', event.target.value)} aria-label={tr('desktop_shell_hook_command')} style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <Field t={t} label={tr('desktop_shell_hook_shell')}>
                  <select value={asString(selectedHandler.shell)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'shell', event.target.value || undefined)} aria-label={tr('desktop_shell_hook_shell')} style={{ ...inputStyle(t), width: '100%' }}>
                    <option value="">{tr('settings_auto')}</option><option value="bash">bash</option><option value="powershell">powershell</option>
                  </select>
                </Field>
                <Field t={t} label={tr('desktop_shell_hooks_args')}>
                  <textarea value={asStringArray(selectedHandler.args).join('\n')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'args', event.target.value.split('\n').filter(Boolean))} rows={4} aria-label={tr('desktop_shell_hooks_args')} style={textareaStyle(t, 4)} />
                </Field>
              </div>
            )}

            {asString(selectedHandler.type) === 'http' && (
              <div style={{ display: 'grid', gap: 12 }}>
                <Field t={t} label={tr('desktop_shell_hook_url')}>
                  <input value={asString(selectedHandler.url)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'url', event.target.value)} aria-label={tr('desktop_shell_hook_url')} style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <div className="hooks-field-grid">
                  <Field t={t} label={tr('desktop_shell_hook_headers')}>
                    <textarea value={recordToKeyValueText(selectedHandler.headers)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'headers', keyValueTextToRecord(event.target.value))} rows={5} aria-label={tr('desktop_shell_hook_headers')} style={textareaStyle(t, 5)} />
                  </Field>
                  <Field t={t} label={tr('desktop_shell_hooks_env')}>
                    <textarea value={asStringArray(selectedHandler.allowedEnvVars).join('\n')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'allowedEnvVars', event.target.value.split('\n').filter(Boolean))} rows={5} aria-label={tr('desktop_shell_hooks_env')} style={textareaStyle(t, 5)} />
                  </Field>
                </div>
              </div>
            )}

            {(asString(selectedHandler.type) === 'agent' || asString(selectedHandler.type) === 'prompt') && (
              <div style={{ display: 'grid', gap: 12 }}>
                <Field t={t} label={tr('desktop_shell_hook_prompt')}>
                  <textarea value={asString(selectedHandler.prompt)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'prompt', event.target.value)} rows={6} aria-label={tr('desktop_shell_hook_prompt')} style={textareaStyle(t, 6)} />
                </Field>
                <Field t={t} label={tr('desktop_shell_hook_model_override')}>
                  <input value={asString(selectedHandler.model)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'model', event.target.value)} aria-label={tr('desktop_shell_hook_model_override')} style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
              </div>
            )}

            {asString(selectedHandler.type) === 'mcp_tool' && (
              <div style={{ display: 'grid', gap: 12 }}>
                <div className="hooks-field-grid">
                  <Field t={t} label={tr('settings_mcp_servers')}>
                    <input value={asString(selectedHandler.server)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'server', event.target.value)} aria-label={tr('settings_mcp_servers')} style={{ ...inputStyle(t), width: '100%' }} />
                  </Field>
                  <Field t={t} label={tr('desktop_shell_hook_tool')}>
                    <input value={asString(selectedHandler.tool)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'tool', event.target.value)} aria-label={tr('desktop_shell_hook_tool')} style={{ ...inputStyle(t), width: '100%' }} />
                  </Field>
                </div>
                <Field t={t} label={tr('desktop_shell_hooks_input_json')}>
                  <textarea key={prettyJson(selectedHandler.input)} defaultValue={prettyJson(asRecord(selectedHandler.input))} onBlur={(event) => {
                    try {
                      const value = JSON.parse(event.target.value) as unknown;
                      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(tr('desktop_shell_hooks_input_object'));
                      updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'input', value);
                    } catch (cause) { setPageError(cause instanceof Error ? cause.message : tr('desktop_shell_hooks_input_invalid')); }
                  }} rows={7} aria-label={tr('desktop_shell_hooks_input_json')} style={textareaStyle(t, 7)} />
                </Field>
              </div>
            )}

            <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
              {(['once', 'async', 'asyncRewake', 'continueOnBlock'] as const).map((key) => (
                <label key={key} style={{ display: 'inline-flex', gap: 7, alignItems: 'center', color: t.text2, fontSize: 12.5 }}>
                  <input type="checkbox" checked={selectedHandler[key] === true} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, key, event.target.checked)} />{tr(({ async: 'desktop_shell_hook_async', once: 'desktop_shell_hook_once', suppressOutput: 'desktop_shell_hook_suppress' } as Record<string, string>)[key] ?? key)}
                </label>
              ))}
            </div>
            {selectedHandler.async === true && (
              <div className="hooks-field-grid">
                <Field t={t} label={tr('desktop_shell_hooks_async_timeout')}>
                  <input type="number" min={0} value={typeof selectedHandler.asyncTimeout === 'number' ? selectedHandler.asyncTimeout : ''} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'asyncTimeout', event.target.value ? Number(event.target.value) : undefined)} aria-label={tr('desktop_shell_hooks_async_timeout')} style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <Field t={t} label={tr('desktop_shell_hook_rewake')}>
                  <input value={asString(selectedHandler.rewakeMessage)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'rewakeMessage', event.target.value)} aria-label={tr('desktop_shell_hook_rewake')} style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
              </div>
            )}
            <button type="button" onClick={() => {
              mutateDraft((next) => {
                const groups = Array.isArray(next[selection.event]) ? next[selection.event] as JsonRecord[] : [];
                const group = asRecord(groups[selection.groupIndex]);
                const handlers = Array.isArray(group.hooks) ? group.hooks.map(asRecord) : [];
                handlers.splice(selection.handlerIndex, 1);
                group.hooks = handlers;
                groups[selection.groupIndex] = group;
                next[selection.event] = groups;
              });
              setSelection({ kind: 'group', event: selection.event, groupIndex: selection.groupIndex });
            }} style={ghostButtonStyle(t, false, true)}>{tr('desktop_shell_hooks_delete_handler')}</button>
          </div>
        )}

        <details className="hooks-json-panel" open={selection.kind === 'all'}>
          <summary style={{ cursor: 'pointer', color: t.text2, fontSize: 13, fontWeight: 600 }}>{tr('desktop_shell_hooks_json_preview')}</summary>
          <Field t={t} label={tr('desktop_shell_hooks_layer_json')}>
            <textarea value={draftText} onChange={(event) => setDraftText(event.target.value)} rows={18} aria-label={tr('desktop_shell_hooks_layer_json')} style={textareaStyle(t, 18)} />
          </Field>
        </details>

        <div className="hooks-actions">
          <button type="button" disabled={loading} onClick={validateHooks} style={ghostButtonStyle(t, loading)}>
            {tr('desktop_shell_preflight')}
          </button>
          <button type="button" disabled={loading || !dirty || !document.revision_sha256} onClick={saveHooks} className="hooks-save" style={ghostButtonStyle(t, loading || !dirty || !document.revision_sha256)}>
            {tr('settings_save')}
          </button>
          <button type="button" disabled={loading || !dirty} onClick={discardDraft} style={ghostButtonStyle(t, loading || !dirty, true)}>
            {tr('common_cancel')}
          </button>
          <button type="button" data-testid="hooks-open-settings-files" onClick={() => onNavigate(hooksPageModel({}).escapeHatch)} style={ghostButtonStyle(t)}>
            {tr('desktop_shell_view_files')}
          </button>
        </div>

        <div style={noteStyle(t)}>
          {tr('desktop_shell_hooks_merged')}
          <pre style={{ margin: '8px 0 0', whiteSpace: 'pre-wrap', overflowX: 'auto' }}>{prettyJson(effectiveHooks)}</pre>
        </div>
      </div>
    );

  return (
    <>
      <Card title={tr('settings_parity_hooks')}>
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="hooks" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} label={tr('desktop_shell_hooks_layer_effective')} />
        <div className="configuration-page hooks-page" style={{ '--hooks-border': t.border, '--hooks-text': t.text, '--hooks-muted': t.text3, '--hooks-surface': t.surface } as CSSProperties}>
          {!showDetail ? <div className="configuration-list">
            <div style={{ display: 'grid', gap: 10 }}>
              <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder={tr('desktop_shell_hooks_search_event')} aria-label={tr('desktop_shell_hooks_search')} style={searchInputStyle(t)} />
              <div style={noteStyle(t)}>
                {tr('desktop_shell_hooks_edit_desc')}
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <select value={newEvent} onChange={(event) => setNewEvent(event.target.value as (typeof HOOK_EVENTS)[number])} aria-label={tr('desktop_shell_hooks_new_event')} style={{ ...inputStyle(t), minWidth: 0, flex: 1 }}>
                  {availableEvents.map((event) => <option key={event} value={event}>{event}</option>)}
                </select>
                <button type="button" disabled={availableEvents.length === 0} onClick={() => {
                  mutateDraft((next) => {
                    if (!Array.isArray(next[newEvent])) next[newEvent] = [{ hooks: [{ type: 'command', command: '' }] }];
                  });
                  setSelection({ kind: 'handler', event: newEvent, groupIndex: 0, handlerIndex: 0 });
                  setShowDetail(true);
                }} style={ghostButtonStyle(t, availableEvents.length === 0)}>{tr('workspace_confirm_add')}</button>
              </div>
            </div>
            <button type="button" data-testid="hooks-open-settings-files" onClick={() => onNavigate(hooksPageModel({}).escapeHatch)} style={{ ...ghostButtonStyle(t), justifySelf: 'start' }}>{tr('desktop_shell_view_files')}</button>
            <div style={sidebarListStyle()}>
              <div style={sidebarSectionTitleStyle(t)}>{tr('desktop_shell_hooks_layer_events')}</div>
              <button type="button" onClick={() => requestSelection({ kind: 'all' })} style={sidebarButtonStyle(t, selection.kind === 'all')}>
                <span style={{ fontSize: 13, fontWeight: 600 }}>{tr('desktop_shell_hooks_all')}</span>
                <span style={{ fontSize: 11.5, color: t.text4 }}>{tr('desktop_shell_hooks_event_count', eventSummaries.length)}</span>
              </button>
              {filteredEvents.map((entry) => {
                return (
                  <div key={entry.event} style={{ display: 'grid', gap: 6 }}>
                    <button type="button" className="configuration-entry" onClick={() => requestSelection({ kind: 'event', event: entry.event })} style={sidebarButtonStyle(t, selection.kind === 'event' && selection.event === entry.event)}>
                      <span style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap' }}>
                        <span style={{ fontSize: 13, fontWeight: 600 }}>{entry.event}</span>
                        <SourcePill t={t} label={tr('desktop_shell_hook_groups', entry.groupCount)} />
                        <SourcePill t={t} label={tr('desktop_shell_hook_handlers', entry.handlerCount)} />
                      </span>
                    </button>

                  </div>
                );
              })}
              {filteredEvents.length === 0 && <div style={secondaryMetaStyle(t)}>{tr('desktop_shell_hooks_no_match')}</div>}
            </div>
          </div> : <div className="configuration-detail hooks-editor" style={managerDetailStyle()}>
            <button type="button" className="configuration-back" onClick={() => setShowDetail(false)} style={ghostButtonStyle(t)}>{tr('desktop_shell_hooks_back')}</button>
            <DomainOperationBanner t={t} operation={hookOperation} fallbackDomainLabel={tr('settings_parity_hooks')} />
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                {tr('desktop_shell_hooks_unsaved')}
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { discardDraft(); setSelection(pendingSelection); setShowDetail(true); }} style={ghostButtonStyle(t)}>
                    {tr('desktop_shell_discard_switch')}
                  </button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>
                    {tr('desktop_shell_continue_editing')}
                  </button>
                </div>
              </div>
            )}
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {loading && <div style={noteStyle(t)}>{tr('desktop_shell_hooks_syncing')}</div>}
            {detail}
          </div>}
        </div>
      </Card>

      {!showDetail && <Card title={tr('desktop_shell_runtime_status')}>
        <Row title={tr('desktop_shell_refresh_catalog')} desc={tr('desktop_shell_hooks_runtime_desc')} align="center">
          <button type="button" onClick={() => void bridge.refreshHooks()} style={ghostButtonStyle(t)}>{tr('settings_linux_refresh_button')}</button>
        </Row>
        {runtimeHooks.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_shell_hooks_runtime_empty')}</div>
        )}
        {runtimeHooks.map((hook) => (
          <Row key={`${hook.event}:${hook.name}:${hook.matcher ?? '*'}`} title={hook.name} desc={runtimeSummary(hook)} align="center">
            {hook.blocking ? <SourcePill t={t} label={tr('desktop_shell_hook_blocking')} tone="warn" /> : null}
          </Row>
        ))}
      </Card>}
    </>
  );
}
