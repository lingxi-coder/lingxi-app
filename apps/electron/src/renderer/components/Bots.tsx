import { useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from 'react';
import type { BotUi, BotsApi } from '../../shared/bots';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useT } from '../theme/ThemeContext';
import { botDraftProblems, createBot, emptyBotDraft, hasBotDraftProblems, sortBots, type BotDraft, type BotDraftProblems } from '../bridge/botForm';
import { LoginItemController } from '../bridge/botLoginItem';
import { botActivity, botRuntimes, relativeActivity, type BotActivity } from '../bridge/botSession';
import type { BotsState } from '../bridge/botsStore';
import type { UseBots } from '../bridge/useBots';
import type { UseBridge } from '../bridge/bridgeTypes';
import { AgentAvatar } from './AgentAvatar';
import { BotDetail, BotStateChip } from './BotDetail';
import { BotConfirm, BotFoldersField, BotGeneralFields, BotPermissionField, BotPersonaField, useObservable, type ModelChoices } from './BotFields';
import { BotCreateIntro } from './BotCreateIntro';
import { BotLeaveGuard } from '../bridge/botLeaveGuard';
import { draftFromProposal, type BotCreationProposal } from '../bridge/botCreation';
import { BotsBackground } from './BotsBackground';
import { Icon } from './Icon';

