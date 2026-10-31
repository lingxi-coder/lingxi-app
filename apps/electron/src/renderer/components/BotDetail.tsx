import { useCallback, useEffect, useState } from 'react';
import type { BotUi, BotsApi } from '../../shared/bots';
import { tr } from '../i18n/locale';
import { botDraftProblems, botPeers, hasBotDraftProblems, ipcErrorMessage } from '../bridge/botForm';
import { BotEditorController, botEditorDirty, type BotEditorState } from '../bridge/botEditor';
import { BotMemoryController } from '../bridge/botMemory';
import { BotRoutinesController, routineScopeOf } from '../bridge/botRoutines';
import { BotSkillsController } from '../bridge/botSkills';
import { BotTasksController, type BotTasksState } from '../bridge/botTasks';
import { relativeActivity, removeBot, startBotChat, type BotActivity } from '../bridge/botSession';
import { botRemoteAvailability, useRemoteControl } from '../bridge/remoteControl';
import type { BotsController } from '../bridge/botsStore';
import type { UseBridge } from '../bridge/bridgeTypes';
import { BotChatsPanel, BotMemoryPanel, BotRoutinesPanel, BotSkillsPanel, BotTasksPanel, type BotChats } from './BotPanels';
import {
  BotAcceptField,
  BotConfirm,
  BotFoldersField,
  BotGeneralFields,
  BotPermissionField,
  BotPersonaField,
  BotRemoteField,
  useObservable,
  type ModelChoices,
} from './BotFields';

export type BotTab = 'chats' | 'routines' | 'tasks' | 'general' | 'persona' | 'memory' | 'skills' | 'folders' | 'permissions';

const TABS: readonly BotTab[] = ['chats', 'routines', 'tasks', 'general', 'persona', 'memory', 'skills', 'folders', 'permissions'];
/** The tabs whose fields are one draft, saved together. */
const DRAFT_TABS: ReadonlySet<BotTab> = new Set<BotTab>(['general', 'persona', 'folders', 'permissions']);

function tabLabel(tab: BotTab): string {
  switch (tab) {
    case 'chats': return tr('bots_tab_chats');
    case 'routines': return tr('bots_tab_routines');
    case 'tasks': return tr('bots_tab_tasks');
    case 'general': return tr('bots_tab_general');
    case 'persona': return tr('bots_tab_persona');
    case 'memory': return tr('bots_tab_memory');
    case 'skills': return tr('bots_tab_skills');
    case 'folders': return tr('bots_tab_folders');
    case 'permissions': return tr('bots_tab_permissions');
  }
}

export function activityLabel(activity: BotActivity): string {
  return activity === 'working' ? tr('bots_state_working') : activity === 'waiting' ? tr('bots_state_waiting') : tr('bots_state_idle');
}

export function BotStateChip({ activity }: { activity: BotActivity }) {
  return <span className="bots-state" data-state={activity}>{activityLabel(activity)}</span>;
}

/** Where saving the draft stands, with the way out of a conflict. */
export function BotSaveBar({ state, dirty, problems, onSave, onReload }: {
  state: Pick<BotEditorState, 'save'>;
  dirty: boolean;
  problems: boolean;
  onSave(): void;
  onReload(): void;
}) {
  const { save } = state;
  return <footer className="bots-savebar">
    <div className="bots-save-status">
      {save.kind === 'conflict' ? <p role="alert" className="bots-problem">{tr('bots_conflict')} <button type="button" onClick={onReload}>{tr('bots_reload')}</button></p> : null}
      {save.kind === 'failed' ? <p role="alert" className="bots-problem">{tr('bots_save_failed', save.message)}</p> : null}
      {save.kind === 'saving' ? <p role="status" className="bots-muted">{tr('bots_saving')}</p> : null}
      {save.kind === 'saved' ? <p role="status" className="bots-muted">{tr('bots_saved')}</p> : null}
      {dirty && (save.kind === 'idle' || save.kind === 'failed') ? <p role="status" className="bots-muted">{tr('bots_unsaved')}</p> : null}
    </div>
    <button type="button" data-variant="primary" disabled={!dirty || problems || save.kind === 'saving' || save.kind === 'conflict'} onClick={onSave}>
      {save.kind === 'saving' ? tr('bots_saving') : tr('common_save')}
    </button>
  </footer>;
}

/** What deleting the Bot does to the tasks it is in: the ones still going on are canceled. Nothing is said while that is not known, or when there are none. */
export function BotDeleteTasksNote({ state }: { state: Pick<BotTasksState, 'view' | 'error'> }) {
  const { view } = state;
  if (view !== undefined) {
    return view.active.received > 0 || view.active.sent > 0 ? <p className="bots-hint">{tr('bots_delete_tasks', view.active.received, view.active.sent)}</p> : null;
  }
  return state.error !== undefined ? <p className="bots-hint">{tr('bots_delete_tasks_unknown')}</p> : null;
}

/** What of the Scheduled page's data a Bot's routines panel reads. */
export type BotRoutinesBridge = Pick<UseBridge, 'scheduledScopes' | 'manageScheduled'>;

