import { useEffect, useMemo, useRef, useState } from 'react';
import { ChatTopbarControl } from '../../../../packages/teams/ChatTopbar';
import { RemoteRpcError, type RemoteBotSummary, type RemoteBotWorkspaceMode } from '@lingxi/bridge-client/remote';
import type { Locale } from '../utils/locale';
import { requestErrorStringKey } from './failureText';
import { knownBots, type RemoteClient } from './remoteClient';
import { SessionRow } from './SessionList';
import { t, type StringKey } from './strings';
import { useStore } from './useStore';
import type { ConnectionState } from './viewerConnection';

/** A Bot's status is one of a session's, less the one that only a session can be in. */
const STATUS: Record<RemoteBotSummary['status'], StringKey> = {
  idle: 'statusIdle',
  running: 'statusRunning',
  requires_action: 'statusRequiresAction',
};

interface BotListProps {
  locale: Locale;
  client: RemoteClient;
  onOpen(botId: string): void;
}

/** The Bots of the computer, above its sessions; nothing at all where the computer has none to show or does not know Bots. */
export function BotList({ locale, client, onOpen }: BotListProps) {
  const bots = knownBots(useStore(client)!);
  if (bots.length === 0) return null;
  return (
    <section className="rc-panel">
      <h2>{t(locale, 'bots')}</h2>
      <ul className="rc-list">
        {bots.map((bot) => (
          <li key={bot.bot_id}>
            <button type="button" className="rc-row rc-row-button" onClick={() => onOpen(bot.bot_id)}>
              <span className="rc-row-main">
                <strong>{bot.name}</strong>
                {bot.description ? <span className="rc-row-sub rc-bot-description">{bot.description}</span> : null}
                {bot.waiting_threads > 0 ? <span className="rc-pending-tag">{t(locale, 'botWaiting', { n: bot.waiting_threads })}</span> : null}
                {bot.pending_workspace_requests ? <span className="rc-pending-tag">{t(locale, 'botFolderAsks', { n: bot.pending_workspace_requests })}</span> : null}
              </span>
              <span className={`rc-status rc-status-${bot.status}`}>{t(locale, STATUS[bot.status] ?? 'statusIdle')}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

interface BotPageProps {
  locale: Locale;
  client: RemoteClient;
  hostName: string;
  botId: string;
  connection: ConnectionState;
  /** Back to the computer's page. */
  onBack(): void;
  onOpenSession(sessionId: string): void;
}

/** One Bot: what it is, its chats newest first, and a way to start a new one. */
export function BotPage({ locale, client, hostName, botId, connection, onBack, onOpenSession }: BotPageProps) {
  const state = useStore(client)!;
  const bot = knownBots(state).find((entry) => entry.bot_id === botId);
  return (
    <>
      {!bot ? <ChatTopbarControl><header className="rc-session-head">
        <button type="button" className="rc-back" onClick={onBack} aria-label={t(locale, 'backToComputer', { name: hostName })}>‹</button>
        <div className="rc-session-title"><h2>{t(locale, 'bots')}</h2></div>
        {connection.phase !== 'ready' ? <span className="rc-topbar-status" role="status">{t(locale, 'reconnecting')}</span> : null}
      </header></ChatTopbarControl> : null}
      {state.bots === undefined ? (
        <p className="rc-note" role="status">{t(locale, 'loading')}</p>
      ) : bot === undefined ? (
        <BotGone locale={locale} />
      ) : (
        <BotChats locale={locale} client={client} bot={bot} connection={connection} hostName={hostName} onBack={onBack} onOpenSession={onOpenSession} />
      )}
    </>
  );
}

/** The Bot is not in the computer's list (its switch went off, it was deleted, or the computer does not know Bots). */
function BotGone({ locale }: { locale: Locale }) {
  return (
    <section className="rc-panel">
      <p role="status">{t(locale, 'botGone')}</p>
    </section>
  );
}

/** Why starting a chat failed: the Bot going away is told as such, anything else as any refused request is. */
function newChatErrorKey(code: string): StringKey {
  return code === 'unknown_session' || code === 'not_remote_enabled' ? 'botGone' : requestErrorStringKey(code);
}

interface BotChatsProps {
  locale: Locale;
  client: RemoteClient;
  bot: RemoteBotSummary;
  connection: ConnectionState;
  hostName: string;
  onBack(): void;
  onOpenSession(sessionId: string): void;
}

function BotChats({ locale, client, bot, connection, hostName, onBack, onOpenSession }: BotChatsProps) {
  const threads = useMemo(() => client.botThreads(bot.bot_id), [client, bot.bot_id]);
  const state = useStore(threads)!;
  const [creating, setCreating] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    threads.activate();
    return () => threads.deactivate();
  }, [threads]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The chat is made on the computer whether or not this page waits for it: a page that was left is not taken to it afterwards.
  // It works in the folder `workspaceId` names, or in the Bot's own workspace if there is none.
  const create = (workspaceId?: string): void => {
    setCreating(true);
    setFailure(null);
    client.createBotThread(bot.bot_id, workspaceId).then(
      (sessionId) => {
        if (mounted.current) onOpenSession(sessionId);
      },
      (error: unknown) => {
        if (!mounted.current) return;
        setCreating(false);
        setFailure(error instanceof RemoteRpcError ? error.code : 'internal');
      },
    );
  };

  // The list failed because the Bot is no longer there; `bots.changed` says so too, a moment later.
  const gone = state.error?.code === 'unknown_session' || state.error?.code === 'not_remote_enabled';
  return (
    <>
      <ChatTopbarControl><header className="rc-session-head rc-bot-head">
        <button type="button" className="rc-back" onClick={onBack} aria-label={t(locale, 'backToComputer', { name: hostName })}>‹</button>
        <div className="rc-session-title">
          <h2>{bot.name}</h2>
          {!gone && bot.description ? <p title={bot.description}>{bot.description}</p> : null}
          {bot.waiting_threads > 0 ? <span className="rc-pending-tag">{t(locale, 'botWaiting', { n: bot.waiting_threads })}</span> : null}
          {bot.pending_workspace_requests ? <span className="rc-pending-tag">{t(locale, 'botFolderAsks', { n: bot.pending_workspace_requests })}</span> : null}
        </div>
        <span className={`rc-status rc-status-${bot.status}`}>{t(locale, STATUS[bot.status] ?? 'statusIdle')}</span>
        {connection.phase !== 'ready' ? <span className="rc-topbar-status" role="status">{t(locale, 'reconnecting')}</span> : null}
        {!gone ? <button type="button" className="button primary rc-new-chat" disabled={creating || connection.phase !== 'ready'} onClick={() => create()}>
          {t(locale, 'botNewChat')}
        </button> : null}
      </header></ChatTopbarControl>
      {gone ? <BotGone locale={locale} /> : <>
      <BotFolders locale={locale} client={client} bot={bot} connection={connection} creating={creating} onNewChat={create} />
      <section className="rc-panel">
        {failure !== null ? <p className="rc-note rc-danger" role="alert">{t(locale, newChatErrorKey(failure))}</p> : null}
        <div className="rc-bot-chats">
          {state.threads === undefined && state.error === undefined ? <p className="rc-note" role="status">{t(locale, 'loading')}</p> : null}
          {state.error !== undefined ? (
            <p className="rc-note rc-danger" role="alert">
              {t(locale, 'errGeneric')}{' '}
              <button type="button" className="rc-link" onClick={() => threads.refresh()}>{t(locale, 'retry')}</button>
            </p>
          ) : null}
          {state.threads?.length === 0 ? (
            <div className="rc-empty">
              <h3>{t(locale, 'botNoChats')}</h3>
            </div>
          ) : null}
          <ul className="rc-list">
            {(state.threads ?? []).map((thread) => (
              <SessionRow key={thread.session_id} locale={locale} session={thread} onOpen={onOpenSession} />
            ))}
          </ul>
        </div>
      </section></>}
    </>
  );
}

const MODE_STRINGS: Record<RemoteBotWorkspaceMode, StringKey> = { read: 'botFolderRead', write: 'botFolderWrite' };

const asMode = (value: string): RemoteBotWorkspaceMode => (value === 'write' ? 'write' : 'read');

interface BotFoldersProps {
  locale: Locale;
  client: RemoteClient;
  bot: RemoteBotSummary;
  connection: ConnectionState;
  /** A chat is being started. */
  creating: boolean;
  /** Start a chat in the folder with this `workspace_id`. */
  onNewChat(workspaceId: string): void;
}

/**
 * The folders the Bot works in besides its own workspace: how far it may go in each, a chat in one, taking one away,
 * the folders it asked for, and adding one of the computer's projects. A folder is only ever one of the computer's
 * projects, never a path typed here. Nothing is shown for a Bot with no folder, no request and no project to give.
 */
function BotFolders({ locale, client, bot, connection, creating, onNewChat }: BotFoldersProps) {
  const store = useMemo(() => client.botWorkspaces(bot.bot_id), [client, bot.bot_id]);
  const state = useStore(store)!;
  const [project, setProject] = useState('');
  const [mode, setMode] = useState<RemoteBotWorkspaceMode>('read');
  /** The folder whose removal is waiting to be confirmed. */
  const [removing, setRemoving] = useState<string | null>(null);

  useEffect(() => {
    store.activate();
    return () => store.deactivate();
  }, [store]);

  const folders = state.folders;
  const idle = connection.phase === 'ready' && !state.busy;
  const failed = state.error !== undefined ? <p className="rc-note rc-danger rc-folder-failure" role="alert">{t(locale, 'botFolderFailed', { reason: state.error.message })}</p> : null;
  if (folders === undefined || (folders.workspaces.length === 0 && folders.requests.length === 0 && folders.projects.length === 0)) return failed === null ? null : <section className="rc-panel rc-bot-folders">{failed}</section>;

  const add = (): void => {
    if (project === '') return;
    void store.add(project, mode).then((taken) => {
      if (taken) setProject('');
    });
  };
  return (
    <section className="rc-panel rc-bot-folders" aria-label={t(locale, 'botFolders')}>
      {folders.requests.length > 0 ? (
        <div className="rc-folder-requests">
          <h3>{t(locale, 'botFolderRequests')}</h3>
          <ul className="rc-list">
            {folders.requests.map((request) => (
              <li key={request.request_id} className="rc-row rc-folder">
                <span className="rc-row-main">
                  <strong className="rc-folder-path" title={request.path}>{request.path}</strong>
                  <span className="rc-row-sub">{t(locale, MODE_STRINGS[request.mode] ?? 'botFolderRead')}</span>
                  {request.reason ? <span className="rc-row-sub">{request.reason}</span> : null}
                </span>
                <span className="rc-folder-actions">
                  <button type="button" className="button primary small" disabled={!idle} onClick={() => void store.decide(request.request_id, 'approve')}>{t(locale, 'botFolderAllow')}</button>
                  <button type="button" className="button secondary small" disabled={!idle} onClick={() => void store.decide(request.request_id, 'deny')}>{t(locale, 'deny')}</button>
                </span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <h3>{t(locale, 'botFolders')}</h3>
      <p className="rc-note">{t(locale, 'botFoldersHint')}</p>
      {folders.workspaces.length === 0 ? <p className="rc-note">{t(locale, 'botFoldersEmpty')}</p> : (
        <ul className="rc-list rc-folder-list">
          {folders.workspaces.map((folder) => (
            <li key={folder.workspace_id} className="rc-row rc-folder">
              <span className="rc-row-main">
                <strong>{folder.label}</strong>
                <span className="rc-row-sub rc-folder-path" title={folder.path}>{folder.path}</span>
                {removing === folder.workspace_id ? <span className="rc-row-sub rc-danger rc-folder-confirm" role="status">{t(locale, 'botFolderRemoveAsk', { name: folder.label })}</span> : null}
              </span>
              <span className="rc-folder-actions">
                <select aria-label={t(locale, 'botFolderAccess', { name: folder.label })} value={folder.mode} disabled={!idle} onChange={(event) => void store.setMode(folder.workspace_id, asMode(event.target.value))}>
                  <option value="read">{t(locale, 'botFolderRead')}</option>
                  <option value="write">{t(locale, 'botFolderWrite')}</option>
                </select>
                <button type="button" className="button secondary small" disabled={!idle || creating} aria-label={t(locale, 'botFolderNewChatIn', { name: folder.label })} onClick={() => onNewChat(folder.workspace_id)}>{t(locale, 'botNewChat')}</button>
                {removing === folder.workspace_id ? (
                  <>
                    <button type="button" className="button secondary small rc-danger" disabled={!idle} onClick={() => { setRemoving(null); void store.removeFolder(folder.workspace_id); }}>{t(locale, 'botFolderRemoveConfirm')}</button>
                    <button type="button" className="rc-link" onClick={() => setRemoving(null)}>{t(locale, 'cancel')}</button>
                  </>
                ) : (
                  <button type="button" className="rc-link rc-danger" disabled={!idle} aria-label={t(locale, 'botFolderRemoveNamed', { name: folder.label })} onClick={() => setRemoving(folder.workspace_id)}>{t(locale, 'botFolderRemove')}</button>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
      {folders.projects.length > 0 ? (
        <div className="rc-folder-add">
          <h3>{t(locale, 'botFolderAddTitle')}</h3>
          <div className="rc-folder-add-row">
            <select aria-label={t(locale, 'botFolderPick')} value={project} disabled={!idle} onChange={(event) => setProject(event.target.value)}>
              <option value="">{t(locale, 'botFolderPick')}</option>
              {folders.projects.map((entry) => <option key={entry.path} value={entry.path} title={entry.path}>{entry.label}</option>)}
            </select>
            <select aria-label={t(locale, 'botFolderAccess', { name: t(locale, 'botFolderAddTitle') })} value={mode} disabled={!idle} onChange={(event) => setMode(asMode(event.target.value))}>
              <option value="read">{t(locale, 'botFolderRead')}</option>
              <option value="write">{t(locale, 'botFolderWrite')}</option>
            </select>
            <button type="button" className="button primary small" disabled={!idle || project === ''} onClick={add}>{t(locale, 'botFolderAdd')}</button>
          </div>
        </div>
      ) : null}
      {failed}
    </section>
  );
}