/** The Bots page: the list, the form that makes a Bot, and one Bot with its settings. */
export function Bots({ visible, bots, api, bridge, onOpenChat, onNewRoutine, onOpenRoutine, onAssist }: {
  visible: boolean;
  bots: UseBots;
  api: BotsApi;
  bridge: Pick<UseBridge, 'bootstrap' | 'desktop' | 'sessionLoading' | 'sessionRuntimeStatus' | 'openBotSession' | 'deleteBot' | 'pickBotFolder' | 'scheduledScopes' | 'manageScheduled' | 'setBotsKeepAwake'>;
  /** Called once a chat of a Bot is on screen, to show the chat view. */
  onOpenChat(): void;
  /** Show the Scheduled page's editor for a new task in a Bot's scope. */
  onNewRoutine(scopeId: string): void;
  /** Show a task of a Bot's scope on the Scheduled page. */
  onOpenRoutine(scopeId: string, jobId: string): void;
  onAssist?(request: string): Promise<void>;
}) {
  const t = useT();
  useI18n();
  const { controller, state, view, setView } = bots;
  useEffect(() => { if (visible) void controller.reload(); }, [controller, visible]);
  // The owner can change the login item in System Settings: read it again whenever the page comes into view or the window is returned to.
  const [loginItem] = useState(() => new LoginItemController(api.loginItem));
  useEffect(() => {
    if (!visible) return;
    const refresh = () => { void loginItem.refresh(); };
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [loginItem, visible]);

  const runtimes = botRuntimes(bridge.bootstrap?.runtimes ?? [], bridge.sessionRuntimeStatus);
  const activityOf = (botId: string): BotActivity => botActivity(botId, runtimes);
  const models: ModelChoices = { references: bridge.desktop.models, details: bridge.desktop.modelDetails };
  const style = {
    '--bots-background': t.stageBg, '--bots-text': t.text, '--bots-muted': t.text3, '--bots-surface': t.surface,
    '--bots-surface-hover': t.surfaceHover, '--bots-border': t.border, '--bots-accent': t.accent, '--bots-accent-bg': t.accentBg,
    '--bots-error': t.danger, '--bots-ok': t.ok, '--bots-warn': t.warn,
  } as CSSProperties;

  return <section className="bots-page" aria-label={tr('bots_title')} style={style}>
    {view.kind === 'create'
      ? <BotCreate key={view.requestId ?? 'new'} api={api} models={models} pickFolder={bridge.pickBotFolder} onCancel={() => setView({ kind: 'list' })}
        initialProposal={view.proposal} guard={bots.guard} onAssist={onAssist}
        onCreated={(bot) => { controller.put(bot); setView({ kind: 'detail', botId: bot.botId }); }} />
      : view.kind === 'detail'
        ? <BotDetail key={view.botId} botId={view.botId} listed={state.bots.find((bot) => bot.botId === view.botId)} api={api} bots={controller} guard={bots.guard}
          activity={activityOf(view.botId)} models={models} pickFolder={bridge.pickBotFolder} visible={visible} sessionLoading={bridge.sessionLoading}
          routines={bridge} onNewRoutine={onNewRoutine} onOpenRoutine={onOpenRoutine} deleteRequest={view.deleteRequest}
          onBack={() => setView({ kind: 'list' })}
          onOpenChat={async () => { await bridge.openBotSession(view.botId); onOpenChat(); }}
          onOpenTaskChat={async (botId, sessionId) => { await bridge.openBotSession(botId, sessionId); onOpenChat(); }}
          onDelete={async () => { await bridge.deleteBot(view.botId); controller.remove(view.botId); setView({ kind: 'list' }); }} />
        : <BotList state={state} activityOf={activityOf} onNew={() => setView({ kind: 'create' })}
          onOpen={(botId) => setView({ kind: 'detail', botId })} onRetry={() => void controller.reload()}
          background={<BotsBackground keepAwake={bridge.bootstrap?.settings?.botsKeepAwake === true} onKeepAwake={(enabled) => void bridge.setBotsKeepAwake(enabled)} loginItem={loginItem} />} />}
  </section>;
}

export function BotList({ state, activityOf, onNew, onOpen, onRetry, background }: {
  state: BotsState;
  activityOf(botId: string): BotActivity;
  onNew(): void;
  onOpen(botId: string): void;
  onRetry(): void;
  /** What the Bots need of this Mac to go on working; shown below the Bots, once there are some. */
  background?: ReactNode;
}) {
  useI18n();
  const bots = sortBots(state.bots);
  const loading = state.availability === undefined || (state.loading && bots.length === 0);
  const empty = !loading && state.error === undefined && bots.length === 0;
  return <>
    <header className="bots-header">
      <div><h1>{tr('bots_title')}</h1><p>{tr('bots_tagline')}</p></div>
      <button type="button" data-variant="primary" onClick={onNew}>{tr('bots_new')}</button>
    </header>
    {state.error !== undefined ? <p role="alert" className="bots-problem">{tr('bots_load_failed')} {state.error} <button type="button" onClick={onRetry}>{tr('common_retry')}</button></p> : null}
    {loading ? <p role="status" className="bots-muted">{tr('bots_loading')}</p> : null}
    {empty ? <div className="bots-empty">
      <Icon name="bot" size={38} stroke={1.5} />
      <h2>{tr('bots_empty_title')}</h2>
      <p>{tr('bots_empty_body')}</p>
      <button type="button" data-variant="primary" onClick={onNew}>{tr('bots_create_first')}</button>
    </div> : null}
    {bots.length > 0 ? <ul className="bots-grid" aria-label={tr('bots_list_label')}>
      {bots.map((bot) => <BotCard key={bot.botId} bot={bot} activity={activityOf(bot.botId)} onOpen={() => onOpen(bot.botId)} />)}
    </ul> : null}
    {bots.length > 0 ? background : null}
  </>;
}

function BotCard({ bot, activity, onOpen }: { bot: BotUi; activity: BotActivity; onOpen(): void }) {
  return <li>
    <button type="button" className="bots-card" onClick={onOpen}>
      <span className="bots-avatar" aria-hidden="true"><AgentAvatar agentId={bot.botId} size={40} /></span>
      <span className="bots-card-name">{bot.name}</span>
      <span className="bots-card-description">{bot.description}</span>
      <span className="bots-card-footer">
        <BotStateChip activity={activity} />
        <span className="bots-muted">{bot.lastActivityAt !== undefined ? tr('bots_last_active', relativeActivity(bot.lastActivityAt)) : tr('bots_no_chats_yet')}</span>
      </span>
    </button>
  </li>;
}

/** A name nobody has typed yet is not a mistake worth pointing at; once the owner tries to create the Bot it is. */
export function shownProblems(problems: BotDraftProblems, attempted: boolean): BotDraftProblems {
  if (attempted || problems.name !== 'name_required') return problems;
  const shown: BotDraftProblems = { ...problems };
  delete shown.name;
  return shown;
}

export function BotCreate({ api, models, pickFolder, onCancel, onCreated, initialProposal, guard: providedGuard, onAssist }: {
  api: Pick<BotsApi, 'create'>;
  models: ModelChoices;
  pickFolder(): Promise<{ path: string } | null>;
  onCancel(): void;
  onCreated(bot: BotUi): void;
  initialProposal?: BotCreationProposal;
  guard?: BotLeaveGuard;
  onAssist?(request: string): Promise<void>;
}) {
  const [draft, setDraft] = useState<BotDraft>(() => initialProposal ? draftFromProposal(initialProposal) : emptyBotDraft());
  const [mode, setMode] = useState<'assist' | 'manual'>(() => onAssist && !initialProposal ? 'assist' : 'manual');
  const [request, setRequest] = useState('');
  const [opening, setOpening] = useState(false);
  const [localGuard] = useState(() => new BotLeaveGuard(() => {}));
  const guard = providedGuard ?? localGuard;
  const leaving = useObservable(guard);
  const dirty = Boolean(request || draft.name || draft.description || draft.persona || draft.model || draft.workspaces.length || draft.permissionMode !== 'default');
  useEffect(() => { guard.hold(dirty); }, [guard, dirty]);
  useEffect(() => () => guard.hold(false), [guard]);
  const saving = useRef(false);
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const problems = botDraftProblems(draft);
  const change = (next: Partial<BotDraft>) => { setDraft((current) => ({ ...current, ...next })); setError(''); };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (saving.current) return;
    setAttempted(true);
    if (hasBotDraftProblems(problems)) return;
    saving.current = true;
    setBusy(true);
    setError('');
    const created = await createBot(api, draft);
    saving.current = false;
    setBusy(false);
    if (created.ok) { guard.hold(false); onCreated(created.bot); }
    else setError(created.message);
  };
  const shown = shownProblems(problems, attempted);
  return <div className="bots-detail">
    <button type="button" className="bots-back" data-variant="quiet" disabled={busy || opening} onClick={() => guard.leave(onCancel)}>← {tr('bots_back')}</button>
    <header className="bots-header"><div><h1>{tr(initialProposal ? 'bots_assist_review_title' : 'bots_new')}</h1>
      {initialProposal ? <p>{tr('bots_assist_review_hint')}</p> : null}</div></header>
    {onAssist && !initialProposal ? <div className="bot-create-mode" role="group" aria-label={tr('bots_assist_method')}>
      <button type="button" aria-pressed={mode === 'assist'} disabled={busy || opening} onClick={() => setMode('assist')}>{tr('bots_assist_mode')}</button>
      <button type="button" aria-pressed={mode === 'manual'} disabled={busy || opening} onClick={() => setMode('manual')}>{tr('bots_assist_manual')}</button>
    </div> : null}
    {mode === 'assist' && onAssist ? <BotCreateIntro request={request} onChange={setRequest} onContinue={async text => {
      setOpening(true);
      try { await onAssist(text); }
      finally { setOpening(false); }
    }} /> : <form className="bot-create-fields" aria-busy={busy || undefined} noValidate onSubmit={(event) => void submit(event)}>
    <BotGeneralFields draft={draft} problems={shown} models={models} disabled={busy} onChange={change} />
    <BotPersonaField draft={draft} problems={shown} disabled={busy} onChange={change} />
    <BotPermissionField draft={draft} disabled={busy} onChange={change} />
    <BotFoldersField draft={draft} problems={shown} disabled={busy} pickFolder={pickFolder} onChange={change} />
    <footer className="bots-savebar">
      <div className="bots-save-status">{error ? <p role="alert" className="bots-problem">{tr('bots_create_failed', error)}</p> : null}</div>
      <button type="submit" data-variant="primary" disabled={busy}>{busy ? tr('bots_creating') : tr('bots_create')}</button>
    </footer>
    </form>}
    {leaving.asking ? <BotConfirm title={tr('bots_discard_title')} icon="info" danger={false} confirmLabel={tr('bots_discard_confirm')}
      onCancel={() => guard.cancel()} onConfirm={() => guard.confirm()}><p>{tr('bots_discard_body')}</p></BotConfirm> : null}
  </div>;
}
