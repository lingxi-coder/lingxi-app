import { memo, useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { ChatEvent, VaultProvider } from '../../../../../packages/llm-vault/contracts';
import { ComposerEditor, ComposerSurface, ComposerToolbar } from '../../../../../packages/chat-ui/ComposerSurface';
import { sendsOnEnter } from '../../../../../packages/teams/compose';
import { ShareLinkButton } from '../../../../../packages/teams/ShareLinkButton';
import type { ChatTarget } from '../../app/router';
import { IconArrowUp, IconCopy, IconRefresh, IconSliders, IconStop, IconTrash } from '../../components/Icons';
import { Markdown } from '../../remote/markdown';
import { LlmError, type LlmClient } from './client';
import { errorText } from './errors';
import { contextOf, newId, titleOf, type AgentChat, type AgentChatSummary, type AgentHistory, type AgentMessage } from './history';
import { ProviderSettings } from './ProviderSettings';
import { agentShareLink } from './share';

export interface AgentHostProps {
  uid: string;
  name: string;
  locale: string;
  target: ChatTarget;
  client: LlmClient;
  history: AgentHistory;
  /** The account's model services, loaded once per page (`useModelServices`) so moving between panes does not refetch them. */
  models: ModelServices;
  onAgentChats(chats: AgentChatSummary[]): void;
  open(target: ChatTarget, replace?: boolean): void;
  /** A prompt to put in the composer of a new chat (a shared `new` link); never sent by itself. */
  initialPrompt?: string;
  /** The composer took `initialPrompt`: it is not offered again when the pane comes back. */
  onPromptTaken?(): void;
  /** Schedules a render of streamed text; tests pass a synchronous one. */
  frame?(callback: () => void): void;
}

export type Providers = { state: 'loading' } | { state: 'ready'; list: VaultProvider[] } | { state: 'error'; code: string };
export interface ModelServices { providers: Providers; load(): void; saved(provider: VaultProvider | undefined): void }

/** The account's model services: loaded on first use, then kept current by what this page saves or deletes. */
export function useModelServices(client: LlmClient): ModelServices {
  const [providers, setProviders] = useState<Providers>({ state: 'loading' });
  const load = useCallback(() => {
    setProviders(current => current.state === 'error' ? { state: 'loading' } : current);
    client.listProviders().then(list => setProviders({ state: 'ready', list }), error => setProviders({ state: 'error', code: error instanceof LlmError ? error.code : 'UNAVAILABLE' }));
  }, [client]);
  useEffect(load, [load]);
  // A save is folded into a loaded list; before the list has loaded (or after it failed) the whole list is read again,
  // so one saved service never stands in for the others.
  const saved = useCallback((provider: VaultProvider | undefined) => {
    if (!provider) { load(); return; }
    let reload = false;
    setProviders(current => {
      if (current.state !== 'ready') { reload = true; return current; }
      return { state: 'ready', list: [...current.list.filter(p => p.id !== provider.id), provider] };
    });
    if (reload) load();
  }, [load]);
  return useMemo(() => ({ providers, load, saved }), [providers, load, saved]);
}
interface Choice { providerId: string; model: string }
const choiceKey = (uid: string) => `lingxi.agent.model.${uid}`;
const readChoice = (uid: string): Choice | undefined => { try { const value = JSON.parse(localStorage.getItem(choiceKey(uid)) ?? 'null'); return value?.providerId && value?.model ? value : undefined; } catch { return undefined; } };
const writeChoice = (uid: string, choice: Choice) => { try { localStorage.setItem(choiceKey(uid), JSON.stringify(choice)); } catch { /* per-browser convenience only */ } };
const modelsOf = (provider: VaultProvider) => provider.models.length ? provider.models : provider.defaultModel ? [{ id: provider.defaultModel }] : [];
/** `wanted` when the provider still offers it, else its default model, else its first. */
const modelOf = (provider: VaultProvider, wanted?: string) => {
  const models = modelsOf(provider);
  return (models.find(model => model.id === wanted) ?? models.find(model => model.id === provider.defaultModel) ?? models[0])!.id;
};
const defaultFrame = (callback: () => void) => { if (typeof requestAnimationFrame === 'function') requestAnimationFrame(callback); else setTimeout(callback, 16); };

/** The Agent side of the Chat workspace: model settings, the new-chat home and saved Agent chats. */
export function AgentHost(props: AgentHostProps) {
  const { uid, client, history, onAgentChats, locale, models } = props;
  const { providers, load } = models;
  const refreshChats = useCallback(() => { void history.list(uid).then(list => onAgentChats(list), () => undefined); }, [history, uid, onAgentChats]);
  useEffect(refreshChats, [refreshChats]);
  const [dialog, setDialog] = useState(false);

  const list = providers.state === 'ready' ? providers.list : [];
  const settings = (onClose?: () => void) => <ProviderSettings locale={locale} client={client} providers={list} onClose={onClose} onChanged={models.saved} />;

  if (props.target.kind === 'providers') return <div className="agent-pane agent-pane-settings">{settings()}</div>;
  return <>
    <AgentPane {...props} providers={providers} onRetryProviders={load} onSetup={() => setDialog(true)} onSaved={refreshChats} />
    {dialog ? <SettingsDialog title={locale === 'zh' ? '模型服务' : 'Model services'} onClose={() => setDialog(false)}>{settings(() => setDialog(false))}</SettingsDialog> : null}
  </>;
}

interface PaneProps extends AgentHostProps { providers: Providers; onRetryProviders(): void; onSetup(): void; onSaved(): void }

/**
 * The model settings as a modal on the native `<dialog>`: focus moves in and stays inside, Escape or a click outside the
 * panel closes it, and focus goes back to what opened it.
 */
function SettingsDialog({ title, onClose, children }: { title: string; onClose(): void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null), id = useId();
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (typeof node.showModal === 'function') { if (!node.open) node.showModal(); } else node.setAttribute('open', '');
    node.querySelector<HTMLElement>('button, input, select, textarea')?.focus();
    return () => { if (node.open && typeof node.close === 'function') node.close(); opener?.focus(); };
  }, []);
  return <dialog ref={ref} className="agent-dialog" aria-labelledby={id}
    onCancel={event => { event.preventDefault(); close.current(); }}
    onClick={event => { if (event.target === event.currentTarget) close.current(); }}>
    <h2 id={id} className="sr-only">{title}</h2>
    {children}
  </dialog>;
}

