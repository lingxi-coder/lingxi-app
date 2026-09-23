import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import type { WritableScopeDto } from '@lingxi/bridge-client';
import { Icon } from '../../Icon';
import { Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { parseJsonObjectInput } from '../jsonInput';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import { ExtensionHubTabs, extensionHubStyle } from './ExtensionHub';
import {
  adminRecordCommand,
  asRecord,
  asString,
  asStringArray,
  DomainOperationBanner,
  Field,
  managerDetailStyle,
  nextConfigurationOperationId,
  noteStyle,
  parseEventEnvelope,
  prettyJson,
  searchInputStyle,
  secondaryMetaStyle,
  SourcePill,
  textareaStyle,
} from './configurationAdmin';

export { parseJsonObjectInput };

/**
 * MCP 的三个作用域各自落在哪。**它和设置层不是一回事**，尽管用了同样三个词 ——
 * 见下面 `MCP_SCOPE_VS_LAYERS_NOTE`。这三条对应 `client-protocol` 里
 * `WritableScopeDto` 各变体的文档：user → `~/.lingxi.json` 顶层 `mcpServers`；
 * local → 同一个 `~/.lingxi.json` 里 `projects["<项目>"]` 下；
 * project → `<项目>/.mcp.json`。
 *
 * 与设置层的说明一样不写死路径：每台机器的实际文件位置由引擎在选中条目的
 * `path` 上回传，页面已经在标题下方显示它。
 */
const MCP_SCOPE_DESCRIPTIONS: Record<WritableScopeDto, string> = {
  user: '存在主目录的配置文件里，本机所有项目共用。',
  local: '也存在主目录的配置文件里，但只对这个项目生效——文件不在项目内，不会提交。',
  project: '存在项目内的配置文件里，随仓库提交、团队共享。',
};

/**
 * 这句对照是必须的，不是锦上添花：MCP 的「本地」和设置层的「本地」含义正好相反。
 * 设置层的 local 在**项目里**（`<项目>/.lingxi/settings.local.json`），MCP 的 local
 * 在**主目录里**。一个刚在设置页学会「本地 = 项目内、不提交」的人，到这一页会把
 * 同一个词读成同一个意思，然后猜错文件在哪。
 */
const MCP_SCOPE_VS_LAYERS_NOTE =
  'MCP 的作用域与设置页的层是两套独立存储，同名不同义：MCP 的「本地」在主目录，'
  + '设置的「本地」在项目内。在这里选的作用域不受设置页顶部层切换器影响。';

interface ScopeSnapshot {
  scope: WritableScopeDto;
  path: string;
  revision_sha256: string;
  raw_json: string;
}

interface McpRuntimeServer {
  name: string;
  status: unknown;
  transport?: string;
  source?: string;
  writable?: boolean;
  read_only_reason?: string;
}

interface McpSnapshotEnvelope {
  scopes?: ScopeSnapshot[];
  runtime_servers?: McpRuntimeServer[];
  approval?: {
    enabled_servers?: string[];
    disabled_servers?: string[];
    enable_all_project_servers?: boolean;
    legacy_source_present?: boolean;
    revision_sha256?: string;
  };
}

interface McpEntry {
  id: string;
  scope: WritableScopeDto;
  name: string;
  config: Record<string, unknown>;
  path: string;
  revision_sha256: string;
}

type Selection = { kind: 'list' } | { kind: 'server'; id: string } | { kind: 'create'; scope: WritableScopeDto };

function callMcpAdmin(bridge: PageContentProps['bridge'], command: unknown) {
  const admin = (bridge as { mcpAdmin?: (payload: unknown) => Promise<unknown> }).mcpAdmin;
  return typeof admin === 'function' ? admin(command as never) : Promise.resolve();
}

function parseSnapshot(bridge: PageContentProps['bridge']): McpSnapshotEnvelope {
  return parseEventEnvelope<McpSnapshotEnvelope>(bridge.mcpConfigurationSnapshotEvent?.snapshot_json, {
    scopes: [],
    runtime_servers: (bridge.mcpServersEvent?.servers ?? []).map((server) => ({
      name: server.name,
      status: server.status,
      transport: server.transport,
    })),
    approval: {},
  });
}

function parseEntries(snapshot: McpSnapshotEnvelope): McpEntry[] {
  return (snapshot.scopes ?? []).flatMap((scope) => {
    try {
      const parsed = JSON.parse(scope.raw_json) as Record<string, unknown>;
      const servers = asRecord(parsed.mcpServers);
      return Object.entries(servers).map(([name, config]) => ({
        id: `${scope.scope}:${name}`,
        scope: scope.scope,
        name,
        config: asRecord(config),
        path: scope.path,
        revision_sha256: scope.revision_sha256,
      }));
    } catch {
      return [];
    }
  });
}

function inferTransport(config: Record<string, unknown>): string {
  if (asString(config.type)) return asString(config.type);
  if (asString(config.transport)) return asString(config.transport);
  if (asString(config.url)) return 'http';
  if (asString(config.command)) return 'stdio';
  return 'custom';
}

function JsonPropertyEditor({
  t,
  label,
  value,
  onCommit,
}: {
  t: ReturnType<typeof useT>;
  label: string;
  value: unknown;
  onCommit: (value: unknown) => void;
}) {
  const [text, setText] = useState(prettyJson(value ?? {}));
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setText(prettyJson(value ?? {}));
    setError(null);
  }, [value]);
  return (
    <Field t={t} label={label}>
      <textarea
        value={text}
        onChange={(event) => setText(event.target.value)}
        onBlur={() => {
          try {
            onCommit(JSON.parse(text) as unknown);
            setError(null);
          } catch (cause) {
            setError(cause instanceof Error ? cause.message : 'JSON 无效');
          }
        }}
        rows={5}
        style={textareaStyle(t, 5)}
        aria-label={`mcp-${label}`}
      />
      {error && <div role="alert" style={secondaryMetaStyle(t)}>{error}</div>}
    </Field>
  );
}

