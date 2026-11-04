import { useEffect, useMemo, useRef, useState } from 'react';
import { RemoteRpcError, type RemoteBotSummary } from '@lingxi/bridge-client/remote';
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
      {connection.phase !== 'ready' ? <p className="rc-banner" role="status">{t(locale, 'reconnecting')}</p> : null}
      <div className="rc-crumb">
        <button type="button" className="rc-link" onClick={onBack}>{t(locale, 'backToComputer', { name: hostName })}</button>
      </div>
      {state.bots === undefined ? (
        <p className="rc-note" role="status">{t(locale, 'loading')}</p>
      ) : bot === undefined ? (
        <BotGone locale={locale} />
      ) : (
        <BotChats locale={locale} client={client} bot={bot} connection={connection} onOpenSession={onOpenSession} />
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
  onOpenSession(sessionId: string): void;
}

function BotChats({ locale, client, bot, connection, onOpenSession }: BotChatsProps) {
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
  const create = (): void => {
    setCreating(true);
    setFailure(null);
    client.createBotThread(bot.bot_id).then(
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
  if (state.error?.code === 'unknown_session' || state.error?.code === 'not_remote_enabled') return <BotGone locale={locale} />;
  return (
    <section className="rc-panel">
      <div className="rc-bot-head">
        <div>
          <h2>{bot.name}</h2>
          {bot.description ? <p>{bot.description}</p> : null}
          {bot.waiting_threads > 0 ? <span className="rc-pending-tag">{t(locale, 'botWaiting', { n: bot.waiting_threads })}</span> : null}
        </div>
        <span className={`rc-status rc-status-${bot.status}`}>{t(locale, STATUS[bot.status] ?? 'statusIdle')}</span>
      </div>
      <div className="rc-actions">
        <button type="button" className="button primary" disabled={creating || connection.phase !== 'ready'} onClick={create}>
          {t(locale, 'botNewChat')}
        </button>
      </div>
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
    </section>
  );
}
