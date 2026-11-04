import type { SessionRowDto } from '@lingxi/bridge-client';
import { tr } from '../i18n/locale';
import { botMemoryProblem, type BotMemoryController } from '../bridge/botMemory';
import { routineRow, type BotRoutinesController } from '../bridge/botRoutines';
import type { BotSkillsController } from '../bridge/botSkills';
import { taskRow, type BotTasksController } from '../bridge/botTasks';
import { relativeActivity } from '../bridge/botSession';
import { formatSessionMetadata } from '../bridge/sessionPresentation';
import { BotConfirm, BotDialog, problemText, useObservable } from './BotFields';
import { DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';

/** The chats of a Bot, as `listSessions` last answered. */
export type BotChats =
  | { status: 'loading' }
  | { status: 'ready'; sessions: readonly SessionRowDto[] }
  | { status: 'failed'; message: string };

export function BotChatsPanel({ chats, opening, disabled, onOpen, onRetry }: {
  chats: BotChats;
  /** The chat being opened: its id, or `'new'`. */
  opening: string | undefined;
  disabled: boolean;
  onOpen(sessionId: string): void;
  onRetry(): void;
}) {
  if (chats.status === 'loading') return <p role="status" className="bots-muted">{tr('bots_chats_loading')}</p>;
  if (chats.status === 'failed') {
    return <div role="alert" className="bots-problem">{tr('bots_chats_failed', chats.message)} <button type="button" onClick={onRetry}>{tr('common_retry')}</button></div>;
  }
  if (chats.sessions.length === 0) return <p className="bots-empty-line">{tr('bots_chats_empty')}</p>;
  return <ul className="bots-rows" aria-label={tr('bots_tab_chats')}>
    {chats.sessions.map((session) => <li key={session.uuid}>
      <button type="button" className="bots-chat-row" disabled={disabled || opening !== undefined} aria-busy={opening === session.uuid || undefined} onClick={() => onOpen(session.uuid)}>
        <span className="bots-row-title">{session.title.trim() || tr('remote_untitled')}</span>
        <span className="bots-muted">{opening === session.uuid ? tr('bots_chat_opening') : formatSessionMetadata(session.modified_rfc3339, session.message_count)}</span>
      </button>
    </li>)}
  </ul>;
}

/** The tasks that run as the Bot. Making and changing one is done on the Scheduled page, which this sends the owner to. */
export function BotRoutinesPanel({ controller, scopeId, onNew, onOpen }: {
  controller: BotRoutinesController;
  /** The scope the scheduler lists for this Bot; absent while it does not list one. */
  scopeId: string | undefined;
  onNew(scopeId: string): void;
  onOpen(scopeId: string, jobId: string): void;
}) {
  const state = useObservable(controller);
  return <div className="bots-panel">
    <p className="bots-hint">{tr('bots_routines_hint')}</p>
    <div><button type="button" disabled={scopeId === undefined} onClick={() => { if (scopeId !== undefined) onNew(scopeId); }}>{tr('bots_routines_new')}</button></div>
    {scopeId === undefined ? <p role="status" className="bots-muted">{tr('bots_routines_unavailable')}</p> : null}
    {scopeId !== undefined && state.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_routines_failed', state.error)} <button type="button" onClick={() => void controller.load(scopeId)}>{tr('common_retry')}</button></p> : null}
    {scopeId !== undefined && state.jobs === undefined && state.error === undefined ? <p role="status" className="bots-muted">{tr('bots_routines_loading')}</p> : null}
    {scopeId !== undefined && state.jobs?.length === 0 ? <p className="bots-empty-line">{tr('bots_routines_empty')}</p> : null}
    {scopeId !== undefined && state.jobs && state.jobs.length > 0 ? <ul className="bots-rows" aria-label={tr('bots_tab_routines')}>
      {state.jobs.map(routineRow).map((row) => <li key={row.id}>
        <button type="button" className="bots-chat-row" disabled={!row.openable} onClick={() => onOpen(scopeId, row.id)}>
          <span className="bots-row-title">{row.title}<span className="bots-tag" data-status={row.status}>{row.statusLabel}</span></span>
          <span className="bots-muted">{row.schedule}</span>
          <span className="bots-muted">{row.next}</span>
        </button>
      </li>)}
    </ul> : null}
  </div>;
}

export function BotMemoryPanel({ controller }: { controller: BotMemoryController }) {
  const state = useObservable(controller);
  const { open, deleting } = state;
  const problem = open?.editing ? botMemoryProblem(open.draft) : undefined;
  return <div className="bots-panel">
    <p className="bots-hint">{tr('bots_memory_hint')}</p>
    {state.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_memory_failed', state.error)} <button type="button" onClick={() => void controller.load()}>{tr('common_retry')}</button></p> : null}
    {state.entries === undefined && state.error === undefined ? <p role="status" className="bots-muted">{tr('bots_memory_loading')}</p> : null}
    {state.entries?.length === 0 ? <p className="bots-empty-line">{tr('bots_memory_empty')}</p> : null}
    {state.entries && state.entries.length > 0 ? <ul className="bots-rows" aria-label={tr('bots_tab_memory')}>
      {state.entries.map((entry) => <li key={entry.name} className="bots-row" data-open={open?.name === entry.name || undefined}>
        <div className="bots-row-main">
          <span className="bots-row-title">{entry.name}{entry.type ? <span className="bots-tag">{entry.type}</span> : null}</span>
          {entry.description ? <span className="bots-row-text">{entry.description}</span> : null}
          <span className="bots-muted">{tr('bots_memory_modified', relativeActivity(entry.modifiedAt))}</span>
        </div>
        <div className="bots-row-actions">
          <button type="button" data-variant="quiet" aria-label={`${tr('bots_memory_view')}: ${entry.name}`} onClick={() => void controller.open(entry.name)}>{tr('bots_memory_view')}</button>
          <button type="button" data-variant="quiet" aria-label={`${tr('common_delete')}: ${entry.name}`} onClick={() => controller.askDelete(entry.name)}>{tr('common_delete')}</button>
        </div>
      </li>)}
    </ul> : null}
    {open ? <section className="bots-viewer" aria-label={open.name}>
      <header>
        <strong className="mono">{open.name}</strong>
        <div className="bots-row-actions">
          {!open.editing ? <button type="button" data-variant="quiet" disabled={open.busy || open.error !== undefined} onClick={() => controller.edit()}>{tr('bots_memory_edit')}</button> : null}
          <button type="button" data-variant="quiet" disabled={open.busy && open.editing} onClick={() => controller.close()}>{tr('common_close')}</button>
        </div>
      </header>
      {open.busy && !open.editing && open.content === '' && open.error === undefined ? <p role="status" className="bots-muted">{tr('bots_memory_loading')}</p> : null}
      {open.error !== undefined ? <p role="alert" className="bots-problem">{open.editing ? tr('bots_save_failed', open.error) : tr('bots_memory_failed', open.error)}</p> : null}
      {open.editing ? <>
        <textarea className="bots-memory-text" aria-label={tr('bots_memory_edit_label')} rows={14} spellCheck={false} value={open.draft} disabled={open.busy}
          aria-invalid={problem ? true : undefined} onChange={(event) => controller.setDraft(event.target.value)} />
        {problem ? <p role="alert" className="bots-problem">{problemText(problem)}</p> : null}
        <div className="bots-row-actions">
          <button type="button" data-variant="primary" disabled={open.busy || problem !== undefined} onClick={() => void controller.save()}>{open.busy ? tr('bots_saving') : tr('common_save')}</button>
          <button type="button" disabled={open.busy} onClick={() => controller.cancelEdit()}>{tr('common_cancel')}</button>
        </div>
      </> : open.error === undefined && !open.busy ? <pre className="bots-memory-content">{open.content}</pre> : null}
    </section> : null}
    {deleting ? <BotConfirm title={tr('bots_memory_delete_title')} confirmLabel={tr('bots_memory_delete_confirm')} busyLabel={tr('bots_deleting')} busy={deleting.busy}
      error={deleting.error !== undefined ? tr('bots_action_failed', deleting.error) : undefined} onCancel={() => controller.cancelDelete()} onConfirm={() => void controller.confirmDelete()}>
      <p>{tr('bots_memory_delete_body', deleting.name)}</p>
    </BotConfirm> : null}
  </div>;
}

export function BotSkillsPanel({ controller }: { controller: BotSkillsController }) {
  const state = useObservable(controller);
  const { importing, deleting } = state;
  return <div className="bots-panel">
    <p className="bots-hint">{tr('bots_skills_hint')}</p>
    <div><button type="button" onClick={() => void controller.openImport()}>{tr('bots_skills_import')}</button></div>
    {state.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_skills_failed', state.error)} <button type="button" onClick={() => void controller.load()}>{tr('common_retry')}</button></p> : null}
    {state.skills === undefined && state.error === undefined ? <p role="status" className="bots-muted">{tr('bots_skills_loading')}</p> : null}
    {state.skills?.length === 0 ? <p className="bots-empty-line">{tr('bots_skills_empty')}</p> : null}
    {state.skills && state.skills.length > 0 ? <ul className="bots-rows" aria-label={tr('bots_tab_skills')}>
      {state.skills.map((skill) => <li key={skill.name} className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{skill.name}</span>
          {skill.description ? <span className="bots-row-text">{skill.description}</span> : null}
        </div>
        <div className="bots-row-actions">
          <button type="button" data-variant="quiet" aria-label={`${tr('common_delete')}: ${skill.name}`} onClick={() => controller.askDelete(skill.name)}>{tr('common_delete')}</button>
        </div>
      </li>)}
    </ul> : null}
    {importing ? <BotDialog title={tr('bots_skills_import_title')} icon="puzzle" busy={importing.busy !== undefined} onClose={() => controller.closeImport()}
      footer={<DesktopDialogActions><DesktopDialogButton variant="cancel" disabled={importing.busy !== undefined} onClick={() => controller.closeImport()}>{tr('common_close')}</DesktopDialogButton></DesktopDialogActions>}>
      <div className="bots-dialog-body">
        <p>{tr('bots_skills_import_body')}</p>
        {importing.candidates === undefined ? <p role="status" className="bots-muted">{tr('bots_skills_loading')}</p> : null}
        {importing.candidates?.length === 0 && importing.error === undefined ? <p className="bots-empty-line">{tr('bots_skills_import_empty')}</p> : null}
        {importing.candidates && importing.candidates.length > 0 ? <ul className="bots-rows" aria-label={tr('bots_skills_import_title')}>
          {importing.candidates.map((skill) => <li key={skill.name} className="bots-row">
            <div className="bots-row-main">
              <span className="bots-row-title">{skill.name}</span>
              {skill.description ? <span className="bots-row-text">{skill.description}</span> : null}
            </div>
            <div className="bots-row-actions">
              <button type="button" disabled={importing.busy !== undefined} aria-label={`${tr('bots_skills_import_action')}: ${skill.name}`} onClick={() => void controller.importSkill(skill.name)}>
                {importing.busy === skill.name ? tr('bots_skills_importing') : tr('bots_skills_import_action')}
              </button>
            </div>
          </li>)}
        </ul> : null}
        {importing.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_action_failed', importing.error)}</p> : null}
      </div>
    </BotDialog> : null}
    {deleting ? <BotConfirm title={tr('bots_skills_delete_title')} confirmLabel={tr('bots_skills_delete_confirm')} busyLabel={tr('bots_deleting')} busy={deleting.busy}
      error={deleting.error !== undefined ? tr('bots_action_failed', deleting.error) : undefined} onCancel={() => controller.cancelDelete()} onConfirm={() => void controller.confirmDelete()}>
      <p>{tr('bots_skills_delete_body', deleting.name)}</p>
    </BotConfirm> : null}
  </div>;
}

/** The tasks other Bots handed to this Bot and the ones it handed to them: who, how it stands, and what came of it. */
export function BotTasksPanel({ controller, opening, disabled, onOpen }: {
  controller: BotTasksController;
  /** The task whose chat is being opened. */
  opening: string | undefined;
  /** A chat is being opened, or the window is busy showing one. */
  disabled: boolean;
  /** Show the chat of the session that does the task, which is a chat of the receiving Bot. */
  onOpen(task: { taskId: string; botId: string; sessionId: string }): void;
}) {
  const state = useObservable(controller);
  const { view, canceling } = state;
  const rows = (view?.tasks ?? []).map((task) => taskRow(task));
  const stopping = canceling ? taskRow(canceling.task) : undefined;
  return <div className="bots-panel">
    <p className="bots-hint">{tr('bots_tasks_hint')}</p>
    {state.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_tasks_failed', state.error)} <button type="button" onClick={() => void controller.load()}>{tr('common_retry')}</button></p> : null}
    {view === undefined && state.error === undefined ? <p role="status" className="bots-muted">{tr('bots_tasks_loading')}</p> : null}
    {view !== undefined && rows.length === 0 ? <p className="bots-empty-line">{tr('bots_tasks_empty')}</p> : null}
    {rows.length > 0 ? <ul className="bots-rows" aria-label={tr('bots_tab_tasks')}>
      {rows.map((row) => <li key={row.id} className="bots-row bots-task" data-state={row.state}>
        <div className="bots-row-main">
          <span className="bots-row-title">{row.title}<span className="bots-tag" data-status={row.state}>{row.stateLabel}</span></span>
          <span className="bots-row-text">{row.summary}</span>
          <span className="bots-muted">{row.when}</span>
          {row.reason !== undefined ? <span className="bots-muted">{row.reason}</span> : null}
          {row.result ? <details className="bots-task-result">
            <summary>{tr('bots_task_result')}</summary>
            <pre className="bots-memory-content">{row.result.text}</pre>
            {row.result.truncated ? <p className="bots-hint">{tr('bots_task_result_cut')}</p> : null}
          </details> : null}
          {row.files.length > 0 ? <div className="bots-task-files">
            <span className="bots-muted">{row.filesLabel}</span>
            <ul>{row.files.map((file) => <li key={file.path}>
              <span>{file.name}</span> <span className="bots-muted">{file.size}</span> <span className="bots-muted mono">{file.path}</span>
            </li>)}</ul>
          </div> : null}
          {row.cost !== undefined ? <span className="bots-muted">{row.cost}</span> : null}
        </div>
        <div className="bots-row-actions">
          {row.openable ? <button type="button" data-variant="quiet" disabled={disabled || opening !== undefined} aria-busy={opening === row.id || undefined}
            aria-label={`${tr('bots_task_open_chat')}: ${row.title}`} onClick={() => onOpen({ taskId: row.id, botId: row.receiver.botId, sessionId: row.receiver.sessionId })}>
            {opening === row.id ? tr('bots_chat_opening') : tr('bots_task_open_chat')}
          </button> : null}
          {row.active ? <button type="button" data-variant="quiet" aria-label={`${tr('bots_task_stop')}: ${row.title}`} onClick={() => controller.askCancel(row.id)}>{tr('bots_task_stop')}</button> : null}
        </div>
      </li>)}
    </ul> : null}
    {view?.truncated ? <p className="bots-hint">{tr('bots_tasks_truncated')}</p> : null}
    {canceling && stopping ? <BotConfirm title={tr('bots_task_stop_title')} icon="info" danger={false} confirmLabel={tr('bots_task_stop_confirm')} busyLabel={tr('bots_task_stopping')} busy={canceling.busy}
      error={canceling.error !== undefined ? tr('bots_action_failed', canceling.error) : undefined} onCancel={() => controller.dismissCancel()} onConfirm={() => void controller.confirmCancel()}>
      <p>{tr('bots_task_stop_body')}</p>
      <p className="bots-muted">{stopping.title}: {stopping.summary}</p>
    </BotConfirm> : null}
  </div>;
}
