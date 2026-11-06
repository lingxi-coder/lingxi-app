import { useState, type ReactElement } from 'react';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { messageErrorText } from './errors.js';
import { Avatar, Dialog, type RenderAvatar } from './ui';
import { defaultConversationTitle, findBotDm, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';

interface DialogProps { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; zh: boolean; renderAvatar?: RenderAvatar; onClose(): void }
const MAX_BOTS = 8;

/**
 * The "New channel" and "New message" dialogs. A
 * channel has a name and members; "direct" is either a DM (exactly one person; an existing DM
 * is opened instead of duplicated) or a group (people and Bots, optionally named). Creation
 * goes through `TeamsWorkspace.createConversation`: the viewer is added first and every chosen
 * Bot must still be usable.
 */
export function NewConversationDialog({ workspace, snapshot, zh, mode, renderAvatar, onClose }: DialogProps & { mode: 'channel' | 'direct' }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [direct, setDirect] = useState<'dm' | 'group'>('dm');
  const kind = mode === 'channel' ? 'channel' : direct;
  const [title, setTitle] = useState(''), [parties, setParties] = useState<string[]>([]), [botParties, setBotParties] = useState<string[]>([]), [tried, setTried] = useState(false);
  const people = snapshot.directory.filter(member => member.uid !== snapshot.uid);
  const bots = snapshot.bots.filter(sharedBotUsable);
  const owner = (id: string) => snapshot.directory.find(member => member.uid === id)?.displayName ?? id;
  const named = kind === 'channel' || (kind === 'group' && parties.length === 0);
  const valid = kind === 'dm' ? parties.length === 1 : parties.length + botParties.length > 0 && (!named || title.trim() !== '');
  const submit = async () => {
    if (!valid || snapshot.offline) return;
    setTried(true);
    const ok = kind === 'dm' ? await workspace.openDm(parties[0]!)
      : await workspace.createConversation(kind, kind === 'group' && !title.trim() ? defaultConversationTitle(parties, snapshot.directory, snapshot.uid) : title, parties, botParties);
    if (ok) onClose();
  };
  const choose = (id: string, checked: boolean) => setParties(kind === 'dm' ? [id] : checked ? [...parties, id] : parties.filter(value => value !== id));
  return <Dialog title={mode === 'channel' ? t('New channel', '新建频道') : t('New message', '新建私信')} onClose={onClose}>
    <form className="tw-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      {mode === 'direct' ? <div className="tw-segment" role="radiogroup" aria-label={t('Type', '类型')}>
        {(['dm', 'group'] as const).map(value => <label key={value} className="tw-segment-option" data-checked={direct === value}>
          <input type="radio" name="tw-kind" value={value} checked={direct === value} onChange={() => { setDirect(value); setParties([]); setBotParties([]); }} />
          {value === 'dm' ? t('Direct message', '私聊') : t('Group', '群聊')}
        </label>)}
      </div> : null}
      {kind !== 'dm' ? <label>{kind === 'channel' ? t('Channel name', '频道名称') : t('Group name (optional)', '群聊名称（可选）')}
        <input data-autofocus value={title} onChange={event => setTitle(event.target.value)} maxLength={120} required={named} autoComplete="off" />
      </label> : null}
      <fieldset className="tw-fieldset">
        <legend>{kind === 'dm' ? t('Message', '发给') : t('People', '成员')}</legend>
        {people.map(member => <label key={member.uid} className="tw-check">
          <input type={kind === 'dm' ? 'radio' : 'checkbox'} name="tw-people" checked={parties.includes(member.uid)} onChange={event => choose(member.uid, event.target.checked)} />
          <Avatar identity={{ kind: 'user', id: member.uid, name: member.displayName }} size={20} render={renderAvatar} />
          {member.displayName}
        </label>)}
        {!people.length ? <p className="tw-hint">{t('No other members yet. Invite people from Team settings.', '暂无其他成员，请在团队设置中邀请成员。')}</p> : null}
      </fieldset>
      {kind !== 'dm' && bots.length ? <fieldset className="tw-fieldset">
        <legend>{t('Participating Bots', '参与的 Bot')}</legend>
        {bots.map(bot => <label key={bot.sharedBotId} className="tw-check">
          <input type="checkbox" checked={botParties.includes(bot.sharedBotId)} disabled={!botParties.includes(bot.sharedBotId) && botParties.length >= MAX_BOTS}
            onChange={event => setBotParties(event.target.checked ? [...botParties, bot.sharedBotId] : botParties.filter(key => key !== bot.sharedBotId))} />
          <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={20} render={renderAvatar} />
          {bot.name} · {owner(bot.maintainer.uid)}
        </label>)}
      </fieldset> : null}
      {botParties.length ? <p className="tw-hint">{t('A Bot’s owner can read every conversation that includes their Bot.', 'Bot 的所有者可阅读包含其 Bot 的所有会话。')}</p> : null}
      {snapshot.offline ? <p role="status" className="tw-hint">{t('Reconnect to create conversations.', '联网后才能创建会话。')}</p> : null}
      {tried && snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, zh)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('Cancel', '取消')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.messageBusy || snapshot.offline || !valid}>
          {kind === 'dm' ? t('Open message', '打开私信') : kind === 'group' ? t('Create group', '创建群聊') : t('Create channel', '创建频道')}
        </button>
      </div>
    </form>
  </Dialog>;
}