export function BotDetail({ botId, listed, api, bots, activity, models, pickFolder, visible, sessionLoading, routines: routinesBridge, onBack, onOpenChat, onOpenTaskChat, onNewRoutine, onOpenRoutine, onDelete }: {
  botId: string;
  /** The Bot as the list has it: its name is there to show before the Bot itself has been read. */
  listed: BotUi | undefined;
  api: BotsApi;
  bots: BotsController;
  activity: BotActivity;
  models: ModelChoices;
  pickFolder(): Promise<{ path: string } | null>;
  /** The page is the one on screen. Coming back to it reads the chats again. */
  visible: boolean;
  sessionLoading: boolean;
  routines: BotRoutinesBridge;
  onBack(): void;
  /** Show a chat of this Bot (a new one without an id) in the chat view. Rejects with the reason it could not start. */
  onOpenChat(sessionId?: string): Promise<void>;
  /** Show the chat that does a task, which is a chat of the Bot that was given the task: this one or another. Rejects with the reason it could not start. */
  onOpenTaskChat(botId: string, sessionId: string): Promise<void>;
  /** Go to the Scheduled page to make a task that runs as this Bot, in the scope the scheduler lists for it. */
  onNewRoutine(scopeId: string): void;
  /** Go to the Scheduled page to see one of this Bot's tasks. */
  onOpenRoutine(scopeId: string, jobId: string): void;
  onDelete(): Promise<void>;
}) {
  const [editor] = useState(() => new BotEditorController(api, botId, (bot) => bots.put(bot), () => botPeers(bots.getSnapshot().bots, botId)));
  const [memory] = useState(() => new BotMemoryController(api.memory, botId));
  const [skills] = useState(() => new BotSkillsController(api.skills, botId));
  const [routines] = useState(() => new BotRoutinesController(routinesBridge.manageScheduled));
  const [tasks] = useState(() => new BotTasksController(api.tasks, botId));
  const routineScope = routineScopeOf(routinesBridge.scheduledScopes, botId);
  const state = useObservable(editor);
  const taskState = useObservable(tasks);
  const known = useObservable(bots);
  const remoteAvailability = botRemoteAvailability(useRemoteControl().status);
  const [tab, setTab] = useState<BotTab>('chats');
  const [chats, setChats] = useState<BotChats>({ status: 'loading' });
  const [opening, setOpening] = useState<string | undefined>();
  const [openError, setOpenError] = useState('');
  const [discarding, setDiscarding] = useState(false);
  const [deleting, setDeleting] = useState<{ busy: boolean; error: string } | undefined>();

  useEffect(() => { void editor.load(); }, [editor]);

  const loadChats = useCallback(async () => {
    try { setChats({ status: 'ready', sessions: (await api.listSessions(botId)).sessions }); }
    catch (cause) { setChats({ status: 'failed', message: ipcErrorMessage(cause) }); }
  }, [api, botId]);
  useEffect(() => {
    if (visible && tab === 'chats') void loadChats();
  }, [visible, tab, loadChats]);
  useEffect(() => { if (tab === 'memory') void memory.load(); }, [tab, memory]);
  useEffect(() => { if (tab === 'skills') void skills.load(); }, [tab, skills]);
  // The tasks are read when their tab is shown, or the question whether to delete the Bot is asked, and then follow the host's word that they changed.
  const watchingTasks = visible && (tab === 'tasks' || deleting !== undefined);
  useEffect(() => {
    if (!watchingTasks) return undefined;
    void tasks.load();
    return tasks.watch();
  }, [watchingTasks, tasks]);
  // The routines are read again whenever the page comes back, as the chats are: they run and finish while it is away.
  useEffect(() => {
    if (visible && tab === 'routines' && routineScope !== undefined) void routines.load(routineScope);
  }, [visible, tab, routines, routineScope]);

  const name = state.bot?.name ?? listed?.name ?? '';
  const description = state.bot?.description ?? listed?.description ?? '';
  const peers = botPeers(known.bots, botId);
  const dirty = botEditorDirty(state, peers);
  const problems = botDraftProblems(state.draft, peers);
  // The list is read again whenever the page comes back, so it knows more recent activity than the copy opened here.
  const lastActive = listed?.lastActivityAt ?? state.bot?.lastActivityAt;

  const openWith = async (key: string, start: () => Promise<void>) => {
    if (opening !== undefined || sessionLoading) return;
    setOpening(key);
    setOpenError('');
    const outcome = await startBotChat(start);
    if (!outcome.ok) setOpenError(outcome.message);
    setOpening(undefined);
  };
  const open = (sessionId?: string) => openWith(sessionId ?? 'new', () => onOpenChat(sessionId));
  const remove = async () => {
    setDeleting({ busy: true, error: '' });
    const outcome = await removeBot(onDelete);
    if (!outcome.ok) setDeleting({ busy: false, error: outcome.message });
  };

  return <div className="bots-detail">
    <button type="button" className="bots-back" data-variant="quiet" onClick={() => { if (dirty) setDiscarding(true); else onBack(); }}>← {tr('bots_back')}</button>
    <header className="bots-detail-header">
      <div className="bots-detail-title">
        <h1>{name}</h1>
        <div className="bots-detail-meta">
          <BotStateChip activity={activity} />
          <span className="bots-muted">{lastActive !== undefined ? tr('bots_last_active', relativeActivity(lastActive)) : tr('bots_no_chats_yet')}</span>
        </div>
        {description ? <p className="bots-muted">{description}</p> : null}
      </div>
      <div className="bots-detail-actions">
        <button type="button" data-variant="primary" disabled={opening !== undefined || sessionLoading || state.phase === 'failed'} aria-busy={opening === 'new' || undefined} onClick={() => void open()}>
          {opening === 'new' ? tr('bots_chat_opening') : tr('bots_new_chat')}
        </button>
        <button type="button" data-variant="danger" onClick={() => setDeleting({ busy: false, error: '' })}>{tr('bots_delete')}</button>
      </div>
    </header>
    {openError ? <p role="alert" className="bots-problem">{openError}</p> : null}
    {state.phase === 'failed' ? <p role="alert" className="bots-problem">{tr('bots_detail_failed', state.loadError ?? '')} <button type="button" onClick={() => void editor.load()}>{tr('common_retry')}</button></p> : null}
    {state.phase === 'loading' ? <p role="status" className="bots-muted">{tr('bots_detail_loading')}</p> : null}
    <nav className="bots-tabs" aria-label={tr('bots_sections_label')}>
      {TABS.map((id) => <button key={id} type="button" aria-current={tab === id ? 'page' : undefined} onClick={() => setTab(id)}>{tabLabel(id)}</button>)}
    </nav>
    {tab === 'chats' ? <BotChatsPanel chats={chats} opening={opening} disabled={sessionLoading} onOpen={(sessionId) => void open(sessionId)} onRetry={() => { setChats({ status: 'loading' }); void loadChats(); }} /> : null}
    {tab === 'routines' ? <BotRoutinesPanel controller={routines} scopeId={routineScope} onNew={onNewRoutine} onOpen={onOpenRoutine} /> : null}
    {tab === 'tasks' ? <BotTasksPanel controller={tasks} opening={opening} disabled={sessionLoading}
      onOpen={(task) => void openWith(task.taskId, () => onOpenTaskChat(task.botId, task.sessionId))} /> : null}
    {tab === 'memory' ? <BotMemoryPanel controller={memory} /> : null}
    {tab === 'skills' ? <BotSkillsPanel controller={skills} /> : null}
    {DRAFT_TABS.has(tab) && state.phase === 'ready' ? <>
      {tab === 'general' ? <BotGeneralFields draft={state.draft} problems={problems} models={models} disabled={state.save.kind === 'saving'} onChange={(change) => editor.setDraft(change)} /> : null}
      {tab === 'persona' ? <BotPersonaField draft={state.draft} problems={problems} disabled={state.save.kind === 'saving'} onChange={(change) => editor.setDraft(change)} /> : null}
      {tab === 'folders' ? <BotFoldersField draft={state.draft} problems={problems} disabled={state.save.kind === 'saving'} pickFolder={pickFolder} onChange={(change) => editor.setDraft(change)} /> : null}
      {tab === 'permissions' ? <>
        <BotPermissionField draft={state.draft} disabled={state.save.kind === 'saving'} onChange={(change) => editor.setDraft(change)} />
        <BotRemoteField draft={state.draft} availability={remoteAvailability} disabled={state.save.kind === 'saving'} onChange={(change) => editor.setDraft(change)} />
        <BotAcceptField draft={state.draft} problems={problems} peers={peers === undefined ? undefined : known.bots.filter((bot) => bot.botId !== botId)} disabled={state.save.kind === 'saving'} onChange={(change) => editor.setDraft(change)} />
      </> : null}
      <BotSaveBar state={state} dirty={dirty} problems={hasBotDraftProblems(problems)} onSave={() => void editor.save()} onReload={() => void editor.load()} />
    </> : null}
    {discarding ? <BotConfirm title={tr('bots_discard_title')} icon="info" danger={false} confirmLabel={tr('bots_discard_confirm')} onCancel={() => setDiscarding(false)} onConfirm={onBack}>
      <p>{tr('bots_discard_body')}</p>
    </BotConfirm> : null}
    {deleting ? <BotConfirm title={tr('bots_delete_title', name)} confirmLabel={tr('bots_delete')} busyLabel={tr('bots_deleting')} busy={deleting.busy} disabled={activity !== 'idle'}
      error={deleting.error || undefined} onCancel={() => setDeleting(undefined)} onConfirm={() => void remove()}>
      <p>{tr('bots_delete_body')}</p>
      <BotDeleteTasksNote state={taskState} />
      {activity !== 'idle' ? <p className="bots-hint">{tr('bots_delete_busy_hint')}</p> : null}
    </BotConfirm> : null}
  </div>;
}