export function McpServers({ bridge, onNavigate }: PageContentProps) {
  const t = useT();
  const adminAvailable = typeof (bridge as { mcpAdmin?: unknown }).mcpAdmin === 'function';
  const snapshot = useMemo(() => parseSnapshot(bridge), [bridge.mcpConfigurationSnapshotEvent?.snapshot_json, bridge.mcpServersEvent?.servers]);
  const operation = bridge.configurationOperations?.mcp ?? null;
  const entries = useMemo(() => parseEntries(snapshot), [snapshot]);
  const scopes = snapshot.scopes ?? [];
  const approval = snapshot.approval ?? {};
  const [selection, setSelection] = useState<Selection>({ kind: 'list' });
  const [search, setSearch] = useState('');
  const [draftScope, setDraftScope] = useState<WritableScopeDto>('user');
  const [draftName, setDraftName] = useState('');
  const [draftConfigText, setDraftConfigText] = useState('{\n  "command": "npx",\n  "args": ["-y", "package-name"]\n}');
  const [argumentRows, setArgumentRows] = useState<string[]>(['-y', 'package-name']);
  const [environmentRows, setEnvironmentRows] = useState<Array<{ key: string; value: string }>>([{ key: '', value: '' }]);
  const [pageError, setPageError] = useState<string | null>(null);
  const [pendingSelection, setPendingSelection] = useState<Selection | null>(null);
  const [pendingHubPage, setPendingHubPage] = useState<string | null>(null);

  useEffect(() => {
    if (adminAvailable) void callMcpAdmin(bridge, adminRecordCommand('get_snapshot'));
    else void bridge.refreshMcpServers();
  }, [adminAvailable, bridge.mcpAdmin, bridge.refreshMcpServers]);

  useEffect(() => {
    if (selection.kind === 'server' && !entries.some((entry) => entry.id === selection.id)) {
      setSelection({ kind: 'list' });
    }
  }, [entries, selection]);

  const selected = selection.kind === 'server' ? entries.find((entry) => entry.id === selection.id) ?? null : null;
  const currentScope = selected?.scope ?? (selection.kind === 'create' ? selection.scope : draftScope);

  useEffect(() => {
    const config = selected?.config ?? (selection.kind === 'create' ? { args: ['-y', 'package-name'] } : {});
    const args = Array.isArray(config.args) ? config.args.map(String) : [];
    setArgumentRows(args.length > 0 ? args : ['']);
    const env = Object.entries(asRecord(config.env)).map(([key, value]) => ({ key, value: asString(value) }));
    setEnvironmentRows(env.length > 0 ? env : [{ key: '', value: '' }]);
  }, [selected, selection.kind]);

  useEffect(() => {
    if (selected) {
      setDraftScope(selected.scope);
      setDraftName(selected.name);
      setDraftConfigText(prettyJson(selected.config));
    } else if (selection.kind === 'create') {
      setDraftScope(selection.scope);
      setDraftName('');
      setDraftConfigText('{\n  "command": "npx",\n  "args": ["-y", "package-name"]\n}');
    }
  }, [selected, selection]);

  const dirty = selection.kind !== 'list' && (selected
    ? draftConfigText !== prettyJson(selected.config)
    : draftName.trim().length > 0 || draftConfigText !== '{\n  "command": "npx",\n  "args": ["-y", "package-name"]\n}');
  const requestHubNavigation = (pageId: string) => {
    if (dirty) {
      setPendingHubPage(pageId);
      return;
    }
    onNavigate(pageId);
  };
  const discardAndNavigate = () => {
    if (!pendingHubPage) return;
    setPendingSelection(null);
    setSelection({ kind: 'list' });
    setDraftScope('user');
    setDraftName('');
    setDraftConfigText('{\n  "command": "npx",\n  "args": ["-y", "package-name"]\n}');
    onNavigate(pendingHubPage);
    setPendingHubPage(null);
  };
  const navigationPrompt = pendingHubPage && (
    <div role="alert" style={noteStyle(t, 'warn')}>
      Unsaved MCP changes. Discard them before switching tabs?
      <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <button type="button" onClick={discardAndNavigate} style={ghostButtonStyle(t)}>Discard and switch</button>
        <button type="button" onClick={() => setPendingHubPage(null)} style={ghostButtonStyle(t, false, true)}>Continue editing</button>
      </div>
    </div>
  );
  const draftConfig = useMemo(() => {
    try {
      const parsed = JSON.parse(draftConfigText) as unknown;
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, unknown> : null;
    } catch {
      return null;
    }
  }, [draftConfigText]);
  const updateConfig = (key: string, value: unknown) => {
    if (!draftConfig) {
      setPageError('请先修复高级配置 JSON，才能使用结构化编辑器。');
      return;
    }
    const next = { ...draftConfig };
    if (value === undefined) delete next[key];
    else next[key] = value;
    setDraftConfigText(prettyJson(next));
    setPageError(null);
  };
  const transport = draftConfig ? inferTransport(draftConfig) : 'custom';
  const setTransport = (nextTransport: string) => {
    if (!draftConfig) return;
    const next = { ...draftConfig };
    if (nextTransport === 'stdio') {
      delete next.type;
      delete next.transport;
      delete next.url;
      delete next.headers;
      delete next.headersHelper;
      delete next.oauth;
      delete next.discoveryCache;
      next.command = asString(next.command, 'npx');
      next.args = argumentRows.filter(Boolean);
      if (!Array.isArray(next.args) || next.args.length === 0) setArgumentRows(['']);
    } else {
      next.type = nextTransport;
      next.url = asString(next.url);
      delete next.command;
      delete next.args;
      delete next.env;
      setArgumentRows(['']);
      setEnvironmentRows([{ key: '', value: '' }]);
      if (!['http', 'streamable-http', 'sse'].includes(nextTransport)) delete next.discoveryCache;
    }
    setDraftConfigText(prettyJson(next));
    setPageError(null);
  };

  const updateArgument = (index: number, value: string) => {
    const next = [...argumentRows];
    next[index] = value;
    setArgumentRows(next);
    updateConfig('args', next.filter(Boolean));
  };
  const addArgument = () => setArgumentRows((rows) => [...rows, '']);
  const removeArgument = (index: number) => {
    const next = argumentRows.filter((_, rowIndex) => rowIndex !== index);
    setArgumentRows(next.length > 0 ? next : ['']);
    updateConfig('args', next.filter(Boolean));
  };
  const updateEnvironment = (index: number, field: 'key' | 'value', value: string) => {
    const next = environmentRows.map((row, rowIndex) => rowIndex === index ? { ...row, [field]: value } : row);
    setEnvironmentRows(next);
    updateConfig('env', Object.fromEntries(next.filter((row) => row.key.trim()).map(({ key, value: envValue }) => [key, envValue])));
  };
  const addEnvironment = () => setEnvironmentRows((rows) => [...rows, { key: '', value: '' }]);
  const removeEnvironment = (index: number) => {
    const next = environmentRows.filter((_, rowIndex) => rowIndex !== index);
    setEnvironmentRows(next.length > 0 ? next : [{ key: '', value: '' }]);
    updateConfig('env', Object.fromEntries(next.filter((row) => row.key.trim()).map(({ key, value: envValue }) => [key, envValue])));
  };

  const requestSelection = (next: Selection) => {
    if (dirty) {
      setPendingSelection(next);
      return;
    }
    setPendingSelection(null);
    setSelection(next);
    setPageError(null);
  };

  const filtered = search.trim()
    ? entries.filter((entry) => `${entry.scope} ${entry.name} ${entry.path}`.toLowerCase().includes(search.trim().toLowerCase()))
    : entries;

  const save = () => {
    const parsed = parseJsonObjectInput(draftConfigText, '服务器配置');
    if ('error' in parsed) {
      setPageError(parsed.error);
      return;
    }
    const scopeSnapshot = scopes.find((scope) => scope.scope === draftScope);
    if (!scopeSnapshot) {
      setPageError('当前没有该作用域的配置快照。');
      return;
    }
    if (!draftName.trim()) {
      setPageError('需要一个服务器名称。');
      return;
    }
    setPageError(null);
    void callMcpAdmin(bridge, adminRecordCommand('save_server', {
      operation_id: nextConfigurationOperationId(),
      scope: draftScope,
      revision: scopeSnapshot.revision_sha256,
      payload_json: JSON.stringify({ scope: draftScope, name: draftName.trim(), config: parsed.config }),
    })).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存 MCP 服务器。'));
  };

  const remove = () => {
    if (!selected) return;
    setPageError(null);
    void callMcpAdmin(bridge, adminRecordCommand('remove_server', {
      operation_id: nextConfigurationOperationId(),
      scope: selected.scope,
      revision: selected.revision_sha256,
      payload_json: JSON.stringify({ scope: selected.scope, name: selected.name }),
    })).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法移除 MCP 服务器。'));
  };

  const setApproval = (name: string, decision: 'approve' | 'reject' | 'clear' | 'approve_all') => {
    if (!approval.revision_sha256 || !name) return;
    setPageError(null);
    void callMcpAdmin(bridge, adminRecordCommand('set_approval', {
      operation_id: nextConfigurationOperationId(),
      scope: 'local',
      revision: approval.revision_sha256,
      payload_json: JSON.stringify({ name, decision }),
    })).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法更新审批。'));
  };

  return (
    <>
      <div className="extension-hub-page" style={extensionHubStyle(t)}>
        <ExtensionHubTabs bridge={bridge} active="mcp" onNavigate={requestHubNavigation} />
        {/* 同名不同义的提醒放在页首，而不是藏在作用域下拉旁边：读到下拉的时候，
            人已经在拿设置页的「本地」去理解这里的「本地」了。 */}
        <details className="extension-hub-scope-note" data-testid="mcp-scope-vs-layers">
          <summary>MCP storage scopes</summary>
          <div style={{ ...noteStyle(t), marginTop: 8 }}>
          {MCP_SCOPE_VS_LAYERS_NOTE}
          </div>
        </details>
        <div className="configuration-page">
          {selection.kind === 'list' ? <div className="configuration-list">
            <DomainOperationBanner t={t} operation={operation} fallbackDomainLabel="MCP" />
            <div className="extension-hub-toolbar">
              <input className="extension-hub-search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search MCP servers" aria-label="搜索 MCP 服务器" style={searchInputStyle(t)} />
              <div className="extension-hub-toolbar-actions">
                <button type="button" onClick={() => requestSelection({ kind: 'create', scope: draftScope })} style={{ ...ghostButtonStyle(t), background: t.text, color: t.surface, borderColor: t.text }}>Add <Icon name="chevron" size={12} /></button>
                <button type="button" aria-label="Refresh MCP servers" onClick={() => void (adminAvailable ? callMcpAdmin(bridge, adminRecordCommand('get_snapshot')) : bridge.refreshMcpServers())} className="extension-hub-icon-button"><Icon name="refresh" size={16} /></button>
              </div>
            </div>
            {navigationPrompt}
            <div className="extension-hub-group-title">Servers</div>
            <div className="extension-hub-list">
              {filtered.map((entry) => {
                const rejected = asStringArray(approval.disabled_servers).includes(entry.name);
                const approved = approval.enable_all_project_servers || asStringArray(approval.enabled_servers).includes(entry.name);
                const allowed = approved && !rejected;
                const runtime = (snapshot.runtime_servers ?? []).find((server) => server.name === entry.name);
                return <div key={entry.id} className="extension-hub-row configuration-entry">
                  <span className="extension-hub-icon"><Icon name="server" size={18} color={t.text3} /></span>
                  <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: 'server', id: entry.id })}>
                    <span className="extension-hub-row-title">{entry.name}</span>
                    <span className="extension-hub-row-description">{inferTransport(entry.config)} · {runtime ? asString(runtime.status, 'available') : entry.path}</span>
                  </button>
                  <div className="extension-hub-row-side">
                    <span className="extension-hub-row-scope">{entry.scope === 'user' ? 'Personal' : entry.scope === 'project' ? 'Project' : 'Local'}</span>
                    <button type="button" className="extension-hub-icon-button" aria-label={`Configure ${entry.name}`} onClick={() => requestSelection({ kind: 'server', id: entry.id })}><Icon name="cog" size={16} /></button>
                    {entry.scope === 'project' && <button type="button" className="extension-hub-toggle" role="switch" aria-checked={allowed} aria-label={`${allowed ? 'Disable' : 'Approve'} ${entry.name}`} disabled={!approval.revision_sha256} onClick={() => setApproval(entry.name, allowed ? 'reject' : 'approve')} />}
                  </div>
                </div>;
              })}
              {filtered.length === 0 && <div style={{ padding: '24px 18px', color: t.text3, fontSize: 13 }}>没有匹配的 MCP 服务器。</div>}
            </div>
          </div> : <div className="configuration-detail mcp-editor" style={{ ...managerDetailStyle(), '--mcp-border': t.border, '--mcp-muted': t.text3, '--mcp-text': t.text, '--mcp-surface': t.surface, '--mcp-hover': t.surfaceHover } as CSSProperties}>
            <button type="button" className="configuration-back" onClick={() => requestSelection({ kind: 'list' })} style={ghostButtonStyle(t)}>← Back to servers</button>
            <DomainOperationBanner t={t} operation={operation} fallbackDomainLabel="MCP" />
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                当前草稿未保存。
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { setPendingSelection(null); setSelection(pendingSelection); setPageError(null); }} style={ghostButtonStyle(t)}>丢弃并切换</button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>继续编辑</button>
                </div>
              </div>
            )}
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {navigationPrompt}
            <div>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 6 }}>
                <div style={{ fontSize: 18, fontWeight: 650, color: t.text }}>{selected?.name || 'Connect to a custom MCP'}</div>
                <SourcePill t={t} label={currentScope} />
                {selected && <SourcePill t={t} label={inferTransport(selected.config)} />}
              </div>
              {selected && <div className="mono" style={secondaryMetaStyle(t)}>{selected.path}</div>}
            </div>
            <section className="mcp-connection mcp-identity-card">
              <Field t={t} label="Name">
                <input value={draftName} onChange={(event) => setDraftName(event.target.value)} disabled={Boolean(selected)} style={{ ...inputStyle(t), opacity: selected ? 0.65 : 1 }} aria-label="mcp-name" placeholder="MCP server name" />
              </Field>
              <div className="mcp-type-row">
                <span>Type</span>
                <div className="mcp-type-selector" role="group" aria-label="MCP transport type">
                  <button type="button" aria-pressed={transport === 'stdio'} onClick={() => setTransport('stdio')} disabled={!draftConfig}>STDIO</button>
                  <button type="button" aria-pressed={transport !== 'stdio'} onClick={() => setTransport('streamable-http')} disabled={!draftConfig}>Streamable HTTP</button>
                </div>
              </div>
              <details className="mcp-options">
                <summary>Scope and storage <span>{currentScope}</span></summary>
                <Field t={t} label="Scope">
                  <select value={draftScope} onChange={(event) => { const next = event.target.value as WritableScopeDto; setDraftScope(next); setSelection({ kind: 'create', scope: next }); }} style={inputStyle(t)} aria-label="mcp-scope" disabled={Boolean(selected)}>
                    <option value="user">User</option>
                    <option value="local">Local</option>
                    <option value="project">Project</option>
                  </select>
                  <div data-testid="mcp-scope-description" style={{ marginTop: 6, fontSize: 11.5, color: t.text3, lineHeight: 1.6 }}>
                    {MCP_SCOPE_DESCRIPTIONS[draftScope]}
                  </div>
                </Field>
              </details>
            </section>
            <section className="mcp-connection">
              {transport === 'stdio' ? (
                <>
                  <Field t={t} label="Command to launch">
                    <input value={asString(draftConfig?.command)} onChange={(event) => updateConfig('command', event.target.value)} style={inputStyle(t)} aria-label="mcp-command" />
                  </Field>
                  <div className="mcp-repeater">
                    <div className="mcp-repeater-label">Arguments</div>
                    {argumentRows.map((argument, index) => (
                      <div className="mcp-repeater-row" key={`arg-${index}`}>
                        <input value={argument} onChange={(event) => updateArgument(index, event.target.value)} style={inputStyle(t)} aria-label={`mcp-argument-${index}`} />
                        <button type="button" className="extension-hub-icon-button" aria-label={`Remove argument ${index + 1}`} onClick={() => removeArgument(index)}><Icon name="trash" size={15} /></button>
                      </div>
                    ))}
                    <button type="button" className="mcp-add-row" onClick={addArgument}><Icon name="plus" size={14} /> Add argument</button>
                  </div>
                  <div className="mcp-repeater">
                    <div className="mcp-repeater-label">Environment variables</div>
                    {environmentRows.map((row, index) => (
                      <div className="mcp-repeater-row mcp-env-row" key={`env-${index}`}>
                        <input value={row.key} onChange={(event) => updateEnvironment(index, 'key', event.target.value)} style={inputStyle(t)} aria-label={`mcp-env-key-${index}`} placeholder="Key" />
                        <input value={row.value} onChange={(event) => updateEnvironment(index, 'value', event.target.value)} style={inputStyle(t)} aria-label={`mcp-env-value-${index}`} placeholder="Value" />
                        <button type="button" className="extension-hub-icon-button" aria-label={`Remove environment variable ${index + 1}`} onClick={() => removeEnvironment(index)}><Icon name="trash" size={15} /></button>
                      </div>
                    ))}
                    <button type="button" className="mcp-add-row" onClick={addEnvironment}><Icon name="plus" size={14} /> Add environment variable</button>
                  </div>
                </>
              ) : transport !== 'custom' ? (
                <>
                  <Field t={t} label="URL">
                    <input value={asString(draftConfig?.url)} onChange={(event) => updateConfig('url', event.target.value)} style={inputStyle(t)} aria-label="mcp-url" />
                  </Field>
                  <JsonPropertyEditor t={t} label="headers JSON" value={draftConfig?.headers ?? {}} onCommit={(value) => updateConfig('headers', value)} />
                  <Field t={t} label="Headers helper">
                    <input value={asString(draftConfig?.headersHelper)} onChange={(event) => updateConfig('headersHelper', event.target.value || undefined)} style={inputStyle(t)} aria-label="mcp-headers-helper" />
                  </Field>
                  <JsonPropertyEditor t={t} label="OAuth JSON" value={draftConfig?.oauth ?? {}} onCommit={(value) => updateConfig('oauth', value)} />
                </>
              ) : null}
              <details className="mcp-options"><summary>Advanced connection options</summary>
                <div className="mcp-editor-grid">
                  <Field t={t} label="Transport">
                    <select value={transport} onChange={(event) => setTransport(event.target.value)} disabled={!draftConfig} style={inputStyle(t)} aria-label="mcp-transport">
                      <option value="stdio">stdio</option>
                      <option value="http">http</option>
                      <option value="streamable-http">streamable-http</option>
                      <option value="sse">sse</option>
                      <option value="ws">ws</option>
                      {transport === 'custom' && <option value="custom">Internal / custom</option>}
                    </select>
                  </Field>
                  <Field t={t} label="Timeout (ms)">
                    <input type="number" min={1} value={typeof draftConfig?.timeout === 'number' ? draftConfig.timeout : ''} onChange={(event) => updateConfig('timeout', event.target.value ? Number(event.target.value) : undefined)} style={inputStyle(t)} aria-label="mcp-timeout" />
                  </Field>
                </div>
              </details>
              <details className="mcp-options"><summary>工具与加载选项 <span>按需配置</span></summary>
              <div className="mcp-editor-grid">
                <Field t={t} label="Always load">
                  <select value={draftConfig?.alwaysLoad === undefined ? '' : draftConfig.alwaysLoad ? 'true' : 'false'} onChange={(event) => updateConfig('alwaysLoad', event.target.value === '' ? undefined : event.target.value === 'true')} style={inputStyle(t)} aria-label="mcp-always-load">
                    <option value="">默认</option><option value="true">开启</option><option value="false">关闭</option>
                  </select>
                </Field>
                {['http', 'streamable-http', 'sse'].includes(transport) && (
                  <Field t={t} label="Discovery cache">
                    <select value={draftConfig?.discoveryCache === undefined ? '' : draftConfig.discoveryCache ? 'true' : 'false'} onChange={(event) => updateConfig('discoveryCache', event.target.value === '' ? undefined : event.target.value === 'true')} style={inputStyle(t)} aria-label="mcp-discovery-cache">
                      <option value="">默认</option><option value="true">开启</option><option value="false">关闭</option>
                    </select>
                  </Field>
                )}
              </div>
              <JsonPropertyEditor t={t} label="tools JSON" value={draftConfig?.tools ?? []} onCommit={(value) => updateConfig('tools', value)} />
              <JsonPropertyEditor t={t} label="toolPermissions JSON" value={draftConfig?.toolPermissions ?? {}} onCommit={(value) => updateConfig('toolPermissions', value)} />
              </details>
            </section>
            <details className="mcp-options mcp-raw-config">
              <summary style={{ cursor: 'pointer', color: t.text3, fontSize: 12.5, fontWeight: 700 }}>高级配置 JSON</summary>
              <Field t={t} label="完整服务器配置">
                <textarea value={draftConfigText} onChange={(event) => setDraftConfigText(event.target.value)} rows={16} style={textareaStyle(t, 16)} aria-label="mcp-config-json" />
              </Field>
            </details>
            {selected?.scope === 'project' && (
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                <button type="button" onClick={() => setApproval(selected.name, 'approve')} style={ghostButtonStyle(t)}>批准</button>
                <button type="button" onClick={() => setApproval(selected.name, 'reject')} style={ghostButtonStyle(t, false, true)}>拒绝</button>
                <button type="button" onClick={() => setApproval(selected.name, 'clear')} style={ghostButtonStyle(t)}>撤销</button>
                <button type="button" onClick={() => setApproval(selected.name, 'approve_all')} style={ghostButtonStyle(t)}>批准全部</button>
              </div>
            )}
            <div className="mcp-editor-actions">
              <button type="button" onClick={save} style={{ ...ghostButtonStyle(t), background: t.text, color: t.surface, borderColor: t.text }}>保存</button>
              {selected && <button type="button" onClick={remove} style={ghostButtonStyle(t, false, true)}>移除</button>}
            </div>
            <div style={noteStyle(t)}>
              <div style={{ fontWeight: 700, marginBottom: 6 }}>项目审批摘要</div>
              <div style={secondaryMetaStyle(t)}>批准：{asStringArray(approval.enabled_servers).join(', ') || '无'}</div>
              <div style={secondaryMetaStyle(t)}>拒绝：{asStringArray(approval.disabled_servers).join(', ') || '无'}</div>
              <div style={secondaryMetaStyle(t)}>批准全部：{approval.enable_all_project_servers ? '开启' : '关闭'}</div>
              {approval.legacy_source_present && <div style={secondaryMetaStyle(t)}>检测到旧 ~/.lingxi.json 审批来源。</div>}
            </div>
          </div>}
        </div>
        {selection.kind === 'list' && <details className="extension-hub-runtime">
          <summary>Connection status · {(snapshot.runtime_servers ?? []).length}</summary>
          {(snapshot.runtime_servers ?? []).length === 0 && <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>暂无运行中的 MCP 服务器。</div>}
          {(snapshot.runtime_servers ?? []).map((server) => (
            <Row
              key={`${server.source ?? 'runtime'}:${server.name}`}
              title={server.name}
              desc={`${asString(server.transport, 'transport?')} · ${typeof server.status === 'string' ? server.status : JSON.stringify(server.status)}${server.read_only_reason ? ` · ${server.read_only_reason}` : ''}`}
              align="center"
            >
              <SourcePill t={t} label={server.source ?? (server.writable ? 'config' : 'runtime')} />
            </Row>
          ))}
        </details>}
      </div>
    </>
  );
}
