import { useEffect, useMemo, useState } from 'react';
import type { HookAdminCommandDto, HookDto } from '@lingxi/bridge-client';
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
  detailGridStyle,
  managerDetailStyle,
  managerShellStyle,
  managerSidebarStyle,
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

function callHookAdmin(bridge: PageContentProps['bridge'], command: HookAdminCommandDto | Record<string, unknown>) {
  const admin = (bridge as { hookAdmin?: (payload: unknown) => Promise<unknown> }).hookAdmin;
  return typeof admin === 'function' ? admin(command) : Promise.resolve();
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
    hook.matcher ? `matcher ${hook.matcher}` : null,
    hook.hook_type ? `type ${hook.hook_type}` : null,
    hook.source ? `source ${hook.source}` : null,
    `timeout ${hook.timeout_ms} ms`,
  ].filter(Boolean);
  return parts.join(' · ');
}

export function Hooks({ bridge, snapshot, editingLayer, onNavigate, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const adminAvailable = typeof (bridge as { hookAdmin?: unknown }).hookAdmin === 'function';
  const hookOperation = bridge.configurationOperations?.hook ?? null;
  const [document, setDocument] = useState<HookDocumentEnvelope | null>(null);
  const [selection, setSelection] = useState<HookSelection>({ kind: 'all' });
  const [pendingSelection, setPendingSelection] = useState<HookSelection | null>(null);
  const [search, setSearch] = useState('');
  const [newEvent, setNewEvent] = useState<(typeof HOOK_EVENTS)[number]>('PreToolUse');
  const [draftText, setDraftText] = useState('{}');
  const [pageError, setPageError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const next = hooksDocumentFromDetails(hookOperation?.details_json);
    if (next) setDocument(next);
  }, [hookOperation?.details_json]);

  useEffect(() => {
    if (!adminAvailable) {
      const own = asRecord(snapshot?.layers?.[editingLayer]?.hooks);
      const effective = asRecord(snapshot?.effective?.hooks);
      setDocument({
        scope: editingLayer,
        revision_sha256: '',
        own_json: prettyJson(own),
        effective_json: prettyJson(effective),
      });
      return;
    }
    setLoading(true);
    void callHookAdmin(bridge, { action: 'get_document', scope: editingLayer })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法读取 hooks 文档。'))
      .finally(() => setLoading(false));
  }, [adminAvailable, bridge.hookAdmin, editingLayer]);

  const ownObject = useMemo(() => parseObject(document?.own_json ?? '{}'), [document?.own_json]);
  const ownHooks = useMemo(() => {
    const legacyNested = asRecord(ownObject.hooks);
    return Object.keys(legacyNested).length > 0 ? legacyNested : ownObject;
  }, [ownObject]);
  const effectiveObject = useMemo(() => parseObject(document?.effective_json ?? '{}'), [document?.effective_json]);
  const effectiveHooks = useMemo(() => {
    const legacyNested = asRecord(effectiveObject.hooks);
    return Object.keys(legacyNested).length > 0 ? legacyNested : effectiveObject;
  }, [effectiveObject]);
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
          setPageError('Hooks JSON 必须是对象。');
          return current;
        }
        next = parsed as JsonRecord;
      } catch {
        setPageError('请先修复高级 JSON 中的语法错误。');
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
      return;
    }
    setPendingSelection(null);
    setSelection(next);
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
      setPageError(cause instanceof Error ? `Hooks JSON 不是合法的 JSON：${cause.message}` : 'Hooks JSON 不是合法的 JSON。');
      return;
    }
    setPageError(null);
    setLoading(true);
    void callHookAdmin(bridge, {
      action: 'validate_document',
      operation_id: nextConfigurationOperationId(),
      payload_json: JSON.stringify({ scope: editingLayer, hooks }),
    })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法校验 hooks。'))
      .finally(() => setLoading(false));
  };

  const saveHooks = () => {
    if (!document) return;
    let hooks: unknown;
    try {
      hooks = JSON.parse(draftText);
    } catch (cause) {
      setPageError(cause instanceof Error ? `Hooks JSON 不是合法的 JSON：${cause.message}` : 'Hooks JSON 不是合法的 JSON。');
      return;
    }
    setPageError(null);
    setLoading(true);
    void callHookAdmin(bridge, {
      action: 'save_document',
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      revision: document.revision_sha256,
      payload_json: JSON.stringify({ scope: editingLayer, hooks }),
    })
      .then(() => callHookAdmin(bridge, { action: 'get_document', scope: editingLayer }))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存 hooks。'))
      .finally(() => setLoading(false));
  };

  const detail = !document
    ? <EmptyDetail t={t} title="尚未加载 hooks 文档" body="连接到引擎后，这里会显示当前层自己的 hooks 对象与生效后的合并结果。" />
    : (
      <div style={managerDetailStyle()}>
        <div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
            <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>
              {selection.kind === 'all' ? '当前层 hooks' : selection.event}
            </div>
            <SourcePill t={t} label={editingLayer} />
          </div>
          <div style={secondaryMetaStyle(t)}>
            当前编辑的是当前层自己的 <code className="mono">hooks</code> 对象；不会把 effective 合并结果回写到该层。
          </div>
        </div>

        {selection.kind === 'event' && (
          <div style={noteStyle(t)}>
            <div style={{ marginBottom: 8 }}>此事件包含 {selectedGroups.length} 个 matcher group。</div>
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
              }} style={ghostButtonStyle(t)}>添加 matcher group</button>
              <button type="button" onClick={() => {
                mutateDraft((next) => { delete next[selection.event]; });
                setSelection({ kind: 'all' });
              }} style={ghostButtonStyle(t, false, true)}>删除事件</button>
            </div>
          </div>
        )}

        {selection.kind === 'group' && selectedGroup && (
          <div style={{ display: 'grid', gap: 14 }}>
            <Field t={t} label="Matcher">
              <input value={asString(selectedGroup.matcher)} onChange={(event) => updateGroup(selection.event, selection.groupIndex, 'matcher', event.target.value)} placeholder="* 或 Bash|Write 或正则表达式" aria-label="hook matcher" style={{ ...inputStyle(t), width: '100%' }} />
            </Field>
            <div style={noteStyle(t)}>此 group 包含 {selectedHandlers.length} 个 handler。空 matcher 等同于匹配全部。</div>
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
              }} style={ghostButtonStyle(t)}>添加 handler</button>
              <button type="button" onClick={() => {
                mutateDraft((next) => {
                  const groups = Array.isArray(next[selection.event]) ? next[selection.event] as JsonRecord[] : [];
                  groups.splice(selection.groupIndex, 1);
                  if (groups.length === 0) delete next[selection.event];
                  else next[selection.event] = groups;
                });
                setSelection({ kind: 'event', event: selection.event });
              }} style={ghostButtonStyle(t, false, true)}>删除 group</button>
            </div>
          </div>
        )}

        {selection.kind === 'handler' && selectedHandler && (
          <div style={{ display: 'grid', gap: 14 }}>
            <div style={detailGridStyle()}>
              <Field t={t} label="执行类型">
                <select value={asString(selectedHandler.type, 'command')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'type', event.target.value)} aria-label="hook type" style={{ ...inputStyle(t), width: '100%' }}>
                  {HOOK_TYPES.map((type) => <option key={type} value={type}>{type}</option>)}
                </select>
              </Field>
              <Field t={t} label="Priority">
                <input type="number" value={typeof selectedHandler.priority === 'number' ? selectedHandler.priority : 0} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'priority', Number(event.target.value))} aria-label="hook priority" style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
              <Field t={t} label="Timeout（秒）">
                <input type="number" min={0} value={typeof selectedHandler.timeout === 'number' ? selectedHandler.timeout : ''} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'timeout', event.target.value ? Number(event.target.value) : undefined)} aria-label="hook timeout" style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
              <Field t={t} label="Status message">
                <input value={asString(selectedHandler.statusMessage)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'statusMessage', event.target.value)} aria-label="hook status message" style={{ ...inputStyle(t), width: '100%' }} />
              </Field>
            </div>
            <Field t={t} label="if 条件">
              <input value={asString(selectedHandler.if)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'if', event.target.value)} placeholder="Bash(git push:*)" aria-label="hook if condition" style={{ ...inputStyle(t), width: '100%' }} />
            </Field>

            {asString(selectedHandler.type, 'command') === 'command' && (
              <div style={detailGridStyle()}>
                <Field t={t} label="Command">
                  <input value={asString(selectedHandler.command)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'command', event.target.value)} aria-label="hook command" style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <Field t={t} label="Shell">
                  <select value={asString(selectedHandler.shell)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'shell', event.target.value || undefined)} aria-label="hook shell" style={{ ...inputStyle(t), width: '100%' }}>
                    <option value="">自动</option><option value="bash">bash</option><option value="powershell">powershell</option>
                  </select>
                </Field>
                <Field t={t} label="Args（每行一个）">
                  <textarea value={asStringArray(selectedHandler.args).join('\n')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'args', event.target.value.split('\n').filter(Boolean))} rows={4} aria-label="hook args" style={textareaStyle(t, 4)} />
                </Field>
              </div>
            )}

            {asString(selectedHandler.type) === 'http' && (
              <div style={{ display: 'grid', gap: 12 }}>
                <Field t={t} label="URL">
                  <input value={asString(selectedHandler.url)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'url', event.target.value)} aria-label="hook URL" style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <div style={detailGridStyle()}>
                  <Field t={t} label="Headers（KEY=VALUE）">
                    <textarea value={recordToKeyValueText(selectedHandler.headers)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'headers', keyValueTextToRecord(event.target.value))} rows={5} aria-label="hook headers" style={textareaStyle(t, 5)} />
                  </Field>
                  <Field t={t} label="Allowed env vars（每行一个）">
                    <textarea value={asStringArray(selectedHandler.allowedEnvVars).join('\n')} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'allowedEnvVars', event.target.value.split('\n').filter(Boolean))} rows={5} aria-label="hook allowed env vars" style={textareaStyle(t, 5)} />
                  </Field>
                </div>
              </div>
            )}

            {(asString(selectedHandler.type) === 'agent' || asString(selectedHandler.type) === 'prompt') && (
              <div style={{ display: 'grid', gap: 12 }}>
                <Field t={t} label="Prompt">
                  <textarea value={asString(selectedHandler.prompt)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'prompt', event.target.value)} rows={6} aria-label="hook prompt" style={textareaStyle(t, 6)} />
                </Field>
                <Field t={t} label="Model override">
                  <input value={asString(selectedHandler.model)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'model', event.target.value)} aria-label="hook model" style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
              </div>
            )}

            {asString(selectedHandler.type) === 'mcp_tool' && (
              <div style={{ display: 'grid', gap: 12 }}>
                <div style={detailGridStyle()}>
                  <Field t={t} label="MCP server">
                    <input value={asString(selectedHandler.server)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'server', event.target.value)} aria-label="hook MCP server" style={{ ...inputStyle(t), width: '100%' }} />
                  </Field>
                  <Field t={t} label="Tool">
                    <input value={asString(selectedHandler.tool)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'tool', event.target.value)} aria-label="hook MCP tool" style={{ ...inputStyle(t), width: '100%' }} />
                  </Field>
                </div>
                <Field t={t} label="Input JSON（支持递归 ${path}）">
                  <textarea key={prettyJson(selectedHandler.input)} defaultValue={prettyJson(asRecord(selectedHandler.input))} onBlur={(event) => {
                    try {
                      const value = JSON.parse(event.target.value) as unknown;
                      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('input 必须是对象');
                      updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'input', value);
                    } catch (cause) { setPageError(cause instanceof Error ? cause.message : 'Input JSON 无效'); }
                  }} rows={7} aria-label="hook MCP input" style={textareaStyle(t, 7)} />
                </Field>
              </div>
            )}

            <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
              {(['once', 'async', 'asyncRewake', 'continueOnBlock'] as const).map((key) => (
                <label key={key} style={{ display: 'inline-flex', gap: 7, alignItems: 'center', color: t.text2, fontSize: 12.5 }}>
                  <input type="checkbox" checked={selectedHandler[key] === true} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, key, event.target.checked)} />{key}
                </label>
              ))}
            </div>
            {selectedHandler.async === true && (
              <div style={detailGridStyle()}>
                <Field t={t} label="Async timeout（毫秒）">
                  <input type="number" min={0} value={typeof selectedHandler.asyncTimeout === 'number' ? selectedHandler.asyncTimeout : ''} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'asyncTimeout', event.target.value ? Number(event.target.value) : undefined)} aria-label="hook async timeout" style={{ ...inputStyle(t), width: '100%' }} />
                </Field>
                <Field t={t} label="Rewake message">
                  <input value={asString(selectedHandler.rewakeMessage)} onChange={(event) => updateHandler(selection.event, selection.groupIndex, selection.handlerIndex, 'rewakeMessage', event.target.value)} aria-label="hook rewake message" style={{ ...inputStyle(t), width: '100%' }} />
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
            }} style={ghostButtonStyle(t, false, true)}>删除 handler</button>
          </div>
        )}

        <details open={selection.kind === 'all'}>
          <summary style={{ cursor: 'pointer', color: t.text2, fontSize: 13, fontWeight: 600 }}>高级 JSON 预览 / 编辑</summary>
          <Field t={t} label="当前层 hooks JSON">
            <textarea value={draftText} onChange={(event) => setDraftText(event.target.value)} rows={18} aria-label="hooks-json" style={textareaStyle(t, 18)} />
          </Field>
        </details>

        <div style={actionRowStyle()}>
          <button type="button" disabled={loading} onClick={validateHooks} style={ghostButtonStyle(t, loading)}>
            预检
          </button>
          <button type="button" disabled={loading || !dirty || !adminAvailable || !document.revision_sha256} onClick={saveHooks} style={ghostButtonStyle(t, loading || !dirty || !adminAvailable || !document.revision_sha256)}>
            保存
          </button>
          <button type="button" disabled={loading || !dirty} onClick={discardDraft} style={ghostButtonStyle(t, loading || !dirty, true)}>
            取消
          </button>
          <button type="button" data-testid="hooks-open-settings-files" onClick={() => onNavigate(hooksPageModel({}).escapeHatch)} style={ghostButtonStyle(t)}>
            查看配置文件
          </button>
        </div>

        <div style={noteStyle(t)}>
          生效后合并视图：
          <pre style={{ margin: '8px 0 0', whiteSpace: 'pre-wrap', overflowX: 'auto' }}>{prettyJson(effectiveHooks)}</pre>
        </div>
      </div>
    );

  return (
    <>
      <Card title="Hooks">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="hooks" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} label="当前层 / 生效值" />
        <div style={managerShellStyle(t)}>
          <div style={managerSidebarStyle(t)}>
            <div style={{ display: 'grid', gap: 10 }}>
              <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索 hook 事件" aria-label="搜索 hooks" style={searchInputStyle(t)} />
              <div style={noteStyle(t)}>
                按事件 → matcher group → handler 编辑当前层；高级 JSON 仍可完整预览和修复。
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                <select value={newEvent} onChange={(event) => setNewEvent(event.target.value as (typeof HOOK_EVENTS)[number])} aria-label="新增 hook 事件" style={{ ...inputStyle(t), minWidth: 0, flex: 1 }}>
                  {availableEvents.map((event) => <option key={event} value={event}>{event}</option>)}
                </select>
                <button type="button" disabled={availableEvents.length === 0} onClick={() => {
                  mutateDraft((next) => {
                    if (!Array.isArray(next[newEvent])) next[newEvent] = [{ hooks: [{ type: 'command', command: '' }] }];
                  });
                  setSelection({ kind: 'handler', event: newEvent, groupIndex: 0, handlerIndex: 0 });
                }} style={ghostButtonStyle(t, availableEvents.length === 0)}>添加</button>
              </div>
            </div>
            <div style={sidebarListStyle()}>
              <div style={sidebarSectionTitleStyle(t)}>当前层事件</div>
              <button type="button" onClick={() => requestSelection({ kind: 'all' })} style={sidebarButtonStyle(t, selection.kind === 'all')}>
                <span style={{ fontSize: 13, fontWeight: 600 }}>全部 hooks</span>
                <span style={{ fontSize: 11.5, color: t.text4 }}>{eventSummaries.length} 个事件</span>
              </button>
              {filteredEvents.map((entry) => {
                const groups = Array.isArray(draftHooks[entry.event]) ? draftHooks[entry.event] as JsonRecord[] : [];
                const expanded = selectionEvent(selection) === entry.event;
                return (
                  <div key={entry.event} style={{ display: 'grid', gap: 6 }}>
                    <button type="button" onClick={() => requestSelection({ kind: 'event', event: entry.event })} style={sidebarButtonStyle(t, selection.kind === 'event' && selection.event === entry.event)}>
                      <span style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap' }}>
                        <span style={{ fontSize: 13, fontWeight: 600 }}>{entry.event}</span>
                        <SourcePill t={t} label={`${entry.groupCount} 组`} />
                        <SourcePill t={t} label={`${entry.handlerCount} handler`} />
                      </span>
                    </button>
                    {expanded && groups.map((groupValue, groupIndex) => {
                      const group = asRecord(groupValue);
                      const handlers = Array.isArray(group.hooks) ? group.hooks.map(asRecord) : [];
                      const groupExpanded = selectionGroup(selection) === groupIndex;
                      return (
                        <div key={`${entry.event}:${groupIndex}`} style={{ display: 'grid', gap: 5, marginLeft: 12 }}>
                          <button type="button" onClick={() => requestSelection({ kind: 'group', event: entry.event, groupIndex })} style={sidebarButtonStyle(t, selection.kind === 'group' && selection.event === entry.event && selection.groupIndex === groupIndex)}>
                            <span style={{ fontSize: 12.5, fontWeight: 600 }}>Group {groupIndex + 1}</span>
                            <span className="mono" style={{ fontSize: 11, color: t.text4 }}>{asString(group.matcher, '*')}</span>
                          </button>
                          {groupExpanded && handlers.map((handler, handlerIndex) => (
                            <button key={`${entry.event}:${groupIndex}:${handlerIndex}`} type="button" onClick={() => requestSelection({ kind: 'handler', event: entry.event, groupIndex, handlerIndex })} style={{ ...sidebarButtonStyle(t, selection.kind === 'handler' && selection.event === entry.event && selection.groupIndex === groupIndex && selection.handlerIndex === handlerIndex), marginLeft: 12, width: 'calc(100% - 12px)' }}>
                              <span style={{ fontSize: 12.5, fontWeight: 600 }}>Handler {handlerIndex + 1}</span>
                              <span className="mono" style={{ fontSize: 11, color: t.text4 }}>{asString(handler.type, 'type required')}</span>
                            </button>
                          ))}
                        </div>
                      );
                    })}
                  </div>
                );
              })}
              {filteredEvents.length === 0 && <div style={secondaryMetaStyle(t)}>当前层还没有匹配的 hook 事件。</div>}
            </div>
          </div>

          <div style={managerDetailStyle()}>
            <DomainOperationBanner t={t} operation={hookOperation} fallbackDomainLabel="Hooks" />
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                当前 hooks 草稿还未保存。切换前请先保存，或者丢弃当前修改。
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { discardDraft(); setSelection(pendingSelection); }} style={ghostButtonStyle(t)}>
                    丢弃并切换
                  </button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>
                    继续编辑
                  </button>
                </div>
              </div>
            )}
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {loading && <div style={noteStyle(t)}>正在同步 hooks 文档…</div>}
            {detail}
          </div>
        </div>
      </Card>

      <Card title="运行时 Hook Catalog">
        <Row title="刷新 Catalog" desc="这里显示当前 runtime 已加载的 hooks，不等同于某一层的静态 JSON。" align="center">
          <button type="button" onClick={() => void bridge.refreshHooks()} style={ghostButtonStyle(t)}>刷新</button>
        </Row>
        {runtimeHooks.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>当前 runtime 没有加载任何 hook。</div>
        )}
        {runtimeHooks.map((hook) => (
          <Row key={`${hook.event}:${hook.name}:${hook.matcher ?? '*'}`} title={hook.name} desc={runtimeSummary(hook)} align="center">
            {hook.blocking ? <SourcePill t={t} label="blocking" tone="warn" /> : null}
          </Row>
        ))}
      </Card>
    </>
  );
}
