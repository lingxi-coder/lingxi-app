import { useEffect, useState } from 'react';
import type { McpScopeDto } from '@lingxi/bridge-client';
import { Card, ProvenanceBadge, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { parseJsonObjectInput } from '../jsonInput';
import { ghostButtonStyle, inputStyle } from './ghostButton';

// Re-exported so callers that used to import this FROM this file (including
// `settings-coding-pages.test.ts`) keep working now that the canonical
// definition lives in `../jsonInput` (Task 18 fix round 1, Minor — a
// generic JSON-object parser should not create a page-to-page dependency,
// which is what `Plugins.tsx` importing it from here did).
export { parseJsonObjectInput };

const SCOPES: { id: McpScopeDto; label: string }[] = [
  { id: 'user', label: '用户 (~/.lingxi.json)' },
  { id: 'local', label: '本地 (~/.lingxi.json projects[…])' },
  { id: 'project', label: '项目 (<project>/.mcp.json)' },
];

/**
 * MCP is NOT layered (`nav.ts` marks it `layered: false`) — it owns three
 * storage locations of its own (`mcp_bridge.rs`'s table: User/Local both in
 * `~/.lingxi.json`, Project in `<project>/.mcp.json`), unrelated to the
 * settings-file layer stack the shell's switcher targets. This page carries
 * its OWN scope selector as page-local state, never reads/writes
 * `editingLayer`.
 *
 * **A real data gap, disclosed rather than papered over**: the read side
 * (`refresh_listings{mcp}` → `ClientEvent::McpServers`, pre-existing, Task 6
 * left it untouched) reports a MERGED, currently-running view — name,
 * connection status, transport — with NO scope field
 * (`client-protocol/src/listings.rs`'s `McpServerDto` carries exactly those
 * three fields; `platform_api::orchestrator::McpServerInfo` it's lowered from
 * carries no more). There is also no read command for "what does scope X's
 * OWN file currently contain" — `mcp_bridge.rs` is write-only
 * (`upsert_server`/`remove_server`), by design (its own module doc: the
 * read side is the live registry snapshot, not a per-scope file reader).
 * Two consequences this page cannot engineer around without a wire change
 * (out of scope here — `lingxi-code/` is not touched by this task):
 * - the "运行状态" list below cannot be split into per-scope tabs, and
 *   cannot tell a `Dynamic` (plugin) or `Enterprise` (managed) entry apart
 *   from a `User`/`Local`/`Project` one — so instead of a false per-row
 *   "来源: xxx" badge, this page says plainly that the list is scope-blind
 *   and that add/remove below is a blind write, not an edit of a visible
 *   row.
 * - there is no way to show "what's currently in scope X" before writing;
 *   add/remove act on a name the user types, and `remove_mcp_server` is
 *   idempotent server-side, so removing a name that never existed in that
 *   scope is a safe no-op rather than an error.
 *
 * **Task 18 fix round 1, Important**: the gap above is worse than the first
 * cut of this page said. A project-scope server held at
 * `McpServerBlockReason::ProjectPendingApproval` (`mcp/src/server_gate.rs`)
 * still enters the registry with `McpServerConfig.disabled = true`
 * (`mcp/src/connection.rs`) and is seeded as `Disconnected` — which renders
 * in the "运行状态" list below as `stdio · disconnected`, byte-identical to
 * a server that connected once and shut down cleanly, or one that was never
 * reachable. The prose warning inside the "添加 / 更新服务器" card only
 * shows up when someone actually selects Project scope there; a person who
 * never touches that selector would see an ambiguous row and nothing else.
 * The disclosure in the "运行状态" card header below is UNCONDITIONAL for
 * exactly that reason.
 */
export function McpServers({ bridge }: PageContentProps) {
  const t = useT();
  const [scope, setScope] = useState<McpScopeDto>('user');
  const [name, setName] = useState('');
  const [configText, setConfigText] = useState('{\n  "command": "npx",\n  "args": ["-y", "package-name"]\n}');
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [removeName, setRemoveName] = useState('');
  const [removing, setRemoving] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => { void bridge.refreshMcpServers(); }, [bridge.refreshMcpServers]);

  const servers = bridge.mcpServersEvent?.servers ?? [];

  const handleUpsert = () => {
    const trimmedName = name.trim();
    if (!trimmedName) { setFormError('需要一个服务器名称。'); return; }
    const parsed = parseJsonObjectInput(configText, '服务器配置');
    if ('error' in parsed) { setFormError(parsed.error); return; }
    setFormError(null);
    setSaving(true);
    setActionError(null);
    void bridge.upsertMcpServer(scope, trimmedName, parsed.config)
      .then(() => { setName(''); })
      .catch((cause) => setActionError(cause instanceof Error ? cause.message : '无法保存 MCP 服务器。'))
      .finally(() => setSaving(false));
  };

  const handleRemove = () => {
    const trimmedName = removeName.trim();
    if (!trimmedName) return;
    setRemoving(true);
    setActionError(null);
    void bridge.removeMcpServer(scope, trimmedName)
      .then(() => setRemoveName(''))
      .catch((cause) => setActionError(cause instanceof Error ? cause.message : '无法移除 MCP 服务器。'))
      .finally(() => setRemoving(false));
  };

  return (
    <>
      <Card title="运行状态">
        <div style={{ padding: '10px 18px 0', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
          这份列表反映当前实际连接的服务器（跨三个可写域，加上只读的 Dynamic / Enterprise 来源合并展示），引擎没有上报每一项具体来自哪个存储位置——所以这里不能按域拆分，也无法把插件（Dynamic）或托管策略（Enterprise）提供的只读条目单独标出。下方的新增/移除操作是对着你选的域「盲写」，不是对这份列表里某一行的编辑。
        </div>
        <div style={{ padding: '8px 18px 0', fontSize: 12, color: t.warn, lineHeight: 1.6 }}>
          一个卡在「待审批」状态的项目域服务器，在这份列表里显示为 <code className="mono">disconnected</code>
          ——与一个正常连接后又断开、或从未连接过的服务器完全相同，本页无法把这两种情况区分开，不要把
          「disconnected」直接读成「这个服务器干净地停止了」。
        </div>
        {servers.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>没有已连接的 MCP 服务器，或引擎尚未上报。</div>
        )}
        {servers.map((server) => (
          <Row
            key={server.name}
            align="center"
            title={<span className="mono" style={{ fontSize: 13 }}>{server.name}</span>}
            desc={`${server.transport} · ${server.status.type}${server.status.type === 'error' ? `：${server.status.reason}` : ''}`}
          >
            {null}
          </Row>
        ))}
      </Card>

      <Card title="添加 / 更新服务器">
        <Row title="域" desc="决定写入哪一个存储位置。" align="center">
          <select value={scope} onChange={(e) => setScope(e.target.value as McpScopeDto)} aria-label="MCP 域" style={inputStyle(t)}>
            {SCOPES.map((s) => <option key={s.id} value={s.id}>{s.label}</option>)}
          </select>
        </Row>
        {scope === 'project' && (
          <Row title="项目域的审批" align="start">
            <div style={{ fontSize: 12, color: t.warn, maxWidth: 480, lineHeight: 1.6 }}>
              项目域的服务器需要经过审批（`enabledMcpjsonServers` / `enableAllProjectMcpServers`）才会真正启用；
              这两个审批开关已被迁移工具从唯一被读取的位置移除，导致任何跑过该迁移的机器上，项目域的服务器会永久停留在
              「待审批」状态，且没有可用的审批入口。这是引擎已存在的问题，本页无法修复——这里写入的项目域条目会被保存，
              但请不要期待它们会真正生效。
            </div>
          </Row>
        )}
        <Row title="名称" align="center">
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="服务器名称" aria-label="MCP 服务器名称" style={{ ...inputStyle(t), width: 240 }} />
        </Row>
        <Row title="配置 (JSON)" align="start">
          <textarea
            value={configText}
            onChange={(e) => setConfigText(e.target.value)}
            aria-label="MCP 服务器配置"
            rows={6}
            className="mono"
            style={{ ...inputStyle(t), width: 420, resize: 'vertical' }}
          />
        </Row>
        {formError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{formError}</span></Row>}
        <Row title="保存" align="center">
          <button type="button" disabled={saving} onClick={handleUpsert} style={ghostButtonStyle(t, saving)}>{saving ? '保存中…' : '保存'}</button>
        </Row>
      </Card>

      <Card title="移除服务器">
        <Row title="按名称移除" desc="从上面选中的域移除；名称不存在时是安全的空操作。" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input value={removeName} onChange={(e) => setRemoveName(e.target.value)} placeholder="服务器名称" aria-label="要移除的 MCP 服务器名称" style={inputStyle(t)} />
            <button type="button" disabled={removing || !removeName.trim()} onClick={handleRemove} style={ghostButtonStyle(t, removing || !removeName.trim(), true)}>
              {removing ? '移除中…' : '移除'}
            </button>
          </div>
        </Row>
        {actionError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{actionError}</span></Row>}
      </Card>

      <Card title="只读来源">
        <Row title="Dynamic / Enterprise" desc="由插件或托管策略提供，不能从这个页面编辑或删除。" align="center">
          <ProvenanceBadge destination="managed" />
        </Row>
      </Card>
    </>
  );
}