/** An answer's Markdown, parsed again only when its text changes, not on every streamed frame of a later answer. */
const AnswerText = memo(function AnswerText({ text }: { text: string }) { return <Markdown text={text} />; });

function AgentPane({ uid, name, locale, target, client, history, open, initialPrompt, onPromptTaken, providers, onRetryProviders, onSetup, onSaved, frame = defaultFrame }: PaneProps) {
  const zh = locale === 'zh';
  const t = (en: string, cn: string) => zh ? cn : en;
  const chatId = target.kind === 'agent' ? target.id : undefined;
  const [chat, setChat] = useState<AgentChat>();
  const [missing, setMissing] = useState(false);
  const [draft, setDraft] = useState(initialPrompt ?? '');
  const [streaming, setStreaming] = useState<AbortController>();
  const [choice, setChoice] = useState<Choice | undefined>(() => readChoice(uid));
  const chatRef = useRef<AgentChat>();
  // Chats deleted here: an answer still streaming into one of them is not saved back.
  const removed = useRef(new Set<string>());
  const scroller = useRef<HTMLDivElement>(null);
  const editor = useRef<HTMLTextAreaElement>(null);

  // Opening another chat stops any answer still streaming and loads that chat. The chat being streamed into keeps its
  // state when its own URL takes over from /chat.
  const live = useRef<AbortController>();
  useEffect(() => {
    if (chatId && chatRef.current?.id === chatId) return;
    live.current?.abort();
    if (!chatId) { chatRef.current = undefined; setChat(undefined); setMissing(false); return; }
    let current = true;
    setMissing(false);
    void history.get(uid, chatId).then(found => { if (!current) return; chatRef.current = found; setChat(found); setMissing(!found); }, () => { if (current) setMissing(true); });
    return () => { current = false; };
  }, [chatId, uid, history]);
  useEffect(() => () => live.current?.abort(), []);
  useEffect(() => {
    if (!initialPrompt) return;
    setDraft(initialPrompt); editor.current?.focus(); onPromptTaken?.();
  // Taken once, when it arrives.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialPrompt]);

  const usable = useMemo(() => providers.state === 'ready' ? providers.list.filter(provider => provider.webUsable && modelsOf(provider).length) : [], [providers]);
  const selected = useMemo(() => {
    const wanted = chat?.providerId && chat.model ? { providerId: chat.providerId, model: chat.model } : choice;
    const provider = usable.find(p => p.id === wanted?.providerId) ?? usable[0];
    return provider ? { provider, model: modelOf(provider, provider.id === wanted?.providerId ? wanted.model : undefined) } : undefined;
  }, [usable, chat?.providerId, chat?.model, choice]);

  const persist = async (next: AgentChat) => {
    if (removed.current.has(next.id)) return;
    try { await history.put(next); onSaved(); } catch { /* kept in memory for this page */ }
  };
  const update = (next: AgentChat) => { chatRef.current = next; setChat(next); };
  useEffect(() => { scroller.current?.scrollTo?.({ top: scroller.current.scrollHeight }); }, [chat?.messages.length]);

  const run = async (base: AgentChat, pick: { provider: VaultProvider; model: string }) => {
    const answer: AgentMessage = { id: newId(), role: 'assistant', content: '', model: pick.model, at: Date.now() };
    let current: AgentChat = { ...base, providerId: pick.provider.id, model: pick.model, updatedAt: Date.now(), messages: [...base.messages, answer] };
    update(current);
    void persist(current);
    const controller = new AbortController();
    live.current = controller;
    setStreaming(controller);
    let content = '', reasoning = '', scheduled = false, ended: Partial<AgentMessage> = {};
    // The answer is kept whole while another chat is open, so what is saved at the end is all of it. A model picked
    // while it streams is kept for the chat's next turn.
    const paint = () => {
      scheduled = false;
      const shown = chatRef.current?.id === current.id ? chatRef.current : undefined;
      current = { ...current, ...(shown ? { providerId: shown.providerId, model: shown.model } : {}),
        messages: current.messages.map(message => message.id === answer.id ? { ...message, content, ...(reasoning ? { reasoning } : {}), ...ended } : message) };
      if (shown) update(current);
    };
    const onEvent = (event: ChatEvent) => {
      if (event.type === 'delta') content += event.text;
      else if (event.type === 'reasoning') reasoning += event.text;
      else return;
      if (!scheduled) { scheduled = true; frame(paint); }
    };
    try {
      const last = await client.streamChat({ providerId: pick.provider.id, model: pick.model, messages: contextOf(base.messages) }, onEvent, controller.signal);
      if (last.type === 'error') ended = { error: last.code };
    } catch (error) {
      ended = controller.signal.aborted ? { stopped: true } : { error: error instanceof LlmError ? error.code : 'NETWORK' };
    }
    paint();
    setStreaming(current => current === controller ? undefined : current);
    current = { ...current, updatedAt: Date.now() };
    void persist(current);
    if (chatRef.current?.id === current.id) update(current);
  };

  const send = () => {
    const text = draft.trim();
    if (!text || streaming) return;
    // Until the services have loaded there is nothing to choose from yet; a failed load shows its own card and retry.
    if (!selected) { if (providers.state === 'ready') onSetup(); return; }
    const now = Date.now(), message: AgentMessage = { id: newId(), role: 'user', content: text, at: now };
    const base: AgentChat = chat ? { ...chat, messages: [...chat.messages, message] }
      : { uid, id: newId(), title: titleOf(text), createdAt: now, updatedAt: now, messages: [message] };
    setDraft('');
    chatRef.current = base;
    if (!chat) open({ kind: 'agent', id: base.id }, true);
    void run(base, selected);
  };
  const retry = () => {
    const last = chat?.messages.at(-1);
    if (!chat || streaming || !selected || last?.role !== 'assistant') return;
    void run({ ...chat, messages: chat.messages.slice(0, -1) }, selected);
  };
  const remove = async () => {
    if (!chat) return;
    removed.current.add(chat.id);
    streaming?.abort();
    await history.remove(uid, chat.id).catch(() => undefined);
    onSaved();
    chatRef.current = undefined;
    open({ kind: 'home' }, true);
  };
  const pickModel = (value: string) => {
    if (value === '\u0000settings') { onSetup(); return; }
    const [providerId, model] = value.split('\n');
    if (!providerId || !model) return;
    const next = { providerId, model };
    setChoice(next); writeChoice(uid, next);
    if (chat) { const changed = { ...chat, providerId, model }; update(changed); void persist(changed); }
  };
  const onKey = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (sendsOnEnter({ key: event.key, shiftKey: event.shiftKey, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); send(); }
  };

  const composer = <ComposerSurface className="agent-composer">
    <ComposerEditor>
      <textarea ref={editor} className="composer-editor-control" value={draft} rows={2} aria-label={t('Message', '消息')}
        placeholder={selected ? t('Ask anything', '给灵犀发送消息') : t('Connect a model service to start', '连接模型服务后即可开始')}
        onChange={event => setDraft(event.target.value)} onKeyDown={onKey} />
    </ComposerEditor>
    <ComposerToolbar>
      {usable.length ? <label className="agent-model-chip">
        <span className="sr-only">{t('Model', '模型')}</span>
        <select value={selected ? `${selected.provider.id}\n${selected.model}` : ''} onChange={event => pickModel(event.target.value)}>
          {usable.map(provider => <optgroup key={provider.id} label={provider.label}>
            {modelsOf(provider).map(model => <option key={model.id} value={`${provider.id}\n${model.id}`}>{model.name ?? model.id}</option>)}
          </optgroup>)}
          <option value={'\u0000settings'}>{t('Manage model services…', '管理模型服务…')}</option>
        </select>
      </label> : providers.state === 'ready' ? <button type="button" className="agent-model-chip agent-model-setup" onClick={onSetup}><IconSliders width={15} height={15} />{t('Set up a model', '配置模型')}</button>
        : providers.state === 'loading' ? <span className="agent-model-chip agent-model-loading" role="status">{t('Loading models…', '正在加载模型…')}</span> : null}
      <span className="agent-toolbar-gap" />
      {streaming
        ? <button type="button" className="agent-send agent-stop" aria-label={t('Stop', '停止')} onClick={() => streaming.abort()}><IconStop width={16} height={16} /></button>
        : <button type="button" className="agent-send" aria-label={t('Send', '发送')} disabled={!draft.trim() || providers.state !== 'ready'} onClick={send}><IconArrowUp width={18} height={18} /></button>}
    </ComposerToolbar>
  </ComposerSurface>;

  const noProvider = providers.state === 'ready' && !usable.length;
  const setupCard = noProvider ? <div className="agent-setup-card">
    <div><strong>{t('Connect a model service to start chatting', '连接一个模型服务，即可开始对话')}</strong>
      <p>{t('Use your own key for DeepSeek, Kimi, Qwen, OpenAI, Claude, Gemini and more. Keys you saved in the desktop app with sync turned on appear here.', '使用你自己的 DeepSeek、Kimi、通义千问、OpenAI、Claude、Gemini 等服务的 key。桌面端开启同步后保存的 key 也会出现在这里。')}</p></div>
    <button type="button" className="chat-button" onClick={onSetup}>{t('Set up a model', '配置模型')}</button>
  </div> : providers.state === 'error' ? <div className="agent-setup-card" role="alert">
    <div><strong>{t('Model services could not be loaded', '模型服务暂时无法加载')}</strong><p>{errorText(providers.code, zh)}</p></div>
    <button type="button" className="chat-button chat-button-quiet" onClick={onRetryProviders}>{t('Try again', '重试')}</button>
  </div> : null;

  if (missing) return <div className="chat-notice">
    <h2>{t('This chat is not in this browser', '这个对话不在当前浏览器中')}</h2>
    <p>{t('Agent chats are kept in the browser they were made in.', 'Agent 对话只保存在创建它的浏览器里。')}</p>
    <button type="button" className="chat-button" onClick={() => open({ kind: 'home' }, true)}>{t('New chat', '开启新对话')}</button>
  </div>;

  if (!chat) return <div className="agent-pane agent-home">
    <div className="agent-home-inner">
      <h2>{t(`What's on your mind, ${name}?`, `${name}，今天想聊点什么？`)}</h2>
      <p className="agent-home-sub">{t('Chat with a model, or pick a team, friend or Bot on the left.', '和模型聊聊，或者从左侧选择团队、好友和 Bot 的对话。')}</p>
      {setupCard}
      {composer}
      {!history.persistent ? <p className="agent-footnote">{t('This browser cannot keep chats; they are gone when the page closes.', '当前浏览器无法保存对话，关闭页面后对话会消失。')}</p> : null}
    </div>
  </div>;

  const last = chat.messages.at(-1);
  return <div className="agent-pane">
    <header className="agent-head">
      <h2>{chat.title || t('New chat', '新对话')}</h2>
      <div className="agent-head-actions">
      <ShareQuestion chat={chat} locale={locale} />
      <button type="button" className="chat-icon-button" aria-label={t('Delete this chat', '删除这个对话')} onClick={() => void remove()}><IconTrash width={16} height={16} /></button>
      </div>
    </header>
    <div className="agent-scroll" ref={scroller}>
      <ol className="agent-transcript" aria-live="polite">
        {chat.messages.map(message => message.role === 'user'
          ? <li key={message.id} className="agent-user"><div className="agent-bubble">{message.content}</div></li>
          : <li key={message.id} className="agent-answer">
            {message.reasoning ? <details className="agent-reasoning" open={!!streaming && message === last && !message.content}>
              <summary>{streaming && message === last && !message.content ? t('Thinking…', '思考中…') : t('Thought process', '思考过程')}</summary>
              <p>{message.reasoning}</p>
            </details> : null}
            {message.content ? <AnswerText text={message.content} /> : streaming && message === last && !message.reasoning ? <span className="agent-typing" aria-label={t('Answering', '正在回答')}><i /><i /><i /></span> : null}
            {message.error ? <p className="agent-status agent-status-error" role="alert">{errorText(message.error, zh)}</p> : null}
            {message.stopped ? <p className="agent-stopped">{t('Stopped', '已停止')}</p> : null}
            {!(streaming && message === last) ? <div className="agent-answer-tools">
              {message.content ? <button type="button" className="chat-icon-button" aria-label={t('Copy', '复制')} onClick={() => void navigator.clipboard?.writeText(message.content).catch(() => undefined)}><IconCopy width={15} height={15} /></button> : null}
              {message === last ? <button type="button" className="chat-icon-button" aria-label={t('Retry', '重新回答')} onClick={retry} disabled={!selected}><IconRefresh width={15} height={15} /></button> : null}
              {message.model ? <span className="agent-answer-model">{message.model}</span> : null}
            </div> : null}
          </li>)}
      </ol>
    </div>
    <div className="agent-dock">{setupCard}{composer}<p className="agent-footnote">{t('Answers come from the model you chose and may be wrong.', '回答由你选择的模型生成，可能有误。')}</p></div>
  </div>;
}

/** Shares the chat's first question as a link that opens a new chat with it in the composer; the answers stay in this browser. */
function ShareQuestion({ chat, locale }: { chat: AgentChat; locale: string }) {
  const link = agentShareLink(chat);
  if (!link) return null;
  return <ShareLinkButton link={link} title={chat.title} locale={locale}
    note={locale === 'zh' ? '只包含第一个问题，不含回答' : 'the first question only, without answers'} />;
}
