import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { SkillAdminCommandDto, SkillDto } from '@lingxi/bridge-client';
import { ExtensionGlyph } from './ExtensionHubIcons';
import { useSettingsDraftGuard } from '../useSettingsDraftGuard';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { Toggle } from '../primitives';
import { useT } from '../../../theme/ThemeContext';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { boolFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import { ExtensionHubLayout, ExtensionHubHeader, ExtensionIcon, extensionHubStyle, useExtensionHubDialogFocus, useExtensionHubNavigation } from './ExtensionHub';
import { MarkdownContent } from '../../MarkdownContent';
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
  secondaryMetaStyle,
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
      readonlyReason: tr('desktop_extension_no_document_description'),
    })),
    trash: [],
    sync_claude_ai_note: tr('desktop_extension_sync_unavailable'),
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

export function Skills({ bridge, snapshot, editingLayer, onJumpToLayer, onNavigate, onClose, onRegisterNavigationRequest, onRegisterCloseRequest, linkRequest }: PageContentProps) {
  const t = useT();
  const { locale } = useI18n();
  const model = skillsPageModel({ skills: bridge.skillsEvent?.skills });
  const catalog = useMemo(() => skillCatalogFromBridge(bridge), [bridge.skillCatalogEvent?.catalog_json, bridge.skillsEvent?.skills, locale]);
  const document = useMemo(() => skillDocumentFromBridge(bridge), [bridge.skillDocumentEvent?.document_json]);
  const adminAvailable = typeof (bridge as { skillAdmin?: unknown }).skillAdmin === 'function';
  const skillOperation = bridge.configurationOperations?.skill ?? null;
  const syncEnabled = boolFromLayer(snapshot, editingLayer, 'syncClaudeAiSkills');

  const [selection, setSelection] = useState<SkillSelection>({ kind: 'list' });
  const appliedLink = useRef<string>();
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
  const [pendingHubPage, setPendingHubPage] = useState<{ pageId: string; pluginId?: string } | null>(null);
  const [sourceFilter, setSourceFilter] = useState('user');
  const [showAllInstalled, setShowAllInstalled] = useState(false);
  const [showAllSource, setShowAllSource] = useState(false);
  const [editingDocument, setEditingDocument] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [tryingSkill, setTryingSkill] = useState(false);
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
    ? activeSkills.filter((entry) => `${entry.name} ${entry.description ?? ''} ${entry.whenToUse ?? ''} ${entry.directory} ${entry.source}`.toLowerCase().includes(visibleQuery))
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

  useEffect(() => {
    if (linkRequest?.kind !== 'skill' || appliedLink.current === linkRequest.requestId) return;
    if (linkRequest.payload) {
      appliedLink.current = linkRequest.requestId;
      setSelection({ kind: 'create' });
      setDraftCreateName(asString(linkRequest.payload.name));
      setDraftCreateContent(asString(linkRequest.payload.markdown ?? linkRequest.payload.content));
      setDraftCreateScope(linkRequest.scope === 'project' ? 'project' : 'user');
    } else if (bridge.skillCatalogEvent || bridge.skillsEvent) {
      appliedLink.current = linkRequest.requestId;
      const matches = activeSkills.filter(skill => (skill.id === linkRequest.id || skill.name === linkRequest.id) && (!linkRequest.scope || skill.source === linkRequest.scope));
      if (matches.length === 1) setSelection({ kind: 'skill', id: matches[0]!.id });
      else setPageError(matches.length ? tr('desktop_extension_skill_ambiguous') : tr('desktop_extension_skill_unavailable'));
    }
  }, [linkRequest, activeSkills, bridge.skillCatalogEvent, bridge.skillsEvent]);
  const selectedSkill = selection.kind === 'skill' ? activeSkills.find((entry) => entry.id === selection.id) ?? null : null;
  const selectedTrash = selection.kind === 'trash' ? trashedSkills.find((entry) => entry.id === selection.id) ?? null : null;
  const selectedDocument = selectedSkill && document?.id === selectedSkill.id ? document : null;
  const uninstallDisabledReason = selectedDocument?.writable
    ? undefined
    : selectedDocument?.readonlyReason || (loadingDocument
      ? tr('desktop_extension_skill_reading_uninstall')
      : selectedDocument
        ? tr('desktop_extension_skill_readonly_uninstall')
        : tr('desktop_extension_skill_unreadable_uninstall'));

  useEffect(() => {
    if (!selectedSkill || !adminAvailable) return;
    if (document?.id === selectedSkill.id) return;
    setLoadingDocument(true);
    setPageError(null);
    void callSkillAdmin(bridge, { action: 'get_document', target: selectedSkill.id })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_read_failed')))
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

  const closePrompt = useSettingsDraftGuard({ onClose, onNavigate, onRegisterCloseRequest }, detailDirty || syncDraft !== syncEnabled);
  const requestSelection = (next: SkillSelection) => {
    if (detailDirty) {
      setPendingSelection(next);
      return;
    }
    creatingSkill.current = false;
    setPageError(null);
    setPendingSelection(null);
    setEditingDocument(false);
    setSelection(next);
  };

  const dialogRef = useExtensionHubDialogFocus(selection.kind !== 'list', () => requestSelection({ kind: 'list' }));

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
  const requestHubNavigation = (pageId: string, pluginId?: string) => {
    if (detailDirty || syncHasChanges) {
      setPendingHubPage({ pageId, pluginId });
      return;
    }
    onNavigate(pageId, pluginId);
  };
  useExtensionHubNavigation(onRegisterNavigationRequest, requestHubNavigation);
  const discardAndNavigate = () => {
    if (!pendingHubPage) return;
    discardDrafts();
    setSyncDraft(syncEnabled);
    setSelection({ kind: 'list' });
    onNavigate(pendingHubPage.pageId, pendingHubPage.pluginId);
    setPendingHubPage(null);
  };
  const navigationPrompt = pendingHubPage && (
    <div role="alert" style={noteStyle(t, 'warn')}>
      {tr('desktop_extension_unsaved_skill')} <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <button type="button" onClick={discardAndNavigate} style={ghostButtonStyle(t)}>{tr('desktop_extension_discard_switch')}</button>
        <button type="button" onClick={() => setPendingHubPage(null)} style={ghostButtonStyle(t, false, true)}>{tr('desktop_extension_continue_editing')}</button>
      </div>
    </div>
  );

  const handleReload = () => {
    setReloading(true);
    setPageError(null);
    void bridge.runSlashCommand('/reload-skills', [])
      .then(async (accepted) => {
        if (!accepted) return;
        await bridge.refreshSkills();
        if (adminAvailable) await callSkillAdmin(bridge, { action: 'get_catalog' });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_reload_failed')))
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
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_save_failed')));
  };

  const handleCreateSkill = () => {
    if (!draftCreateName.trim()) {
      setPageError(tr('desktop_extension_skill_name_required'));
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
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_create_failed')));
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
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_move_failed')));
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
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_trash_failed')));
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
    }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_restore_failed')));
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
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_purge_failed')));
  };

  const detail = selection.kind === 'create'
    ? (
      <div style={{ ...managerDetailStyle(), padding: 0 }}>
        <div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
            <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{tr('desktop_extension_new_skill')}</div>
            <SourcePill t={t} label={draftCreateScope === 'project' ? tr('cron_run_project_label') : tr('settings_parity_user_layer')} />
          </div>
          <div style={secondaryMetaStyle(t)}>{tr('desktop_extension_create_skill_description')}</div>
        </div>
        <div style={detailGridStyle()}>
          <Field t={t} label={tr('cron_scope_label')}>
            <select value={draftCreateScope} onChange={(event) => setDraftCreateScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label={tr('cron_scope_label')}>
              <option value="user">{tr('settings_parity_user_layer')}</option>
              <option value="project">{tr('cron_run_project_label')}</option>
            </select>
          </Field>
          <Field t={t} label={tr('onboarding_done_row_name')}>
            <input value={draftCreateName} onChange={(event) => setDraftCreateName(event.target.value)} style={inputStyle(t)} aria-label={tr('onboarding_done_row_name')} placeholder="my-skill" />
          </Field>
        </div>
        <Field t={t} label="SKILL.md">
          <textarea value={draftCreateContent} onChange={(event) => setDraftCreateContent(event.target.value)} rows={14} style={textareaStyle(t, 14)} aria-label="SKILL.md" />
        </Field>
        <div style={{ display: 'flex', gap: 8 }}>
          <button type="button" onClick={handleCreateSkill} style={ghostButtonStyle(t)}>{tr('common_create')}</button>
          <button type="button" onClick={() => requestSelection({ kind: 'list' })} style={ghostButtonStyle(t, false, true)}>{tr('common_cancel')}</button>
        </div>
      </div>
    )
    : selection.kind === 'trash' && selectedTrash
      ? (
        <div style={{ ...managerDetailStyle(), padding: 0 }}>
          <div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
              <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{selectedTrash.name}</div>
              <SourcePill t={t} label={tr('desktop_extension_trash')} tone="warn" />
            </div>
            <div className="mono" style={secondaryMetaStyle(t)}>{selectedTrash.directory}</div>
            {selectedTrash.trashedAt && <div style={secondaryMetaStyle(t)}>{tr('desktop_extension_deleted_batch', selectedTrash.trashedAt)}</div>}
          </div>
          <div style={noteStyle(t, 'warn')}>
            {tr('desktop_extension_trash_description')}</div>
          {purgeConfirmId === selectedTrash.trashId && (
            <div role="alert" style={noteStyle(t, 'danger')}>
              {tr('desktop_extension_purge_warning')}</div>
          )}
          <div style={detailGridStyle()}>
            <Field t={t} label={tr('desktop_extension_restore_to')}>
              <select value={restoreScope} onChange={(event) => setRestoreScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label={tr('desktop_extension_restore_to')}>
                <option value="user">{tr('settings_parity_user_layer')}</option>
                <option value="project">{tr('cron_run_project_label')}</option>
              </select>
            </Field>
            <Field t={t} label={tr('desktop_extension_restore_name')}>
              <input value={restoreName} onChange={(event) => setRestoreName(event.target.value)} style={inputStyle(t)} aria-label={tr('desktop_extension_restore_name')} />
            </Field>
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <button type="button" onClick={handleRestoreSkill} disabled={!selectedTrash.trashId} style={ghostButtonStyle(t, !selectedTrash.trashId)}>{tr('settings_parity_restore')}</button>
            <button type="button" onClick={handlePurgeSkill} disabled={!selectedTrash.trashId} style={ghostButtonStyle(t, !selectedTrash.trashId, true)}>
              {purgeConfirmId === selectedTrash.trashId ? tr('desktop_extension_confirm_purge') : tr('desktop_extension_purge')}
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
                {!selectedSkill.writable && <SourcePill t={t} label={tr('chat_agent_read_only')} tone="warn" />}
                {selectedSkill.parseError && <SourcePill t={t} label={tr('desktop_extension_parse_error')} tone="danger" />}
              </div>
              <div className="mono" style={secondaryMetaStyle(t)}>{selectedSkill.directory}</div>
              <div className="mono" style={secondaryMetaStyle(t)}>{selectedSkill.rootDir}</div>
              {selectedSkill.description && <div style={secondaryMetaStyle(t)}>{selectedSkill.description}</div>}
              {selectedSkill.whenToUse && <div style={secondaryMetaStyle(t)}>{tr('desktop_extension_when_to_use')} {selectedSkill.whenToUse}</div>}
            </div>
            {loadingDocument && <div style={noteStyle(t)}>{tr('desktop_extension_loading_skill')}</div>}
            {!selectedDocument && !loadingDocument && (
              <EmptyDetail t={t} title={tr('desktop_extension_no_document')} body={tr('desktop_extension_no_document_description')} />
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
                    aria-label={tr('desktop_extension_markdown_named', selectedDocument.name)}
                    style={textareaStyle(t, 16)}
                  />
                </Field>
                <div style={detailGridStyle()}>
                  <Field t={t} label={tr('desktop_extension_move_to')}>
                    <select value={moveScope} onChange={(event) => setMoveScope(event.target.value as 'user' | 'project')} style={inputStyle(t)} aria-label={tr('desktop_extension_move_to')} disabled={!selectedDocument.writable}>
                      <option value="user">{tr('settings_parity_user_layer')}</option>
                      <option value="project">{tr('cron_run_project_label')}</option>
                    </select>
                  </Field>
                  <Field t={t} label={tr('desktop_extension_new_name')}>
                    <input value={moveName} onChange={(event) => setMoveName(event.target.value)} style={inputStyle(t)} aria-label={tr('desktop_extension_new_name')} disabled={!selectedDocument.writable} />
                  </Field>
                </div>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                  <button type="button" onClick={handleSaveSkill} disabled={!selectedDocument.writable || draftContent === selectedDocument.markdown} style={ghostButtonStyle(t, !selectedDocument.writable || draftContent === selectedDocument.markdown)}>
                    {tr('settings_save')}
                  </button>
                  <button type="button" onClick={discardDrafts} disabled={!selectedDocument.writable || !detailDirty} style={ghostButtonStyle(t, !selectedDocument.writable || !detailDirty, true)}>
                    {tr('common_cancel')}
                  </button>
                  <button type="button" onClick={handleMoveSkill} disabled={!selectedDocument.writable} style={ghostButtonStyle(t, !selectedDocument.writable)}>
                    {tr('desktop_extension_move')}</button>
                  <button type="button" onClick={handleTrashSkill} disabled={!selectedDocument.writable} style={ghostButtonStyle(t, !selectedDocument.writable, true)}>
                    {tr('desktop_extension_move_trash')}</button>
                </div>
              </>
            )}
          </div>
        )
        : <div style={{ ...managerDetailStyle(), padding: 0 }}><EmptyDetail t={t} title={tr('desktop_extension_no_editable_skill')} body={tr('desktop_extension_no_editable_skill_description')} /></div>;

  const detailTitle = selection.kind === 'create'
    ? tr('desktop_extension_new_skill')
    : selection.kind === 'trash'
      ? selectedTrash?.name ?? tr('desktop_extension_deleted_skill')
      : selectedSkill?.name ?? tr('desktop_extension_skill_details');

  const sourceLabels: Record<string, string> = { user: tr('desktop_personal'), project: tr('cron_run_project_label'), builtin: tr('settings_voice_backend_system'), system: tr('settings_voice_backend_system'), plugin: tr('desktop_extension_plugins'), managed: tr('settings_parity_managed_layer') };
  const sources = [...new Set(activeSkills.map((entry) => entry.source))];
  const activeSource = sources.includes(sourceFilter) ? sourceFilter : sources[0];
  const sourceSkills = filteredSkills.filter((entry) => entry.source === activeSource);
  const skillRows = (entries: SkillCatalogItem[]) => <div className="extension-hub-list">{entries.map((entry) => <button className="extension-hub-row extension-hub-row-open" type="button" key={entry.id} onClick={() => requestSelection({ kind: 'skill', id: entry.id })}>
    <ExtensionIcon kind="skill" />
    <span className="extension-hub-row-copy"><span className="extension-hub-row-title">{entry.name}</span><span className="extension-hub-row-description">{entry.description || entry.whenToUse || tr('desktop_extension_skill_fallback')}</span></span>
    <span className="extension-skill-installed" aria-label={entry.parseError ? tr('desktop_apps_attention') : tr('apps_installed')}><ExtensionGlyph name={entry.parseError ? 'circleAlert' : 'check'} size={18} /></span>
  </button>)}</div>;
  const skillDescription = selectedSkill?.description || selectedSkill?.whenToUse;
  const previewMarkdown = selectedDocument?.markdown.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, '').trim() ?? '';

  return (
    <ExtensionHubLayout>
      {closePrompt}
      {pageError && selection.kind === 'list' && <div role="alert">{pageError}</div>}
      {linkRequest?.payload && selection.kind === 'create' && <p role="status">{tr('desktop_extension_skill_import')}</p>}
      <ExtensionHubHeader title={tr('settings_title_skills')} description={tr('desktop_extension_skill_description')} search={search} onSearch={setSearch} actions={<>
        <button type="button" onClick={handleReload} disabled={reloading} className="extension-hub-icon-button" aria-label={reloading ? tr('desktop_extension_reloading_skills') : tr('desktop_extension_reload_skills')}><ExtensionGlyph name="refresh" size={16} /></button>
        <button type="button" onClick={() => setShowSettings(!showSettings)} className="extension-hub-icon-button" aria-label={tr('desktop_extension_skill_settings')} aria-expanded={showSettings}><ExtensionGlyph name="cog" size={16} /></button>
        <button type="button" onClick={() => requestSelection({ kind: 'create' })} className="extension-hub-primary">{tr('workspace_confirm_add')} <ExtensionGlyph name="plus" size={14} /></button>
      </>} />
      <DomainOperationBanner t={t} operation={skillOperation} fallbackDomainLabel={tr('settings_title_skills')} />
      {pageError && selection.kind === 'list' && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
      {selection.kind === 'list' && navigationPrompt}
      <section className="extension-catalog-section extension-skills-installed">
        <h2 className="extension-hub-group-title">{tr('apps_installed')}</h2>
        {skillRows(showAllInstalled || search ? filteredSkills : filteredSkills.slice(0, 6))}
        {!filteredSkills.length && <div className="extension-hub-empty">{search ? tr('desktop_extension_no_skill_results') : tr('desktop_extension_first_skill')}</div>}
        {filteredSkills.length > 6 && !search && <button type="button" className="extension-see-more" onClick={() => setShowAllInstalled(!showAllInstalled)}>{showAllInstalled ? tr('chat_tool_show_less') : tr('desktop_extension_more_skills', filteredSkills.length - 6)}</button>}
      </section>
      {sources.length > 0 && <section className="extension-catalog-section extension-skills-sources">
        <nav className="extension-catalog-tabs" aria-label={tr('desktop_extension_skill_sources')}>{sources.map((source) => <button key={source} type="button" aria-pressed={activeSource === source} onClick={() => { setSourceFilter(source); setShowAllSource(false); }}>{sourceLabels[source] ?? source}</button>)}</nav>
        {skillRows(showAllSource || search ? sourceSkills : sourceSkills.slice(0, 6))}
        {!sourceSkills.length && <div className="extension-hub-empty">{tr('desktop_extension_no_source_skills')}</div>}
        {sourceSkills.length > 6 && !search && <button type="button" className="extension-see-more" onClick={() => setShowAllSource(!showAllSource)}>{showAllSource ? tr('chat_tool_show_less') : tr('desktop_extension_more_skills', sourceSkills.length - 6)}</button>}
      </section>}
      {filteredTrash.length > 0 && <details className="extension-hub-marketplaces">
        <summary>{tr('desktop_extension_trash_count', filteredTrash.length)}</summary>
        <div className="extension-hub-list">
          {filteredTrash.map((entry) => (
            <button className="extension-hub-row extension-hub-row-open" key={entry.id} type="button" onClick={() => requestSelection({ kind: 'trash', id: entry.id })}>
              <span className="extension-hub-icon"><ExtensionGlyph name="archive" size={18} color={t.text3} /></span>
              <span className="extension-hub-row-copy"><span className="extension-hub-row-title">{entry.name}</span><span className="extension-hub-row-description">{entry.directory}</span></span>
            </button>
          ))}
        </div>
      </details>}

      {selection.kind !== 'list' && <div className="extension-detail-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) requestSelection({ kind: 'list' }); }}>
        <section ref={dialogRef} tabIndex={-1} className="extension-detail-dialog extension-skill-dialog" role="dialog" aria-modal="true" aria-label={tr('desktop_extension_details_named', detailTitle)} style={extensionHubStyle(t)}>
          <header className="extension-detail-head extension-skill-head">
            <div className="extension-skill-topline"><ExtensionIcon kind="skill" /><div className="extension-hub-toolbar-actions">
              {selectedSkill && <details className="extension-hub-menu"><summary className="extension-hub-icon-button" aria-label={tr('desktop_extension_skill_actions')}><ExtensionGlyph name="more" size={16} /></summary><div className="extension-hub-menu-popover"><button type="button" disabled={!selectedDocument} onClick={(event) => { setEditingDocument(true); event.currentTarget.closest('details')?.removeAttribute('open'); }}>{selectedDocument?.writable ? tr('desktop_extension_edit_skill') : tr('desktop_extension_view_source')}</button></div></details>}
              <button type="button" className="extension-hub-icon-button" data-dialog-initial-focus aria-label={tr('desktop_extension_close_details')} onClick={() => requestSelection({ kind: 'list' })}><ExtensionGlyph name="x" size={16} /></button>
            </div></div>
            <div className="extension-detail-title-line"><h2 className="extension-detail-title">{detailTitle}</h2><span className="extension-detail-kind">{tr('chat_tool_verb_skill')}</span></div>
            {skillDescription && <p className="extension-skill-description">{skillDescription}</p>}
          </header>
          <div className="extension-detail-content">
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            <DomainOperationBanner t={t} operation={skillOperation} fallbackDomainLabel={tr('settings_title_skills')} />
            {navigationPrompt}
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                {tr('desktop_extension_unsaved_changes')} <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { discardDrafts(); setSelection(pendingSelection); }} style={ghostButtonStyle(t)}>{tr('desktop_extension_discard_switch')}</button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>{tr('desktop_extension_continue_editing')}</button>
                </div>
              </div>
            )}
            {selection.kind === 'skill' && !editingDocument ? <>
              {selectedDocument?.parseError && <div role="alert" style={noteStyle(t, 'danger')}>{selectedDocument.parseError}</div>}
              <div className="extension-skill-preview">{loadingDocument ? <div role="status">{tr('desktop_extension_loading_skill')}</div> : selectedDocument ? <MarkdownContent text={previewMarkdown || tr('desktop_extension_no_instructions')} /> : <div className="extension-hub-empty">{tr('desktop_extension_skill_doc_unavailable')}</div>}</div>
              {uninstallDisabledReason && <div id="skill-uninstall-readonly-reason" style={noteStyle(t, 'warn')}>
                {uninstallDisabledReason}
              </div>}
            </> : detail}
          </div>
          {selection.kind === 'skill' && !editingDocument && <footer className="extension-detail-footer">
            <button type="button" className="extension-hub-danger" disabled={!selectedDocument?.writable} aria-describedby={uninstallDisabledReason ? 'skill-uninstall-readonly-reason' : undefined} onClick={handleTrashSkill}>{tr('apps_action_uninstall')}</button>
            <button type="button" className="extension-hub-primary" disabled={tryingSkill || !!selectedSkill?.parseError || !selectedDocument} onClick={() => {
              if (!selectedSkill) return;
              setTryingSkill(true);
              void bridge.runSlashCommand(`/${selectedSkill.name}`, []).then((accepted) => { if (accepted) onClose(); }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_skill_start_failed'))).finally(() => setTryingSkill(false));
            }}><ExtensionGlyph name="chat" size={16} />{tr('desktop_extension_try_now')}</button>
          </footer>}
        </section>
      </div>}

      {showSettings && selection.kind === 'list' && <details open className="extension-hub-secondary-settings">
        <summary>{tr('desktop_extension_claude_sync')}</summary>
        <Card>
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="syncClaudeAiSkills" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <Row title={tr('desktop_extension_claude_sync')} desc={catalog.sync_claude_ai_note && catalog.sync_claude_ai_note !== 'Stored only. Claude.ai cloud sync is not wired on desktop.' ? catalog.sync_claude_ai_note : tr('desktop_extension_sync_unavailable')} align="center">
            <Toggle value={syncDraft} onChange={setSyncDraft} label={tr('desktop_extension_claude_sync')} disabled={savingSync} />
          </Row>
          <Row title={tr('settings_save')} desc={tr('desktop_extension_layered_sync')} align="center">
            <div style={{ display: 'flex', gap: 8 }}>
              <button
                type="button"
                disabled={savingSync || !syncHasChanges}
                onClick={() => {
                  setSavingSync(true);
                  setPageError(null);
                  void bridge.updateEngineSettings(editingLayer, { syncClaudeAiSkills: syncDraft })
                    .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_sync_save_failed')))
                    .finally(() => setSavingSync(false));
                }}
                style={ghostButtonStyle(t, savingSync || !syncHasChanges)}
              >
                {tr('settings_save')}
              </button>
              <button type="button" disabled={savingSync || !syncHasChanges} onClick={() => setSyncDraft(syncEnabled)} style={ghostButtonStyle(t, savingSync || !syncHasChanges, true)}>
                {tr('common_cancel')}
              </button>
            </div>
          </Row>
        </Card>
      </details>}

      {!adminAvailable && selection.kind === 'list' && <details className="extension-hub-secondary-settings">
        <summary>{tr('desktop_extension_compatibility_view')}</summary>
        <Card>
          <div style={{ padding: '12px 18px', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
            {tr('desktop_extension_readonly_skills')}</div>
          {model.skills.slice(0, 3).map((skill) => (
            <Row key={skill.source_dir} align="center" title={skill.name} desc={<span className="mono" style={{ fontSize: 11.5 }}>{skill.source_dir}</span>}>{null}</Row>
          ))}
        </Card>
      </details>}
    </ExtensionHubLayout>
  );
}
