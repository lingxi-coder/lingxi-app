import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { DatasetPagePublisher } from '../../../../../packages/teams/DatasetPage.js';
import { VisualizationComposer } from '../../../../../packages/teams/VisualizationComposer.js';
import { visualizationLabels } from '../../../../../packages/teams/visualization.js';
import { useTeamsText } from '../../../../../packages/teams/i18n.js';
import type { TeamSummary } from '../../../../../packages/teams/index.js';
import type { TeamConversation, TeamMessage } from '../../../../../packages/teams/messages.js';
import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge.js';
import type { VisualizationReference } from '../../shared/visualization.js';
import { desktopTeamsApi } from '../bridge/desktopTeamsApi.js';
import { currentLocale, tr } from '../i18n/locale.js';
import { Icon } from './Icon.js';

export function PublishVisualization({ sessionId, reference, kind = 'visualization' }: {
  sessionId: string; reference: VisualizationReference; kind?: 'visualization' | 'page';
}) {
  const locale = currentLocale();
  const t = useTeamsText(locale);
  const title = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const [uid, setUid] = useState('');
  const [source, setSource] = useState('');
  const [teams, setTeams] = useState<TeamSummary[]>([]);
  const [team, setTeam] = useState<TeamSummary>();
  const [chats, setChats] = useState<TeamConversation[]>([]);
  const [chat, setChat] = useState('');
  const [messages, setMessages] = useState<TeamMessage[]>([]);
  const [messageId, setMessageId] = useState('');
  const [directoryReady, setDirectoryReady] = useState(false);
  const [historyReady, setHistoryReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const bridge = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL];
  const api = useMemo(() => uid && bridge?.teams ? desktopTeamsApi(bridge.teams, uid) : undefined, [bridge, uid]);
  const selected = messages.find(message => message.id === messageId);
  const channel = kind === 'visualization' && chats.find(entry => entry.id === chat && entry.kind === 'channel');
  const visualization = useMemo(() => source ? JSON.parse(source) as { title: string; html: string } : undefined, [source]);
  const labels = visualizationLabels(locale);

  useEffect(() => {
    if (!api || !team) return;
    return api.watchMessages({ teamId: team.id, generation: team.generation }, '', snapshot => {
      if (snapshot.kind === 'error') { setFailed(true); return; }
      if (snapshot.kind === 'directory') {
        setChats(snapshot.conversations.filter(item => item.kind !== 'bot_dm' && item.participants.includes('user:' + uid)));
        setDirectoryReady(true);
      }
    });
  }, [api, team, uid]);

  useEffect(() => {
    if (!api || !team || !chat) return;
    let active = true;
    void api.messages<{ messages: TeamMessage[] }>({ action: 'history', teamId: team.id,
      generation: team.generation, conversationId: chat }).then(result => {
      if (!active) return;
      setMessages(result.messages.filter(message => !message.recalled && message.resourceGrant &&
        message.resourceRef && message.resourceRef.kind !== 'pageVersion' &&
        ((message.author.kind === 'user' && message.author.uid === uid) ||
          (message.author.kind === 'task' && message.resourceRef.kind === 'reportRelease'))));
      setHistoryReady(true);
    }, () => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [api, team, chat, uid]);

  async function open(): Promise<void> {
    if (busy || !bridge?.teams) return;
    dialog.current?.showModal();
    setBusy(true);
    setFailed(false);
    try {
      const account = await bridge.account.request({ type: 'status' });
      const owner = account.user?.uid;
      if (!owner) throw new Error('SIGN_IN_REQUIRED');
      const [fragment, directory] = await Promise.all([
        bridge.visualization.source(sessionId, reference, kind),
        bridge.teams.list(owner),
      ]);
      if (!fragment) throw new Error('DATA_UNAVAILABLE');
      setUid(owner);
      setTeams(directory.teams);
      setSource(JSON.stringify({ title: fragment.title, html: fragment.html, assets: {}, actions: {} }, null, 2));
    } catch { setFailed(true); }
    finally { setBusy(false); }
  }

  function selectChat(next: string): void {
    setChat(next);
    setMessages([]);
    setMessageId('');
    setHistoryReady(false);
    setFailed(false);
  }

  function close(): void {
    setUid('');
    setTeams([]);
    setTeam(undefined);
    setChats([]);
    setSource('');
    setDirectoryReady(false);
    selectChat('');
  }

  if (!bridge?.teams) return null;
  const action = kind === 'visualization' ? labels.add : t('tw_dataset_publish_page');
  return <>
    <button type="button" className="visualization-action" disabled={busy} onClick={() => void open()}
      title={action} aria-label={action}>
      <Icon name="page" size={16} />
    </button>
    <dialog ref={dialog} aria-labelledby={title} className="tw-scope tw-dialog tw-data-record" onClose={close}
      onCancel={event => { if (busy) event.preventDefault(); }}>
      <div className="tw-dialog-panel">
        <h3 id={title}>{action}</h3>
        {failed && <p role="alert">{tr('teams_data_unavailable')}</p>}
        {busy && <progress />}
        {source && <label>{t('tw_dataset_team')} <select value={team?.id ?? ''} onChange={event => {
          setTeam(teams.find(item => item.id === event.currentTarget.value));
          setChats([]);
          setDirectoryReady(false);
          selectChat('');
        }}>
          <option value="">{tr('common_none')}</option>
          {teams.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
        </select></label>}
        {team && directoryReady && <label>{t('tw_dataset_shared')} <select value={chat} onChange={event => selectChat(event.currentTarget.value)}>
          <option value="">{tr('common_none')}</option>
          {chats.map(item => <option key={item.id} value={item.id}>{item.title || item.id}</option>)}
        </select></label>}
        {channel && team && api && visualization && <VisualizationComposer key={chat} api={api} locale={locale}
          source={visualization} scope={{ teamId: team.id, generation: team.generation, conversationId: chat }}
          participantName={value => value} onPublished={() => dialog.current?.close()} />}
        {!channel && historyReady && <label>{t('tw_dataset_add')} <select value={messageId} onChange={event => setMessageId(event.currentTarget.value)}>
          <option value="">{tr('common_none')}</option>
          {messages.map(item => <option value={item.id} key={item.id}>{item.text || item.id}</option>)}
        </select></label>}
        {!channel && historyReady && !messages.length && <p>{t('tw_dataset_empty')}</p>}
        {!channel && api && team && selected?.resourceGrant && selected.resourceRef && selected.resourceRef.kind !== 'pageVersion' &&
          <DatasetPagePublisher key={selected.id} api={api} dataset={selected.resourceRef.dataset} via={selected.resourceGrant}
            locale={locale} generation={team.generation} destination={{ team: team.id, conversation: chat }} initialPackage={source} />}
        <form method="dialog"><button type="submit" className="tw-btn" disabled={busy}>{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}
