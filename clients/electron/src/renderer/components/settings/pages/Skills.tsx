import { useEffect, useMemo, useRef, useState } from 'react';
import type { SkillAdminCommandDto, SkillDto } from '@lingxi/bridge-client';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { Toggle } from '../primitives';
import { useT } from '../../../theme/ThemeContext';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { boolFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import {
  asBoolean,
  asString,
  detailGridStyle,
  DomainOperationBanner,
  EmptyDetail,
  Field,
  managerDetailStyle,
  nextConfigurationOperationId,
  noteStyle,
  parseEventEnvelope,
  searchInputStyle,
  secondaryMetaStyle,
  sidebarButtonStyle,
  sidebarListStyle,
  sidebarSectionTitleStyle,
  SourcePill,
  textareaStyle,
} from './configurationAdmin';

const EMPTY_SHA256 = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

interface SkillCatalogItem {
  id: string;
  name: string;
  source: string;
  pluginOwner?: string;
  rootDir: string;
  directory: string;
  writable: boolean;
  readonlyReason?: string;
  revision?: string;
  description?: string;
  whenToUse?: string;
  parseError?: string;
  trashId?: string;
  trashedAt?: string;
}

interface SkillCatalogEnvelope {
  entries?: SkillCatalogItem[];
  trash?: SkillCatalogItem[];
  sync_claude_ai_note?: string;
}

interface SkillDocumentEnvelope {
  id: string;
  name: string;
  source: string;
  pluginOwner?: string;
  directory: string;
  rootDir: string;
  markdown: string;
  writable: boolean;
  revision: string;
  readonlyReason?: string;
  parseError?: string;
  diagnosticsJson?: string;
}

type SkillSelection =
  | { kind: 'list' }
  | { kind: 'skill'; id: string }
  | { kind: 'trash'; id: string }
  | { kind: 'create' };

export interface SkillsPageModel {
  skills: SkillDto[];
  layerAffects: 'syncClaudeAiSkills only';
}

export function skillsPageModel(input: { skills?: SkillDto[] }): SkillsPageModel {
  return { skills: input.skills ?? [], layerAffects: 'syncClaudeAiSkills only' };
}

function sourceScope(source: string): 'user' | 'project' | null {
  return source === 'user' || source === 'project' ? source : null;
}

function skillCatalogFromBridge(bridge: PageContentProps['bridge']): SkillCatalogEnvelope {
  const fromEvent = parseEventEnvelope<SkillCatalogEnvelope>(bridge.skillCatalogEvent?.catalog_json, {});
  if (Array.isArray(fromEvent.entries) || Array.isArray(fromEvent.trash)) return fromEvent;
  return {
    entries: (bridge.skillsEvent?.skills ?? []).map((skill) => ({
      id: skill.source_dir,
      name: skill.name,
      source: skill.source_dir.includes('/.lingxi/skills/') ? 'project' : 'user',
      rootDir: skill.source_dir.replace(/\/[^/]+$/, ''),
      directory: skill.source_dir,
      writable: false,
      readonlyReason: '当前运行时只上报了已发现的 skills 列表，未提供可编辑文档。',
    })),
    trash: [],
    sync_claude_ai_note: 'Stored only. Claude.ai cloud sync is not wired on desktop.',
  };
}

function skillDocumentFromBridge(bridge: PageContentProps['bridge']): SkillDocumentEnvelope | null {
  const parsed = parseEventEnvelope<Partial<SkillDocumentEnvelope> | null>(bridge.skillDocumentEvent?.document_json, null);
  if (!parsed || typeof parsed.id !== 'string') return null;
  return {
    id: parsed.id,
    name: asString(parsed.name),
    source: asString(parsed.source),
    pluginOwner: typeof parsed.pluginOwner === 'string' ? parsed.pluginOwner : undefined,
    directory: asString(parsed.directory),
    rootDir: asString(parsed.rootDir),
    markdown: asString(parsed.markdown),
    writable: asBoolean(parsed.writable),
    revision: asString(parsed.revision),
    readonlyReason: typeof parsed.readonlyReason === 'string' ? parsed.readonlyReason : undefined,
    parseError: typeof parsed.parseError === 'string' ? parsed.parseError : undefined,
    diagnosticsJson: typeof parsed.diagnosticsJson === 'string' ? parsed.diagnosticsJson : undefined,
  };
}

function callSkillAdmin(bridge: PageContentProps['bridge'], command: SkillAdminCommandDto | Record<string, unknown>) {
  const admin = (bridge as { skillAdmin?: (payload: unknown) => Promise<unknown> }).skillAdmin;
  return typeof admin === 'function' ? admin(command) : Promise.resolve();
}

function defaultCreateScope(editingLayer: EditableLayer): 'user' | 'project' {
  return editingLayer === 'project' ? 'project' : 'user';
}

export function Skills({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const model = skillsPageModel({ skills: bridge.skillsEvent?.skills });
  const catalog = useMemo(() => skillCatalogFromBridge(bridge), [bridge.skillCatalogEvent?.catalog_json, bridge.skillsEvent?.skills]);
  const document = useMemo(() => skillDocumentFromBridge(bridge), [bridge.skillDocumentEvent?.document_json]);
  const adminAvailable = typeof (bridge as { skillAdmin?: unknown }).skillAdmin === 'function';
  const skillOperation = bridge.configurationOperations?.skill ?? null;
  const syncEnabled = boolFromLayer(snapshot, editingLayer, 'syncClaudeAiSkills');

  const [selection, setSelection] = useState<SkillSelection>({ kind: 'list' });
  const creatingSkill = useRef(false);
  const [search, setSearch] = useState('');
  const [draftContent, setDraftContent] = useState('');
  const [draftCreateName, setDraftCreateName] = useState('');
  const [draftCreateScope, setDraftCreateScope] = useState<'user' | 'project'>(defaultCreateScope(editingLayer));
  const [draftCreateContent, setDraftCreateContent] = useState('---\ndescription: \n---\n');
  const [moveScope, setMoveScope] = useState<'user' | 'project'>(defaultCreateScope(editingLayer));
  const [moveName, setMoveName] = useState('');
  const [restoreScope, setRestoreScope] = useState<'user' | 'project'>(defaultCreateScope(editingLayer));
  const [restoreName, setRestoreName] = useState('');
  const [syncDraft, setSyncDraft] = useState(syncEnabled);
  const [pendingSelection, setPendingSelection] = useState<SkillSelection | null>(null);
  const [savingSync, setSavingSync] = useState(false);
  const [loadingDocument, setLoadingDocument] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [pageError, setPageError] = useState<string | null>(null);
  const [purgeConfirmId, setPurgeConfirmId] = useState<string | null>(null);

  useEffect(() => {
    void (adminAvailable ? callSkillAdmin(bridge, { action: 'get_catalog' }) : bridge.refreshSkills());
  }, [adminAvailable, bridge.refreshSkills, bridge.skillAdmin]);

  useEffect(() => {
    setSyncDraft(syncEnabled);
    setDraftCreateScope(defaultCreateScope(editingLayer));
    setRestoreScope(defaultCreateScope(editingLayer));
    if (editingLayer !== 'project' && moveScope !== 'project') setMoveScope('user');
  }, [editingLayer, moveScope, syncEnabled]);

  const activeSkills = catalog.entries ?? [];
  const trashedSkills = catalog.trash ?? [];
  const visibleQuery = search.trim().toLowerCase();
  const filteredSkills = visibleQuery
    ? activeSkills.filter((entry) => `${entry.name} ${entry.directory} ${entry.source}`.toLowerCase().includes(visibleQuery))
    : activeSkills;
  const filteredTrash = visibleQuery
    ? trashedSkills.filter((entry) => `${entry.name} ${entry.directory}`.toLowerCase().includes(visibleQuery))
    : trashedSkills;

  useEffect(() => {
    if (selection.kind === 'create' || selection.kind === 'list') return;
    const existsInSkills = activeSkills.some((entry) => entry.id === selection.id);
    const existsInTrash = trashedSkills.some((entry) => entry.id === selection.id);
    if (!existsInSkills && !existsInTrash) {
      setSelection({ kind: 'list' });
    }
  }, [activeSkills, selection, trashedSkills]);

  useEffect(() => {
    if (selection.kind !== 'create' || !document || !creatingSkill.current || document.name !== draftCreateName.trim()) return;
    if (activeSkills.some((entry) => entry.id === document.id)) {
      creatingSkill.current = false;
      setDraftCreateName('');
      setDraftCreateContent('---\ndescription: \n---\n');
      setSelection({ kind: 'skill', id: document.id });
    }
  }, [activeSkills, document, selection.kind, draftCreateName]);

  const selectedSkill = selection.kind === 'skill' ? activeSkills.find((entry) => entry.id === selection.id) ?? null : null;
  const selectedTrash = selection.kind === 'trash' ? trashedSkills.find((entry) => entry.id === selection.id) ?? null : null;
  const selectedDocument = selectedSkill && document?.id === selectedSkill.id ? document : null;

  useEffect(() => {
    if (!selectedSkill || !adminAvailable) return;
    if (document?.id === selectedSkill.id) return;
    setLoadingDocument(true);
    setPageError(null);
    void callSkillAdmin(bridge, { action: 'get_document', target: selectedSkill.id })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法读取 skill 文档。'))
      .finally(() => setLoadingDocument(false));
  }, [adminAvailable, bridge.skillAdmin, document?.id, selectedSkill]);

  useEffect(() => {
    if (!selectedDocument) return;
    setDraftContent(selectedDocument.markdown);
    setMoveScope(selectedDocument.source === 'project' ? 'user' : 'project');
    setMoveName(selectedDocument.name);
  }, [selectedDocument]);

  useEffect(() => {
    if (!selectedTrash) return;
    setRestoreScope(defaultCreateScope(editingLayer));
    setRestoreName(selectedTrash.name);
    setPurgeConfirmId(null);
  }, [editingLayer, selectedTrash]);

  const skillDirty = selectedDocument
    ? draftContent !== selectedDocument.markdown
      || moveName !== selectedDocument.name
      || moveScope !== (selectedDocument.source === 'project' ? 'user' : 'project')
    : false;
  const trashDirty = selectedTrash
    ? restoreName !== selectedTrash.name || restoreScope !== defaultCreateScope(editingLayer)
    : false;
  const detailDirty = selection.kind === 'create'
    ? draftCreateName.trim().length > 0 || draftCreateContent !== '---\ndescription: \n---\n'
    : selection.kind === 'skill'
      ? skillDirty
      : selection.kind === 'trash'
        ? trashDirty
        : false;

  const requestSelection = (next: SkillSelection) => {
    if (detailDirty) {
      setPendingSelection(next);
      return;
    }
    creatingSkill.current = false;
    setPageError(null);
    setPendingSelection(null);
    setSelection(next);
  };

  const discardDrafts = () => {
    if (selectedDocument) {
      setDraftContent(selectedDocument.markdown);
      const resetDocument = selectedDocument;
      setMoveScope(resetDocument.source === 'project' ? 'user' : 'project');
      setMoveName(resetDocument.name);
    }
    if (selectedTrash) {
      const resetTrash = selectedTrash;
      setRestoreName(resetTrash.name);
      setRestoreScope(defaultCreateScope(editingLayer));
    }
    creatingSkill.current = false;
    setDraftCreateName('');
    setDraftCreateScope(defaultCreateScope(editingLayer));
    setDraftCreateContent('---\ndescription: \n---\n');
    setPendingSelection(null);
  };

  const syncHasChanges = syncDraft !== syncEnabled;

  const handleReload = () => {
    setReloading(true);
    setPageError(null);
    void bridge.runSlashCommand('/reload-skills')
      .then(async () => {
        await bridge.refreshSkills();
        if (adminAvailable) await callSkillAdmin(bridge, { action: 'get_catalog' });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法重新加载 skills。'))
      .finally(() => setReloading(false));
  };

  const handleSaveSkill = () => {
    if (!selectedDocument) return;
    const scope = sourceScope(selectedDocument.source);
    if (!scope) return;
    setPageError(null);
    void callSkillAdmin(bridge, {
      action: 'save_document',
      operation_id: nextConfigurationOperationId(),
      target: selectedDocument.id,
      scope,
      revision: selectedDocument.revision,
      payload_json: JSON.stringify({ name: selectedDocument.name, markdown: draftContent }),
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存 skill。'));
  };

  const handleCreateSkill = () => {
    if (!draftCreateName.trim()) {
      setPageError('需要一个 skill 名称。');
      return;
    }
    setPageError(null);
    creatingSkill.current = true;
    void callSkillAdmin(bridge, {
      action: 'create_skill',
      operation_id: nextConfigurationOperationId(),
      scope: draftCreateScope,
      revision: EMPTY_SHA256,
      payload_json: JSON.stringify({
        name: draftCreateName.trim(),
        markdown: draftCreateContent,
      }),
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法创建 skill。'));
  };

  const handleMoveSkill = () => {
    if (!selectedDocument) return;
    setPageError(null);
    void callSkillAdmin(bridge, {
      action: 'move_skill',
      operation_id: nextConfigurationOperationId(),
      target: selectedDocument.id,
      scope: moveScope,
      revision: selectedDocument.revision,
      payload_json: JSON.stringify({ name: moveName.trim() || selectedDocument.name }),
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法移动 skill。'));
  };

  const handleTrashSkill = () => {
    if (!selectedDocument) return;
    setPageError(null);
    void callSkillAdmin(bridge, {
      action: 'trash_skill',
      operation_id: nextConfigurationOperationId(),
      target: selectedDocument.id,
      revision: selectedDocument.revision,
      payload_json: '{}',
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法移到回收区。'));
  };

  const handleRestoreSkill = () => {
    if (!selectedTrash?.trashId) return;
    setPageError(null);
    void callSkillAdmin(bridge, {
      action: 'restore_skill',
      operation_id: nextConfigurationOperationId(),
      target: selectedTrash.trashId,
      scope: restoreScope,
      revision: selectedTrash.revision ?? EMPTY_SHA256,
      payload_json: JSON.stringify({ name: restoreName.trim() || selectedTrash.name }),
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法恢复 skill。'));
  };

  const handlePurgeSkill = () => {
    if (!selectedTrash?.trashId) return;
    if (purgeConfirmId !== selectedTrash.trashId) {
      setPurgeConfirmId(selectedTrash.trashId);
      return;
    }
    setPageError(null);
    void callSkillAdmin(bridge, {
      action: 'purge_trash_skill',
      operation_id: nextConfigurationOperationId(),
      target: selectedTrash.trashId,
      revision: selectedTrash.revision ?? EMPTY_SHA256,
      payload_json: JSON.stringify({ confirmed: true }),
    })
      .then(() => setPurgeConfirmId(null))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法永久删除 skill。'));
  };

  const detail = selection.kind === 'create'
    ? (
      <div style={{ ...managerDetailStyle(), padding: 0 }}>
        <div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
            <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>新建 Skill</div>
            <SourcePill t={t} label={draftCreateScope === 'project' ? '项目' : '用户'} />
          </div>
          <div style={secondaryMetaStyle(t)}>会创建目录与 SKILL.md。其它附件文件仍需在文件系统里处理。</div>
        </div>
        <div style={detailGridStyle()}>
          <Field t={t} label="作用域">
            <select value={draftCreateScope} onChange={(event) => setDraftCreateScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label="skill-create-scope">
              <option value="user">用户</option>
              <option value="project">项目</option>
            </select>
          </Field>
          <Field t={t} label="名称">
            <input value={draftCreateName} onChange={(event) => setDraftCreateName(event.target.value)} style={inputStyle(t)} aria-label="skill-create-name" placeholder="my-skill" />
          </Field>
        </div>
        <Field t={t} label="SKILL.md">
          <textarea value={draftCreateContent} onChange={(event) => setDraftCreateContent(event.target.value)} rows={14} style={textareaStyle(t, 14)} aria-label="skill-create-content" />
        </Field>
        <div style={{ display: 'flex', gap: 8 }}>
          <button type="button" onClick={handleCreateSkill} style={ghostButtonStyle(t)}>创建</button>
          <button type="button" onClick={() => requestSelection({ kind: 'list' })} style={ghostButtonStyle(t, false, true)}>取消</button>
        </div>
      </div>
    )
    : selection.kind === 'trash' && selectedTrash
      ? (
        <div style={{ ...managerDetailStyle(), padding: 0 }}>
          <div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
              <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{selectedTrash.name}</div>
              <SourcePill t={t} label="回收区" tone="warn" />
            </div>
            <div className="mono" style={secondaryMetaStyle(t)}>{selectedTrash.directory}</div>
            {selectedTrash.trashedAt && <div style={secondaryMetaStyle(t)}>删除批次：{selectedTrash.trashedAt}</div>}
          </div>
          <div style={noteStyle(t, 'warn')}>
            目录仍保留在 skill 回收区中。恢复会移动回用户或项目作用域；永久删除会直接清除该回收区条目。
          </div>
          {purgeConfirmId === selectedTrash.trashId && (
            <div role="alert" style={noteStyle(t, 'danger')}>
              此操作不可恢复。请再次点击“确认永久删除”。
            </div>
          )}
          <div style={detailGridStyle()}>
            <Field t={t} label="恢复到">
              <select value={restoreScope} onChange={(event) => setRestoreScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label="skill-restore-scope">
                <option value="user">用户</option>
                <option value="project">项目</option>
              </select>
            </Field>
            <Field t={t} label="恢复名称">
              <input value={restoreName} onChange={(event) => setRestoreName(event.target.value)} style={inputStyle(t)} aria-label="skill-restore-name" />
            </Field>
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <button type="button" onClick={handleRestoreSkill} disabled={!selectedTrash.trashId} style={ghostButtonStyle(t, !selectedTrash.trashId)}>恢复</button>
            <button type="button" onClick={handlePurgeSkill} disabled={!selectedTrash.trashId} style={ghostButtonStyle(t, !selectedTrash.trashId, true)}>
              {purgeConfirmId === selectedTrash.trashId ? '确认永久删除' : '永久删除'}
            </button>
          </div>
        </div>
      )
      : selectedSkill
        ? (
          <div style={{ ...managerDetailStyle(), padding: 0 }}>
            <div>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, alignItems: 'center', marginBottom: 6 }}>
                <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{selectedSkill.name}</div>
                <SourcePill t={t} label={selectedSkill.source} />
                {selectedSkill.pluginOwner && <SourcePill t={t} label={selectedSkill.pluginOwner} />}
                {!selectedSkill.writable && <SourcePill t={t} label="只读" tone="warn" />}
                {selectedSkill.parseError && <SourcePill t={t} label="解析错误" tone="danger" />}
              </div>
              <div className="mono" style={secondaryMetaStyle(t)}>{selectedSkill.directory}</div>
              <div className="mono" style={secondaryMetaStyle(t)}>{selectedSkill.rootDir}</div>
              {selectedSkill.description && <div style={secondaryMetaStyle(t)}>{selectedSkill.description}</div>}
              {selectedSkill.whenToUse && <div style={secondaryMetaStyle(t)}>When to use: {selectedSkill.whenToUse}</div>}
            </div>
            {loadingDocument && <div style={noteStyle(t)}>正在读取 SKILL.md…</div>}
            {!selectedDocument && !loadingDocument && (
              <EmptyDetail t={t} title="还没有拿到文档内容" body="如果当前引擎还没接入 skill_admin get_document，这里会退回为目录级清单。" />
            )}
            {selectedDocument && (
              <>
                {selectedDocument.parseError && <div role="alert" style={noteStyle(t, 'danger')}>{selectedDocument.parseError}</div>}
                {selectedDocument.readonlyReason && <div style={noteStyle(t, 'warn')}>{selectedDocument.readonlyReason}</div>}
                <Field t={t} label="SKILL.md">
                  <textarea
                    value={draftContent}
                    onChange={(event) => setDraftContent(event.target.value)}
                    rows={16}
                    readOnly={!selectedDocument.writable}
                    aria-label={`${selectedDocument.name} markdown`}
                    style={textareaStyle(t, 16)}
                  />
                </Field>
                <div style={detailGridStyle()}>
                  <Field t={t} label="移动到">
                    <select value={moveScope} onChange={(event) => setMoveScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label="skill-move-scope" disabled={!selectedDocument.writable}>
                      <option value="user">用户</option>
                      <option value="project">项目</option>
                    </select>
                  </Field>
                  <Field t={t} label="新名称">
                    <input value={moveName} onChange={(event) => setMoveName(event.target.value)} style={inputStyle(t)} aria-label="skill-move-name" disabled={!selectedDocument.writable} />
                  </Field>
                </div>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                  <button type="button" onClick={handleSaveSkill} disabled={!selectedDocument.writable || draftContent === selectedDocument.markdown} style={ghostButtonStyle(t, !selectedDocument.writable || draftContent === selectedDocument.markdown)}>
                    保存
                  </button>
                  <button type="button" onClick={discardDrafts} disabled={!selectedDocument.writable || !detailDirty} style={ghostButtonStyle(t, !selectedDocument.writable || !detailDirty, true)}>
                    取消
                  </button>
                  <button type="button" onClick={handleMoveSkill} disabled={!selectedDocument.writable} style={ghostButtonStyle(t, !selectedDocument.writable)}>
                    移动
                  </button>
                  <button type="button" onClick={handleTrashSkill} disabled={!selectedDocument.writable} style={ghostButtonStyle(t, !selectedDocument.writable, true)}>
                    移到回收区
                  </button>
                </div>
              </>
            )}
          </div>
        )
        : <div style={{ ...managerDetailStyle(), padding: 0 }}><EmptyDetail t={t} title="没有可编辑的 skill" body="可以返回列表，或创建新的用户或项目 skill。" /></div>;

  return (
    <>
      <Card title="Skills">
        <div className="configuration-page">
          <DomainOperationBanner t={t} operation={skillOperation} fallbackDomainLabel="Skills" />
          {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
          {selection.kind === 'list' ? <div className="configuration-list">
            <div className="configuration-toolbar">
              <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索 skill 名称或路径" aria-label="搜索 skills" style={searchInputStyle(t)} />
              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" onClick={() => requestSelection({ kind: 'create' })} style={ghostButtonStyle(t)}>新建</button>
                <button type="button" onClick={handleReload} disabled={reloading} style={ghostButtonStyle(t, reloading)}>{reloading ? '重载中…' : '重新加载'}</button>
              </div>

            </div>
            <div style={sidebarListStyle()}>
              <div style={sidebarSectionTitleStyle(t)}>Skills · {filteredSkills.length}</div>
              {filteredSkills.map((entry) => (
                <button className="configuration-entry" key={entry.id} type="button" onClick={() => requestSelection({ kind: 'skill', id: entry.id })} style={sidebarButtonStyle(t, false)}>
                  <span style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap' }}>
                    <span style={{ fontSize: 13, fontWeight: 600 }}>{entry.name}</span>
                    <SourcePill t={t} label={entry.source} />
                    {!entry.writable && <SourcePill t={t} label="只读" tone="warn" />}
                  </span>
                  <span className="mono" style={{ fontSize: 11.5, color: t.text4, overflow: 'hidden', textOverflow: 'ellipsis' }}>{entry.directory}</span>
                </button>
              ))}
              {filteredSkills.length === 0 && <div style={secondaryMetaStyle(t)}>没有匹配的 skill。</div>}
              <div style={{ ...sidebarSectionTitleStyle(t), marginTop: 20 }}>回收区 · {filteredTrash.length}</div>
              {filteredTrash.map((entry) => (
                <button className="configuration-entry" key={entry.id} type="button" onClick={() => requestSelection({ kind: 'trash', id: entry.id })} style={sidebarButtonStyle(t, false)}>
                  <span style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                    <span style={{ fontSize: 13, fontWeight: 600 }}>{entry.name}</span>
                    <SourcePill t={t} label="已删除" tone="warn" />
                  </span>
                  <span className="mono" style={{ fontSize: 11.5, color: t.text4, overflow: 'hidden', textOverflow: 'ellipsis' }}>{entry.directory}</span>
                </button>
              ))}
              {filteredTrash.length === 0 && <div style={secondaryMetaStyle(t)}>回收区为空。</div>}
            </div>
          </div> : <div className="configuration-detail" style={managerDetailStyle()}>
            <button type="button" className="configuration-back" onClick={() => requestSelection({ kind: 'list' })} style={ghostButtonStyle(t)}>← 返回 Skills</button>
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                当前修改尚未保存。离开前请保存，或丢弃草稿。
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button
                    type="button"
                    onClick={() => {
                      discardDrafts();
                      setSelection(pendingSelection);
                    }}
                    style={ghostButtonStyle(t)}
                  >
                    丢弃并切换
                  </button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>继续编辑</button>
                </div>
              </div>
            )}
            {detail}
          </div>}
        </div>
      </Card>

      {selection.kind === 'list' && <Card title="Claude.ai 同步">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="syncClaudeAiSkills" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <Row title="syncClaudeAiSkills" desc={(catalog.sync_claude_ai_note ?? 'Stored only. Claude.ai cloud sync is not wired on desktop.').replace('Stored only.', '仅保存。').replace('Claude.ai cloud sync is not wired on desktop.', '当前还没有接入 Claude.ai 云端同步。')} align="center">
          <Toggle value={syncDraft} onChange={savingSync ? () => undefined : setSyncDraft} />
        </Row>
        <Row title="保存" desc="这是本页唯一按层写入的普通设置项。" align="center">
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              type="button"
              disabled={savingSync || !syncHasChanges}
              onClick={() => {
                setSavingSync(true);
                setPageError(null);
                void bridge.updateEngineSettings(editingLayer, { syncClaudeAiSkills: syncDraft })
                  .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存 syncClaudeAiSkills。'))
                  .finally(() => setSavingSync(false));
              }}
              style={ghostButtonStyle(t, savingSync || !syncHasChanges)}
            >
              保存
            </button>
            <button type="button" disabled={savingSync || !syncHasChanges} onClick={() => setSyncDraft(syncEnabled)} style={ghostButtonStyle(t, savingSync || !syncHasChanges, true)}>
              取消
            </button>
          </div>
        </Row>
      </Card>}

      {!adminAvailable && selection.kind === 'list' && <Card title="兼容视图">
        <div style={{ padding: '12px 18px', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
          当前运行时仅支持查看已发现的 skills。升级运行时后可编辑和管理。
        </div>
        {model.skills.slice(0, 3).map((skill) => (
          <Row key={skill.source_dir} align="center" title={skill.name} desc={<span className="mono" style={{ fontSize: 11.5 }}>{skill.source_dir}</span>}>{null}</Row>
        ))}
      </Card>}
    </>
  );
}