/** Pick a Bot you may use and chat with it: opens the Bot DM you already have, or starts one. */
export function NewBotDmDialog({ workspace, snapshot, zh, renderAvatar, onClose }: DialogProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [tried, setTried] = useState(false);
  const usable = snapshot.bots.filter(sharedBotUsable);
  const owner = (id: string) => snapshot.directory.find(member => member.uid === id)?.displayName ?? id;
  const open = async (bot: SharedBotCard, fresh: boolean) => { setTried(true); if (await workspace.openBotDm(bot, fresh)) onClose(); };
  return <Dialog title={t('Message a Bot', '与 Bot 对话')} onClose={onClose}>
    <div className="tw-form">
      <p className="tw-hint">{t('Bots run on their owner’s Desktop. The owner can read every conversation containing their Bot.', 'Bot 在所有者的 Desktop 上运行。所有者可阅读包含其 Bot 的所有会话。')}</p>
      {snapshot.botsUnverified ? <p role="status" className="tw-hint">{t('Bot availability could not be verified.', '未能确认 Bot 可用状态。')}</p> : null}
      <ul className="tw-picker" aria-label={t('Bots you can message', '可对话的 Bot')}>
        {usable.map(bot => {
          const existing = findBotDm(snapshot.conversations, snapshot.uid, bot.sharedBotId);
          return <li key={bot.sharedBotId}>
            <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={28} render={renderAvatar} />
            <div className="tw-picker-text">
              <b>{bot.name}</b>
              <small>{bot.ready ? t('Online', '在线') : t('Host unavailable', '宿主不可用')} · {t('Owner', '所有者')}: {owner(bot.maintainer.uid)}</small>
            </div>
            <div className="tw-picker-actions">
              <button type="button" className="tw-primary" disabled={snapshot.messageBusy} onClick={() => void open(bot, false)}>{existing ? t('Open chat', '打开会话') : t('Start chat', '开始对话')}</button>
              {existing ? <button type="button" disabled={snapshot.messageBusy} onClick={() => void open(bot, true)}>{t('New chat', '新对话')}</button> : null}
            </div>
          </li>;
        })}
      </ul>
      {!usable.length && !snapshot.botsUnverified ? <p className="tw-hint">{snapshot.botsLoaded
        ? t('No Bots are available to you yet. Ask a Bot’s owner to share one, or share your own from Team settings.', '暂无可用的 Bot。请 Bot 所有者共享，或在团队设置中共享你自己的 Bot。')
        : t('Loading Bots…', '正在加载 Bot…')}</p> : null}
      {tried && snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, zh)}</p> : null}
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={onClose}>{t('Close', '关闭')}</button></div>
    </div>
  </Dialog>;
}
