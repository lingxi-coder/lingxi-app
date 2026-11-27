import { useState, type ReactElement } from 'react';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { messageErrorText } from './errors.js';
import { Avatar, Dialog, type RenderAvatar } from './ui';
import { defaultConversationTitle, findBotDm, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';
import { useTeamsText } from './i18n.js';

interface DialogProps { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string; renderAvatar?: RenderAvatar; onClose(): void }
const MAX_BOTS = 8;

/**
 * The "New channel" and "New message" dialogs. A
 * channel has a name and members; "direct" is either a DM (exactly one person; an existing DM
 * is opened instead of duplicated) or a group (people and Bots, optionally named). Creation
 * goes through `TeamsWorkspace.createConversation`: the viewer is added first and every chosen
 * Bot must still be usable.
 */
export function NewConversationDialog({ workspace, snapshot, locale, mode, renderAvatar, onClose }: DialogProps & { mode: 'channel' | 'direct' }): ReactElement {
  const t = useTeamsText(locale);
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
  return <Dialog title={mode === 'channel' ? t('teams_new_channel_title') : t('teams_new_message_title')} onClose={onClose}>
    <form className={mode === 'channel' ? 'tw-form tw-new-channel-form' : 'tw-form'} onSubmit={event => { event.preventDefault(); void submit(); }}>
      {mode === 'direct' ? <div className="tw-segment" role="radiogroup" aria-label={t('teams_conversation_type')}>
        {(['dm', 'group'] as const).map(value => <label key={value} className="tw-segment-option" data-checked={direct === value}>
          <input type="radio" name="tw-kind" value={value} checked={direct === value} onChange={() => { setDirect(value); setParties([]); setBotParties([]); }} />
          {value === 'dm' ? t('teams_conversation_dm') : t('teams_conversation_group')}
        </label>)}
      </div> : null}
      {kind !== 'dm' ? <label>{kind === 'channel' ? t('teams_new_channel_name') : t('teams_new_group_name_optional')}
        <input data-autofocus value={title} onChange={event => setTitle(event.target.value)} maxLength={120} required={named} autoComplete="off" />
      </label> : null}
      <fieldset className="tw-fieldset">
        <legend>{kind === 'dm' ? t('tw_new_message_label') : t('tw_people')}</legend>
        {people.map(member => <label key={member.uid} className="tw-check">
          <input type={kind === 'dm' ? 'radio' : 'checkbox'} name="tw-people" checked={parties.includes(member.uid)} onChange={event => choose(member.uid, event.target.checked)} />
          <Avatar identity={{ kind: 'user', id: member.uid, name: member.displayName }} size={24} render={renderAvatar} />
          {member.displayName}
        </label>)}
        {!people.length ? <p className="tw-hint">{t('teams_new_no_members')}</p> : null}
      </fieldset>
      {kind !== 'dm' && bots.length ? <fieldset className="tw-fieldset">
        <legend>{t('teams_new_participating_bots')}</legend>
        {bots.map(bot => <label key={bot.sharedBotId} className="tw-check">
          <input type="checkbox" checked={botParties.includes(bot.sharedBotId)} disabled={!botParties.includes(bot.sharedBotId) && botParties.length >= MAX_BOTS}
            onChange={event => setBotParties(event.target.checked ? [...botParties, bot.sharedBotId] : botParties.filter(key => key !== bot.sharedBotId))} />
          <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={24} render={renderAvatar} />
          {bot.name} · {owner(bot.maintainer.uid)}
        </label>)}
      </fieldset> : null}
      {botParties.length ? <p className="tw-hint">{t('teams_new_bot_owner_note')}</p> : null}
      {snapshot.offline ? <p role="status" className="tw-hint">{t('teams_new_offline')}</p> : null}
      {tried && snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, locale)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('common_cancel')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.messageBusy || snapshot.offline || !valid}>
          {kind === 'dm' ? t('teams_new_open_dm') : kind === 'group' ? t('teams_new_create_group') : t('teams_new_create_channel')}
        </button>
      </div>
    </form>
  </Dialog>;
}

/** Pick a Bot you may use and chat with it: opens the Bot DM you already have, or starts one. */
export function NewBotDmDialog({ workspace, snapshot, locale, renderAvatar, onClose }: DialogProps): ReactElement {
  const t = useTeamsText(locale);
  const [tried, setTried] = useState(false);
  const usable = snapshot.bots.filter(sharedBotUsable);
  const owner = (id: string) => snapshot.directory.find(member => member.uid === id)?.displayName ?? id;
  const open = async (bot: SharedBotCard, fresh: boolean) => { setTried(true); if (await workspace.openBotDm(bot, fresh)) onClose(); };
  return <Dialog title={t('teams_new_bot_title')} onClose={onClose}>
    <div className="tw-form">
      <p className="tw-hint">{t('tw_new_bots_note')}</p>
      {snapshot.botsUnverified ? <p role="status" className="tw-hint">{t('teams_bots_unverified')}</p> : null}
      <ul className="tw-picker" aria-label={t('teams_new_bots_pickable')}>
        {usable.map(bot => {
          const existing = findBotDm(snapshot.conversations, snapshot.uid, bot.sharedBotId, snapshot.bots);
          return <li key={bot.sharedBotId}>
            <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={24} render={renderAvatar} />
            <div className="tw-picker-text">
              <b>{bot.name}</b>
              <small>{bot.ready ? t('remote_online') : t('tw_host_unavailable')} · {t('teams_bot_owner_fmt', owner(bot.maintainer.uid))}</small>
            </div>
            <div className="tw-picker-actions">
              <button type="button" className="tw-primary" disabled={snapshot.messageBusy} onClick={() => void open(bot, false)}>{existing ? t('teams_new_bot_open') : t('teams_new_bot_start')}</button>
              {existing ? <button type="button" disabled={snapshot.messageBusy} onClick={() => void open(bot, true)}>{t('remote_bot_new_chat')}</button> : null}
            </div>
          </li>;
        })}
      </ul>
      {!usable.length && !snapshot.botsUnverified ? <p className="tw-hint">{snapshot.botsLoaded
        ? t('teams_new_no_bots')
        : t('teams_bots_loading')}</p> : null}
      {tried && snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, locale)}</p> : null}
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={onClose}>{t('common_close')}</button></div>
    </div>
  </Dialog>;
}
